/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! GLSL ES 1.00 → desktop GLSL 1.20 source translator.
//!
//! touchHLE's OpenGL ES 2.0 layer is implemented on top of OpenGL 2.1, which
//! does not understand the GLSL ES dialect that ES 2.0 apps ship. The
//! translation is intentionally minimal — just enough to make the test corpus
//! we have at hand compile:
//!
//! 1. Insert/replace a `#version 120` directive at the top.
//! 2. Drop top-level precision declarations (`precision lowp float;` etc.).
//! 3. Strip inline precision qualifiers (`lowp`/`mediump`/`highp`) from the
//!    rest of the source.
//!
//! This is *not* a full GLSL translator. Apps that use ES-only features (like
//! `gl_FragColor` is fine; built-in attribute names; integer textures; …) are
//! not handled. We can extend this when needed.

/// Patch legacy ES 2.0 shadow-sampler shaders for the capabilities of the host driver.
/// The extension is fragment-only; some apps require it in their shared vertex prefix.
/// On hosts without it, emulate comparison sampling with an ordinary depth-texture sample.
/// <https://registry.khronos.org/OpenGL/extensions/EXT/EXT_shadow_samplers.txt>
pub fn patch_shadow_samplers_extension(
    source: &str,
    is_vertex_shader: bool,
    shadow_samplers_ext_supported: bool,
) -> String {
    let mut output = String::with_capacity(source.len() + 160);
    for line in source.split_inclusive('\n') {
        let line_without_comment = line.split("//").next().unwrap_or(line);
        let directive = line_without_comment
            .trim_start()
            .strip_prefix('#')
            .map(str::trim_start);
        let extension_action = directive.and_then(|directive| {
            let rest = directive.strip_prefix("extension")?;
            let keyword_boundary = rest
                .chars()
                .next()
                .map_or(true, |c| !c.is_ascii_alphanumeric() && c != '_');
            if !keyword_boundary {
                return None;
            }
            let mut parts = rest.trim_start().split_whitespace();
            if parts.next()? != "GL_EXT_shadow_samplers" {
                return None;
            }
            Some(parts.last().unwrap_or(""))
        });

        if extension_action.is_some() && (is_vertex_shader || !shadow_samplers_ext_supported) {
            if line.ends_with("\r\n") {
                output.push_str("\r\n");
            } else if line.ends_with('\n') {
                output.push('\n');
            }
        } else {
            output.push_str(line);
        }
    }

    if is_vertex_shader || shadow_samplers_ext_supported || !output.contains("shadow2DEXT") {
        return output;
    }

    let sample_function = if output
        .lines()
        .any(|line| line.trim_start().starts_with("#version 300 es"))
    {
        "texture"
    } else {
        "texture2D"
    };
    let mut output = output
        .replace("sampler2DShadow", "sampler2D")
        .replace("shadow2DEXT", "touchHLE_shadow2DEXT");
    let helper = format!(
        "highp float touchHLE_shadow2DEXT(highp sampler2D shadow_map, highp vec3 shadow_coord) {{\n    highp float depth = {sample_function}(shadow_map, shadow_coord.xy).r;\n    return step(shadow_coord.z, depth);\n}}\n\n"
    );
    let mut offset = 0;
    let mut insertion_point = None;
    for line in output.split_inclusive('\n') {
        if line.trim() == "#define FRAGMENT" {
            insertion_point = Some(offset + line.len());
            break;
        }
        offset += line.len();
    }
    let insertion_point = insertion_point.or_else(|| {
        output.find("void main").map(|main_start| {
            output[..main_start]
                .rfind('\n')
                .map_or(0, |newline| newline + 1)
        })
    });
    if let Some(insertion_point) = insertion_point {
        output.insert_str(insertion_point, &helper);
    }
    output
}

/// Translate ES shaders to the desktop core dialect. Legacy ES interfaces
/// and texture functions were removed from the desktop core profile.
pub fn translate_glsl_es_to_330(source: &str, vertex: bool) -> String {
    let mut body = String::with_capacity(source.len());
    let mut preamble_end = 0;
    let mut conditional_depth = 0_u32;
    let mut in_preamble = true;
    let mut source_version = "100";
    let mut output_name = "touchHLE_fragOutput".to_owned();
    while source.contains(&output_name) {
        output_name.push('_');
    }

    let mut fragment_color = false;
    let mut fragment_data = false;
    let mut block_comment = false;
    for line in source.lines() {
        // Skip only promoted extensions. Keep other directives in their
        // original conditional scope and let the host compiler handle them.
        if !block_comment {
            if let Some(directive) = line.trim_start().strip_prefix('#') {
                let mut words = directive
                    .split(|c: char| c.is_ascii_whitespace() || c == ':')
                    .filter(|word| !word.is_empty());
                match words.next() {
                    Some("version") => {
                        if words.next() == Some("300") {
                            source_version = "300";
                        }
                        body.push('\n');
                        continue;
                    }
                    Some("extension")
                        if matches!(
                            words.next(),
                            Some(
                                "GL_OES_standard_derivatives"
                                    | "GL_EXT_shader_texture_lod"
                                    | "GL_OES_texture_3D"
                                    | "GL_EXT_draw_buffers"
                                    | "GL_EXT_frag_depth"
                                    | "GL_EXT_shadow_samplers"
                            )
                        ) =>
                    {
                        body.push('\n');
                        continue;
                    }
                    _ => {}
                }
            }
        }

        if in_preamble {
            let t = line.trim_start();
            if let Some(directive) = t.strip_prefix('#') {
                match directive.split_whitespace().next() {
                    Some("if" | "ifdef" | "ifndef") => conditional_depth += 1,
                    Some("endif") => conditional_depth = conditional_depth.saturating_sub(1),
                    _ => {}
                }
            } else if !(block_comment || t.is_empty() || t.starts_with("//") || t.starts_with("/*"))
            {
                in_preamble = false;
            }
        }
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < line.len() {
            let rest = &line[i..];
            if block_comment || rest.starts_with("/*") {
                if let Some(end) = rest.find("*/") {
                    body.push_str(&rest[..end + 2]);
                    i += end + 2;
                    block_comment = false;
                    continue;
                }
                body.push_str(rest);
                block_comment = true;
                break;
            }
            if rest.starts_with("//") {
                body.push_str(rest);
                break;
            }
            if !is_ident_char(bytes[i]) {
                let c = rest.chars().next().unwrap();
                body.push(c);
                i += c.len_utf8();
                continue;
            }
            let start = i;
            while i < bytes.len() && is_ident_char(bytes[i]) {
                i += 1;
            }
            let token = &line[start..i];
            let replacement = match token {
                "attribute" => "in",
                "varying" if vertex => "out",
                "varying" => "in",
                "texture2D" | "textureCube" | "texture3D" | "texture3DOES" | "shadow2DEXT" => {
                    "texture"
                }
                "texture2DProj" | "texture3DProj" | "texture3DProjOES" | "shadow2DProjEXT" => {
                    "textureProj"
                }
                "texture2DLod" | "textureCubeLod" | "texture2DLodEXT" | "textureCubeLodEXT" => {
                    "textureLod"
                }
                "texture2DProjLod" | "texture2DProjLodEXT" => "textureProjLod",
                "texture2DGradEXT" | "textureCubeGradEXT" => "textureGrad",
                "texture2DProjGradEXT" => "textureProjGrad",
                "gl_FragDepthEXT" => "gl_FragDepth",
                "gl_FragColor" if !vertex => {
                    fragment_color = true;
                    body.push_str(&output_name);
                    "[0]"
                }
                "gl_FragData" if !vertex => {
                    fragment_data = true;
                    &output_name
                }
                // Guest preprocessors commonly guard stage precision and
                // version-specific interfaces with these ES built-ins.
                "GL_ES" => "touchHLE_GLSL_ES",
                "__VERSION__" => "touchHLE_ES_VERSION",
                "float16_t" => "float",
                "f16vec2" => "vec2",
                "f16vec3" => "vec3",
                "f16vec4" => "vec4",
                "f16mat2" => "mat2",
                "f16mat3" => "mat3",
                "f16mat4" => "mat4",
                _ => token,
            };
            body.push_str(replacement);
        }
        body.push('\n');
        if in_preamble && conditional_depth == 0 {
            preamble_end = body.len();
        }
    }
    let mut translated = format!(
        "#version 330 core\n#define touchHLE_GLSL_ES 1\n#define touchHLE_ES_VERSION {source_version}\n"
    );
    translated.push_str(&body[..preamble_end]);
    if fragment_color || fragment_data {
        let extent = if fragment_data {
            "gl_MaxDrawBuffers"
        } else {
            "1"
        };
        translated.push_str(&format!(
            "layout(location = 0) out vec4 {output_name}[{extent}];\n"
        ));
    }
    translated.push_str(&body[preamble_end..]);
    translated
}

/// Translate legacy ES source for a GL 2.1 compatibility context.
pub fn translate_glsl_es_to_120(source: &str) -> String {
    let mut out = String::with_capacity(source.len() + 32);
    let mut emitted_version = false;

    // Collect #extension directives to hoist them right after #version.
    // Desktop GLSL requires #extension before any non-preprocessor tokens.
    let mut extension_lines: Vec<String> = Vec::new();
    let mut body_lines: Vec<String> = Vec::new();

    for raw_line in source.lines() {
        let trimmed = raw_line.trim_start();

        if !emitted_version {
            if trimmed.starts_with("#version") {
                emitted_version = true;
                continue; // We'll emit our own version directive
            } else if !trimmed.is_empty() && !trimmed.starts_with("//") && !trimmed.starts_with('#')
            {
                emitted_version = true;
            }
        }

        // Collect #extension directives separately so we can hoist them.
        if trimmed.starts_with("#extension") {
            // Strip GL_EXT_shader_texture_lod extension — desktop GLSL 1.20/3.30
            // doesn't have this extension (texture2DLod is built-in in 1.30+).
            if trimmed.contains("GL_EXT_shader_texture_lod") {
                continue;
            }
            extension_lines.push(raw_line.to_string());
            continue;
        }

        // Drop standalone "precision <qual> <type>;" lines.
        if trimmed.starts_with("precision") {
            let rest = trimmed["precision".len()..].trim_start();
            if rest.starts_with("lowp") || rest.starts_with("mediump") || rest.starts_with("highp")
            {
                continue;
            }
        }

        let stripped = strip_precision_qualifiers(raw_line);
        let stripped = strip_half_types(&stripped);
        body_lines.push(stripped);
    }

    // Emit version directive first.
    out.push_str("#version 120\n");

    // Emit hoisted #extension lines right after version.
    for ext in &extension_lines {
        out.push_str(ext);
        out.push('\n');
    }

    // Emit body.
    for line in &body_lines {
        out.push_str(line);
        out.push('\n');
    }

    // ES 1.00 fragment shaders may write to `gl_FragData[n]`. Desktop GLSL
    // 1.20 has no `gl_FragData` built-in (GL 2.1 has a single color
    // attachment), so map `gl_FragData[0]` to `gl_FragColor` and treat any
    // other index as a write to `gl_FragColor` too — the extra attachments
    // do not exist and their contents are never read back by our
    // single-buffer surface model.
    out = translate_frag_data(&out);

    replace_texture_lod_ext_desktop_120(&out)
}

/// In desktop GLSL 1.20, `texture2DLod` is available as a built-in (via
/// GL_ARB_shader_texture_lod which is part of OpenGL 2.1 core). Replace the
/// EXT-suffixed names with the unsuffixed equivalents.
fn replace_texture_lod_ext_desktop_120(source: &str) -> String {
    source
        .replace("texture2DLodEXT", "texture2DLod")
        .replace("texture2DProjLodEXT", "texture2DProjLod")
        .replace("textureCubeLodEXT", "textureCubeLod")
        .replace("texture2DGradEXT", "texture2DGrad")
}

/// Strip occurrences of `lowp`, `mediump`, and `highp` from a line, while
/// preserving identifiers that merely contain those substrings.
fn strip_precision_qualifiers(line: &str) -> String {
    const QUALIFIERS: &[&str] = &["lowp", "mediump", "highp"];

    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let is_word_start = c.is_ascii_alphabetic() || c == b'_';
        if is_word_start && (i == 0 || !is_ident_char(bytes[i - 1])) {
            let mut matched = None;
            for q in QUALIFIERS {
                let qb = q.as_bytes();
                if i + qb.len() <= bytes.len()
                    && &bytes[i..i + qb.len()] == qb
                    && (i + qb.len() == bytes.len() || !is_ident_char(bytes[i + qb.len()]))
                {
                    matched = Some(qb.len());
                    break;
                }
            }
            if let Some(qlen) = matched {
                // Skip the qualifier and any following whitespace, but keep at
                // least one space if there was one before, to preserve
                // separation between tokens.
                let pre_was_space = !out.is_empty() && out.as_bytes()[out.len() - 1] == b' ';
                let mut j = i + qlen;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                    j += 1;
                }
                if !pre_was_space && j < bytes.len() {
                    out.push(' ');
                }
                i = j;
                continue;
            }
        }
        out.push(c as char);
        i += 1;
    }
    out
}
fn strip_half_types(line: &str) -> String {
    line.replace("f16mat4", "mat4")
        .replace("f16mat3", "mat3")
        .replace("f16mat2", "mat2")
        .replace("f16vec4", "vec4")
        .replace("f16vec3", "vec3")
        .replace("f16vec2", "vec2")
        .replace("float16_t", "float")
}

/// Rewrite `gl_FragData[n]` references to legal desktop-GLSL equivalents.
///
/// - `gl_FragData[0]` is exactly `gl_FragColor` in ES 1.00 semantics.
/// - Higher indices only exist with GL_EXT_draw_buffers, which a single-
///   attachment desktop context cannot honor; those writes become plain
///   `gl_FragColor`/`out` writes (the last write wins, which matches the
///   behavior guests observe on single-buffer surfaces).
/// - A non-constant index (e.g. `gl_FragData[i]`) is left untouched; that is
///   invalid ES usage anyway and the driver will reject it with a clear
///   error rather than us guessing.
fn translate_frag_data(source: &str) -> String {
    if !source.contains("gl_FragData") {
        return source.to_string();
    }
    let bytes = source.as_bytes();
    let needle = b"gl_FragData";
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(needle) {
            let mut k = i + needle.len();
            while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                k += 1;
            }
            if k < bytes.len() && bytes[k] == b'[' {
                let mut m = k + 1;
                while m < bytes.len() && (bytes[m] == b' ' || bytes[m] == b'\t') {
                    m += 1;
                }
                let idx_start = m;
                while m < bytes.len() && bytes[m].is_ascii_digit() {
                    m += 1;
                }
                // Skip trailing whitespace and expect ']'.
                while m < bytes.len() && (bytes[m] == b' ' || bytes[m] == b'\t') {
                    m += 1;
                }
                if m < bytes.len() && bytes[m] == b']' {
                    let idx: Option<u32> = source[i + needle.len()..k]
                        .trim()
                        .parse()
                        .ok()
                        .or_else(|| source[i + needle.len()..m].trim().parse().ok());
                    let idx = idx.unwrap_or(u32::MAX);
                    if idx == 0 {
                        out.push_str("gl_FragColor");
                    } else {
                        // Attachments > 0 do not exist on the desktop target;
                        // keep the expression syntactically valid by routing
                        // the write to the primary color output.
                        out.push_str("gl_FragColor");
                    }
                    i = m + 1;
                    continue;
                }
            }
            out.push_str("gl_FragData");
            i += needle.len();
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_stages_use_matching_interfaces_and_keep_distinct_color_outputs() {
        // https://registry.khronos.org/OpenGL/specs/gl/GLSLangSpec.3.30.pdf
        let v = translate_glsl_es_to_330(
            "#version 100\nattribute vec4 pos; varying vec2 uv; // varying unchanged\n",
            true,
        );
        assert!(v.contains("in vec4 pos; out vec2 uv; // varying unchanged"));
        let f = translate_glsl_es_to_330(
            "#extension GL_EXT_draw_buffers : require\nvarying vec2 uv;\nvoid main() { gl_FragData[1] = texture2D(s, uv); }", false);
        assert!(f.starts_with("#version 330 core\n"));
        assert!(f.contains("out vec4 touchHLE_fragOutput[gl_MaxDrawBuffers]"));
        assert!(f.contains("in vec2 uv;"));
        assert!(f.contains("touchHLE_fragOutput[1] = texture(s, uv)"));
        assert!(!f.contains("#extension GL_EXT_draw_buffers"));
    }

    #[test]
    fn core_preserves_version_guards_and_es3_declared_outputs() {
        let f = translate_glsl_es_to_330(
            "// header\n#version 300 es\n#if defined(GL_ES) && __VERSION__ >= 300\nlayout(location=0) out vec4 color;\n#endif", false);
        assert!(f.contains("#define touchHLE_ES_VERSION 300"));
        assert!(f.contains("#if defined(touchHLE_GLSL_ES) && touchHLE_ES_VERSION >= 300"));
        assert!(f.contains("layout(location=0) out vec4 color;"));
        assert!(!f.contains("touchHLE_fragOutput"));
    }

    #[test]
    fn patches_shadow_sampler_extension_by_shader_stage() {
        let src = "#version 100\n#  extension GL_EXT_shadow_samplers : require\nvoid main() {}\n";
        let vertex = patch_shadow_samplers_extension(src, true, false);
        assert!(!vertex.contains("GL_EXT_shadow_samplers"));
        assert_eq!(vertex, "#version 100\n\nvoid main() {}\n");

        let supported_fragment = patch_shadow_samplers_extension(src, false, true);
        assert!(supported_fragment.contains("#  extension GL_EXT_shadow_samplers : require"));
    }

    #[test]
    fn emulates_shadow_sampler_when_the_host_lacks_the_extension() {
        let src = "#version 100\n#define FRAGMENT\n#extension GL_EXT_shadow_samplers : require\nprecision highp float;\nuniform highp sampler2DShadow u_shadowMap;\nvoid main() { highp float result = shadow2DEXT(u_shadowMap, vec3(0.5)); gl_FragColor = vec4(result); }\n";
        let patched = patch_shadow_samplers_extension(src, false, false);
        assert!(!patched.contains("GL_EXT_shadow_samplers"));
        assert!(!patched.contains("sampler2DShadow"));
        assert!(patched.contains("uniform highp sampler2D u_shadowMap;"));
        assert!(patched.contains("touchHLE_shadow2DEXT(u_shadowMap, vec3(0.5))"));
        assert!(patched.contains("texture2D(shadow_map, shadow_coord.xy).r"));
        assert!(patched.contains("step(shadow_coord.z, depth)"));
        assert!(
            patched.find("highp float touchHLE_shadow2DEXT").unwrap()
                < patched.find("void main").unwrap()
        );
    }

    #[test]
    fn rewrites_version_directive() {
        let src = "#version 100\nvoid main() {}\n";
        let out = translate_glsl_es_to_120(src);
        assert!(out.starts_with("#version 120\n"));
        assert!(out.contains("void main()"));
    }

    #[test]
    fn strips_precision_lines() {
        let src = "#version 100\nprecision mediump float;\nprecision highp int;\nvoid main(){}\n";
        let out = translate_glsl_es_to_120(src);
        assert!(!out.contains("precision mediump"));
        assert!(!out.contains("precision highp"));
    }

    #[test]
    fn strips_inline_qualifiers() {
        let src = "#version 100\nvarying lowp vec4 DestinationColor;\n";
        let out = translate_glsl_es_to_120(src);
        assert!(out.contains("varying vec4 DestinationColor;"));
        assert!(!out.contains("lowp"));
    }

    #[test]
    fn preserves_non_qualifier_identifiers() {
        let src = "#version 100\nuniform float highpassCutoff;\n";
        let out = translate_glsl_es_to_120(src);
        assert!(out.contains("highpassCutoff"));
    }

    #[test]
    fn hoists_extension_directives() {
        let src = "#version 100\nvoid foo() {}\n#extension GL_OES_standard_derivatives : enable\nvoid main(){}\n";
        let out = translate_glsl_es_to_120(src);
        // Extension should come right after #version, before any code
        let version_pos = out.find("#version 120").unwrap();
        let ext_pos = out.find("#extension GL_OES_standard_derivatives").unwrap();
        let foo_pos = out.find("void foo()").unwrap();
        assert!(ext_pos < foo_pos);
        assert!(ext_pos > version_pos);
    }

    #[test]
    fn strips_texture_lod_ext_extension_and_replaces_calls() {
        let src = "#version 100\n#extension GL_EXT_shader_texture_lod : enable\nvoid main(){ gl_FragColor = texture2DLodEXT(tex, uv, 0.0); }\n";
        let out = translate_glsl_es_to_120(src);
        assert!(!out.contains("GL_EXT_shader_texture_lod"));
        assert!(!out.contains("texture2DLodEXT"));
        assert!(out.contains("texture2DLod"));
    }
}
