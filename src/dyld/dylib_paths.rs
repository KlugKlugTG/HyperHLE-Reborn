/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Resolve Mach-O install names using each loading image's run-path chain.
//!
//! https://developer.apple.com/library/archive/documentation/DeveloperTools/Conceptual/DynamicLibraries/100-Articles/RunpathDependentLibraries.html

use crate::fs::{resolve_path, GuestPath, GuestPathBuf};

fn expand_image_path(path: &str, loader: &GuestPath, executable: &GuestPath) -> String {
    let loader_directory = loader.parent().unwrap();
    let executable_directory = executable.parent().unwrap();
    let expanded = if path == "@loader_path" {
        loader_directory.to_owned()
    } else if let Some(suffix) = path.strip_prefix("@loader_path/") {
        loader_directory.join(suffix)
    } else if path == "@executable_path" {
        executable_directory.to_owned()
    } else if let Some(suffix) = path.strip_prefix("@executable_path/") {
        executable_directory.join(suffix)
    } else {
        GuestPathBuf::from(path.to_string())
    };
    format!(
        "/{}",
        resolve_path(&expanded, Some(executable_directory)).join("/")
    )
}

pub(crate) fn expand_run_paths(
    paths: &[String],
    loader: &GuestPath,
    executable: &GuestPath,
    inherited: &[String],
) -> Vec<String> {
    let mut result = Vec::new();
    for path in paths {
        let expanded = expand_image_path(path, loader, executable);
        if !result.contains(&expanded) {
            result.push(expanded);
        }
    }
    for path in inherited {
        if !result.contains(path) {
            result.push(path.clone());
        }
    }
    result
}

pub(crate) fn resolve_install_name(
    name: &str,
    loader: &GuestPath,
    executable: &GuestPath,
    run_paths: &[String],
    mut find_file: impl FnMut(&GuestPath) -> Option<GuestPathBuf>,
) -> String {
    if let Some(suffix) = name.strip_prefix("@rpath/") {
        for run_path in run_paths {
            let candidate = GuestPath::new(run_path).join(suffix);
            if let Some(path) = find_file(&candidate) {
                return path.as_str().to_owned();
            }
        }
    } else {
        let candidate = expand_image_path(name, loader, executable);
        if let Some(path) = find_file(GuestPath::new(&candidate)) {
            return path.as_str().to_owned();
        }
    }
    // Keep unresolved system install names available to the HLE linker.
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpath_search_uses_link_order_and_skips_missing_candidates() {
        let executable = GuestPath::new("/Apps/Game.app/main");
        let paths = expand_run_paths(
            &[
                "/usr/lib/swift".into(),
                "@executable_path/Frameworks".into(),
            ],
            executable,
            executable,
            &[],
        );
        let mut searched = Vec::new();
        let resolved = resolve_install_name(
            "@rpath/libswiftCore.dylib",
            executable,
            executable,
            &paths,
            |path| {
                searched.push(path.as_str().to_owned());
                (path.as_str() == "/Apps/Game.app/Frameworks/libswiftCore.dylib")
                    .then(|| path.to_owned())
            },
        );
        assert_eq!(resolved, "/Apps/Game.app/Frameworks/libswiftCore.dylib");
        assert_eq!(
            searched,
            [
                "/usr/lib/swift/libswiftCore.dylib",
                "/Apps/Game.app/Frameworks/libswiftCore.dylib"
            ]
        );
    }

    #[test]
    fn transitive_rpaths_preserve_the_image_that_declared_loader_path() {
        let executable = GuestPath::new("/Apps/Game.app/main");
        let parent = GuestPath::new("/Apps/Game.app/Frameworks/Parent.framework/Parent");
        let child = GuestPath::new("/Apps/Game.app/Frameworks/Child.framework/Child");
        let inherited =
            expand_run_paths(&["@loader_path/../Shared".into()], parent, executable, &[]);
        let paths = expand_run_paths(
            &["@loader_path/Helpers".into()],
            child,
            executable,
            &inherited,
        );
        assert_eq!(
            paths,
            [
                "/Apps/Game.app/Frameworks/Child.framework/Helpers",
                "/Apps/Game.app/Frameworks/Shared"
            ]
        );
        assert_eq!(
            resolve_install_name("@rpath/Support.dylib", child, executable, &paths, |path| {
                (path.as_str() == "/Apps/Game.app/Frameworks/Shared/Support.dylib")
                    .then(|| path.to_owned())
            }),
            "/Apps/Game.app/Frameworks/Shared/Support.dylib"
        );
    }

    #[test]
    fn loader_and_executable_relative_install_names_are_distinct() {
        let executable = GuestPath::new("/Apps/Game.app/main");
        let loader = GuestPath::new("/Apps/Game.app/Frameworks/Foo.framework/Foo");
        for (name, expected) in [
            (
                "@loader_path/Helpers/Bar",
                "/Apps/Game.app/Frameworks/Foo.framework/Helpers/Bar",
            ),
            (
                "@executable_path/Frameworks/Bar",
                "/Apps/Game.app/Frameworks/Bar",
            ),
        ] {
            assert_eq!(
                resolve_install_name(name, loader, executable, &[], |path| Some(path.to_owned())),
                expected
            );
        }
        assert_eq!(
            resolve_install_name("@rpath/Missing", loader, executable, &[], |_| None),
            "@rpath/Missing"
        );
    }
}
