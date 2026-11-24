/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

// This file contains the entry-point for TestApp, which is used for integration
// testing of touchHLE. Depending on the passed arguments, it will run as a
// command-line app that runs automated tests (see cli_tests.m) or as a UIKit
// app that is used for manual interactive testing (see gui_tests.m).
// See also tests/README.md and tests/integration.rs for the details of how it
// is compiled and run.

#include <stdio.h>
#include <string.h>

int TestApp_cli_tests_main(void);
int test_GLKView_drawable_graphics(void);
int TestApp_gui_tests_main(int argc, char **argv);
void TestApp_uncaught_objc_exception_probe(void);
int TestApp_status_bar_launch_probe(int argc, char **argv, int hidden);
void objc_terminate(void);

int main(int argc, char **argv) {
  if (argc == 2 && !strcmp(argv[1], "--trap-probe")) {
    printf("Entering intentional undefined-instruction probe.\n");
    fflush(stdout);
    __builtin_trap();
  } else if (argc == 2 && !strcmp(argv[1], "--breakpoint-probe")) {
    printf("Entering intentional breakpoint probe.\n");
    fflush(stdout);
    __asm__ volatile("bkpt #0");
    return 0;
  } else if (argc == 2 && !strcmp(argv[1], "--objc-exception-probe")) {
    TestApp_uncaught_objc_exception_probe();
  } else if (argc == 2 && !strcmp(argv[1], "--objc-terminate-probe")) {
    objc_terminate();
    __builtin_trap();
  } else if (argc == 2 && !strcmp(argv[1], "--glkit-drawable-probe")) {
    return test_GLKView_drawable_graphics();
  } else if (argc == 2 && !strcmp(argv[1], "--status-bar-hidden-probe")) {
    return TestApp_status_bar_launch_probe(argc, argv, 1);
  } else if (argc == 2 && !strcmp(argv[1], "--status-bar-visible-probe")) {
    return TestApp_status_bar_launch_probe(argc, argv, 0);
  } else if (argc == 2 && !strcmp(argv[1], "--cli-tests")) {
    printf("Starting command-line automated tests (omit --cli-tests for UIKit "
           "test app).\n");
    return TestApp_cli_tests_main();
  } else if (argc == 1) {
    printf("Running UIKit test app (pass --cli-tests for other tests).\n");
    return TestApp_gui_tests_main(argc, argv);
  } else {
    printf("Invalid usage!");
    return -1;
  }
}
