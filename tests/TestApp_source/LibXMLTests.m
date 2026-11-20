/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

#include <dlfcn.h>
#include <stdio.h>
#include <string.h>

// Public libxml2 layouts used by Apple's XMLPerformance sample, also bundled
// by Rolando. Keep the fields here rather than relying on host libxml headers:
// these tests execute as 32-bit iOS code and must exercise guest field access.
// https://developer.apple.com/library/archive/samplecode/XMLPerformance/
// https://github.com/GNOME/libxml2/blob/v2.9.14/include/libxml/xpath.h
typedef struct _xmlDoc xmlDoc;
typedef struct _xmlNode {
  void *_private;
  int type;
  const unsigned char *name;
  struct _xmlNode *children, *last, *parent, *next, *prev;
  xmlDoc *doc;
} xmlNode;
typedef struct {
  xmlDoc *doc;
  xmlNode *node;
} xmlXPathContext;
typedef struct {
  int nodeNr, nodeMax;
  xmlNode **nodeTab;
} xmlNodeSet;
typedef struct {
  int type;
  xmlNodeSet *nodesetval;
  int boolval;
  double floatval;
  unsigned char *stringval;
  void *user;
  int index;
  void *user2;
  int index2;
} xmlXPathObject;

extern xmlDoc *xmlReadMemory(const char *, int, const char *, const char *,
                             int);
extern xmlNode *xmlDocGetRootElement(xmlDoc *);
extern void xmlFreeDoc(xmlDoc *);
extern xmlXPathContext *xmlXPathNewContext(xmlDoc *);
extern void xmlXPathFreeContext(xmlXPathContext *);
extern xmlXPathObject *xmlXPathEvalExpression(const unsigned char *,
                                              xmlXPathContext *);
extern void xmlXPathFreeObject(xmlXPathObject *);
extern unsigned char *xmlGetProp(xmlNode *, const unsigned char *);
extern void xmlFree(void *);

#define XML_CHECK(condition)                                                   \
  do {                                                                         \
    if (!(condition)) {                                                        \
      printf("libxml guest layout failure at line %d: %s\n", __LINE__,         \
             #condition);                                                      \
      return 1;                                                                \
    }                                                                          \
  } while (0)

int test_libxml_xpath_public_structs(void) {
  // The bundled iOS libxml2 predates this API. Resolve HyperHLE's export at
  // runtime so the old guest link library cannot hide the symbol from ld.
  int (*setContextNode)(xmlNode *, xmlXPathContext *) =
      dlsym(RTLD_DEFAULT, "xmlXPathSetContextNode");
  XML_CHECK(setContextNode);
  const char *source =
      "<worlds><world><level url='Hello Rolando'/><level url='Second'/></world>"
      "<world><level url='Third'/></world></worlds>";
  // Repeated parses exercise teardown, including reuse of host node pointers.
  for (int iteration = 0; iteration < 8; iteration++) {
    xmlDoc *doc = xmlReadMemory(source, strlen(source), NULL, NULL, 0);
    XML_CHECK(doc);
    xmlNode *root = xmlDocGetRootElement(doc);
    XML_CHECK(root && root->doc == doc && root->type == 1);
    XML_CHECK(!strcmp((const char *)root->name, "worlds"));
    xmlXPathContext *context = xmlXPathNewContext(root->doc);
    XML_CHECK(context && context->doc == doc);
    context->node = root; // Apple's wrapper writes this field directly.
    xmlXPathObject *result =
        xmlXPathEvalExpression((const unsigned char *)"//world", context);
    XML_CHECK(result && result->type == 1 && result->nodesetval);
    XML_CHECK(result->nodesetval->nodeNr == 2);
    xmlNode *world = result->nodesetval->nodeTab[0];
    xmlNode *otherWorld = result->nodesetval->nodeTab[1];
    XML_CHECK(world && otherWorld && world != otherWorld);
    XML_CHECK(world->doc == doc && world->parent == root);
    xmlXPathFreeObject(result);

    // Result teardown must leave the document's nodes alive. A relative query
    // must use the node written by the guest, not the host's old context node.
    context->node = world;
    result = xmlXPathEvalExpression((const unsigned char *)"level", context);
    XML_CHECK(result && result->nodesetval->nodeNr == 2);
    xmlNode *level = result->nodesetval->nodeTab[0];
    XML_CHECK(level && level->doc == doc && level->type == 1);
    unsigned char *name = xmlGetProp(level, (const unsigned char *)"url");
    XML_CHECK(name && !strcmp((const char *)name, "Hello Rolando"));
    xmlFree(name);
    xmlXPathFreeObject(result);

    XML_CHECK(setContextNode(otherWorld, context) == 0);
    XML_CHECK(context->node == otherWorld);
    result = xmlXPathEvalExpression((const unsigned char *)"level", context);
    XML_CHECK(result && result->nodesetval->nodeNr == 1);
    xmlXPathFreeObject(result);
    xmlXPathFreeContext(context);
    xmlFreeDoc(doc);
  }
  return 0;
}

int test_libxml_xpath_scalar_layouts(void) {
  const char *source = "<root><item>hello</item><item>world</item></root>";
  xmlDoc *doc = xmlReadMemory(source, strlen(source), NULL, NULL, 0);
  XML_CHECK(doc);
  xmlXPathContext *context = xmlXPathNewContext(doc);
  XML_CHECK(context);
  context->node = xmlDocGetRootElement(doc);
  xmlXPathObject *result =
      xmlXPathEvalExpression((const unsigned char *)"count(item)", context);
  XML_CHECK(result && result->type == 3 && result->floatval == 2.0);
  xmlXPathFreeObject(result);
  result =
      xmlXPathEvalExpression((const unsigned char *)"boolean(item)", context);
  XML_CHECK(result && result->type == 2 && result->boolval == 1);
  xmlXPathFreeObject(result);
  result =
      xmlXPathEvalExpression((const unsigned char *)"string(item[1])", context);
  XML_CHECK(result && result->type == 4 && result->stringval);
  XML_CHECK(!strcmp((const char *)result->stringval, "hello"));
  xmlXPathFreeObject(result);
  result = xmlXPathEvalExpression((const unsigned char *)"absent", context);
  XML_CHECK(result && result->nodesetval && result->nodesetval->nodeNr == 0);
  xmlXPathFreeObject(result);
  xmlXPathFreeContext(context);
  xmlFreeDoc(doc);
  return 0;
}

int test_libxml_xpath_document_and_namespace_nodes(void) {
  const char *source = "<!DOCTYPE root [<!ENTITY text 'hello'>]>"
                       "<root xmlns:sdk='urn:ios'><sdk:item/></root>";
  xmlDoc *doc = xmlReadMemory(source, strlen(source), NULL, NULL, 0);
  XML_CHECK(doc);
  xmlNode *root = xmlDocGetRootElement(doc);
  xmlXPathContext *context = xmlXPathNewContext(doc);
  XML_CHECK(root && context);
  context->node = root;
  xmlXPathObject *result =
      xmlXPathEvalExpression((const unsigned char *)"/", context);
  XML_CHECK(result && result->nodesetval->nodeNr == 1);
  XML_CHECK(result->nodesetval->nodeTab[0]->type == 9);
  xmlXPathFreeObject(result);

  // Namespace results have xmlNs's layout, and belong to the XPath result.
  // Interpreting them as xmlNode reads beyond the host allocation.
  typedef struct {
    void *next;
    int type;
    const unsigned char *href, *prefix;
    void *_private;
    xmlDoc *context;
  } xmlNs;
  context->node = root;
  result =
      xmlXPathEvalExpression((const unsigned char *)"namespace::sdk", context);
  XML_CHECK(result && result->nodesetval->nodeNr == 1);
  xmlNs *ns = (xmlNs *)result->nodesetval->nodeTab[0];
  XML_CHECK(ns && ns->type == 18);
  XML_CHECK(ns->href && !strcmp((const char *)ns->href, "urn:ios"));
  XML_CHECK(ns->prefix && !strcmp((const char *)ns->prefix, "sdk"));
  XML_CHECK(ns->next == root);
  xmlXPathFreeObject(result);
  XML_CHECK(root->doc == doc && root->type == 1);
  xmlXPathFreeContext(context);
  xmlFreeDoc(doc);
  return 0;
}
