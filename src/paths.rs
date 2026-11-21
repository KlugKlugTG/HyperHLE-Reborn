/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Paths for host files used by touchHLE: settings, fonts, etc.
//!
//! There are three categories of files:
//!
//! * Resources bundled with touchHLE that neither touchHLE nor the user should
//!   modify: [DYLIBS_DIR], [FONTS_DIR], [DEFAULT_OPTIONS_FILE]. Depending on
//!   the platform these may or may not be ordinary files, and must be accessed
//!   through [ResourceFile].
//! * Files the user is expected to modify, but not touchHLE: [APPS_DIR],
//!   [USER_OPTIONS_FILE], [WALLPAPER_FILES]. These are ordinary files and are
//!   found in [user_data_base_path].
//! * Files that touchHLE will create and modify, and the user may modify if
//!   they want to: [SANDBOX_DIR], [PHOTO_ALBUM_DIR].
//!   These are ordinary files and are found in [user_data_base_path].
//!
//! See also [crate::fs], which provides a virtual filesystem for the guest app
//! and defines path types.

use std::borrow::Cow;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

/// Name of the directory containing ARMv6 dynamic libraries bundled with
/// touchHLE.
pub const DYLIBS_DIR: &str = "touchHLE_dylibs";

/// Name of the directory containing fonts bundled with touchHLE.
pub const FONTS_DIR: &str = "touchHLE_fonts";

/// Name of the file containing touchHLE's default options for various apps.
pub const DEFAULT_OPTIONS_FILE: &str = "touchHLE_default_options.txt";

/// The default options file to actually read.
///
/// A host that ships more than one emulator core needs one file per core: the
/// cores understand different options, and an option a core does not recognise
/// aborts the launch. The iOS host sets `TOUCHHLE_DEFAULT_OPTIONS_FILE` to the
/// file belonging to the core it loaded.
pub fn default_options_file() -> String {
    std::env::var("TOUCHHLE_DEFAULT_OPTIONS_FILE")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| DEFAULT_OPTIONS_FILE.to_string())
}

/// On Apple hosts, if touchHLE is located in an app bundle, return its resource
/// path. If touchHLE is not located in an app bundle, return
/// [None].
#[allow(dead_code)]
fn get_bundled_resources_path() -> Option<PathBuf> {
    if !matches!(std::env::consts::OS, "ios" | "macos") {
        return None;
    }
    let base_path = PathBuf::from(sdl2::filesystem::base_path().ok()?);
    if std::env::consts::OS == "ios" || base_path.file_name().is_some_and(|p| p == "Resources") {
        Some(base_path)
    } else {
        None
    }
}

/// Abstraction over a platform-specific type for accessing a resource bundled
/// with touchHLE.
pub struct ResourceFile {
    #[cfg(target_os = "android")]
    file: sdl2::rwops::RWops<'static>,
    #[cfg(not(target_os = "android"))]
    file: std::fs::File,
}
impl ResourceFile {
    pub fn open(path: &str) -> Result<Self, String> {
        Ok(Self {
            // On Android, these resources are included as "assets" within the
            // APK. We access them via SDL2's wrapper of Android's assets API.
            #[cfg(target_os = "android")]
            file: sdl2::rwops::RWops::from_file(path, "r")?,

            // On other OSes, resources are accessed as ordinary files.
            #[cfg(not(target_os = "android"))]
            file: {
                let base_path = get_bundled_resources_path();
                // When not in a bundle, look in the current directory.
                let path = base_path.as_deref().unwrap_or(Path::new(".")).join(path);
                std::fs::File::open(path).map_err(|e| e.to_string())?
            },
        })
    }
    pub fn get(&mut self) -> &mut (impl Read + Seek) {
        &mut self.file
    }
}
impl std::fmt::Debug for ResourceFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "ResourceFile")
    }
}

/// Whether various resources are in user-accessible files. If they aren't,
/// touchHLE has to be able to display their license terms.
pub const RESOURCES_ARE_EXTERNAL_FILES: bool =
    cfg!(not(any(target_os = "android", target_os = "ios")));

/// Name of the directory where the user can put apps if they want them to
/// appear in the app picker.
pub const APPS_DIR: &str = "HyperHLE_apps";

/// Name of the file intended for the user's own options.
pub const USER_OPTIONS_FILE: &str = "HyperHLE_options.txt";

/// Name of the log file written by HyperHLE.
pub const LOG_FILE: &str = "HyperHLE_log.txt";

/// Persistent preferences for the graphical app picker.
pub const PICKER_SETTINGS_FILE: &str = "HyperHLE_picker_settings.txt";

/// Name of the file recording the most recently launched app.
pub const LAST_APP_FILE: &str = "HyperHLE_last_app.txt";

/// Names of files the user can put a wallpaper image (for the app picker) in.
#[allow(unused)]
pub const WALLPAPER_FILES: &[&str] = &[
    "HyperHLE_wallpaper.png",
    "HyperHLE_wallpaper.jpg",
    "HyperHLE_wallpaper.jpeg",
];

/// Name of the bundled default wallpaper resource, copied into the user
/// directory on first run. This is the file baked into the app bundle; it is
/// intentionally not the same as [WALLPAPER_FILES].
pub const BUNDLED_WALLPAPER_FILE: &str = "touchHLE_wallpaper.png";

/// Name of the directory where touchHLE will store sandboxed app data, e.g.
/// the `Documents` directory.
pub const SANDBOX_DIR: &str = "HyperHLE_sandbox";

/// Name of the directory where redirected/exported SQLite databases live.
pub const SQLITE_DIR: &str = "HyperHLE_sqlite";

/// Name of the directory where touchHLE will store IMG_####.PNG files saved to
/// the Photo Album.
pub const PHOTO_ALBUM_DIR: &str = "DCIM/100APPLE";

/// Get a platform-specific base path needed for accessing touchHLE's
/// user-modifiable files. This is empty on platforms other than Android.
pub fn user_data_base_path() -> Cow<'static, Path> {
    #[cfg(target_os = "android")]
    unsafe {
        // This is an exception to the rule that SDL2 should only be used
        // directly from src/window.rs. This is just too distant from windowing
        // to belong there.

        // Android storage has evolved in a quite messy fashion. Both "internal
        // storage" and "external storage" (aka the "SD card") are likely to be
        // internal on a modern device, as absurd as that might sound. SDL2 has
        // APIs to get paths for both. We use the "external storage" because
        // it's more likely to be user-accessible.
        extern "C" {
            fn SDL_AndroidGetExternalStoragePath() -> *const std::ffi::c_char;
        }
        let path = SDL_AndroidGetExternalStoragePath();
        if path.is_null() {
            log!("Couldn't get Android external storage path!");
            panic!();
        }
        Cow::from(Path::new(std::ffi::CStr::from_ptr(path).to_str().unwrap()))
    }
    #[cfg(not(target_os = "android"))]
    {
        if std::env::consts::OS == "ios" {
            let home = std::env::var_os("HOME").expect("iOS app container has no HOME path");
            return Cow::from(PathBuf::from(home).join("Documents"));
        }

        // When touchHLE is run from a .app bundle on macOS, the user might not
        // be able to control the current directory, so user data needs to go in
        // a standard location.
        if get_bundled_resources_path().is_some() {
            return Cow::from(PathBuf::from(
                sdl2::filesystem::pref_path("touchhle.org", "touchHLE").unwrap(),
            ));
        }
        Cow::from(Path::new("."))
    }
}

/// The Android application id, as set by `applicationId` in
/// `android/app/build.gradle`. The documents provider is registered with
/// `${applicationId}.provider` as its authority, so this must be kept in sync
/// with the Gradle file (only the branding suffix is appended separately).
const ANDROID_APPLICATION_ID: &str = "org.hyperhle.reborn";

/// The authority of the documents provider that exposes the user data
/// directory to the system file manager on Android. A build with branding
/// (e.g. "UNOFFICIAL") gets `applicationIdSuffix`ed, so the authority carries
/// the lowercased branding too.
fn android_documents_authority() -> String {
    let branding = crate::branding();
    if branding.is_empty() {
        format!("{ANDROID_APPLICATION_ID}.provider")
    } else {
        format!(
            "{ANDROID_APPLICATION_ID}.{}.provider",
            branding.to_lowercase()
        )
    }
}

/// A `content://` URI for one document of the provider above, using the
/// `…/document/<document id>` form that `DocumentsContract` understands. The
/// document id is a single path segment, so the slashes in it (the provider
/// uses hierarchical ids like `root/<name>`) must be percent-encoded.
fn android_document_uri(document_id: &str) -> String {
    format!(
        "content://{}/document/{}",
        android_documents_authority(),
        document_id.replace('/', "%2F")
    )
}

fn path_to_file_url(path: &std::path::Path) -> Result<String, String> {
    // Canonicalize when possible, but don't require the directory to already
    // exist — the file manager button may be pressed before any app has been
    // added and the apps directory lazily created. Falling back to an
    // absolute non-canonical path is better than failing with
    // "Can't canonicalize".
    let abs_path = if path.exists() {
        path.canonicalize()
            .map_err(|e| format!("Can't canonicalize path {}: {e}", path.display()))?
    } else {
        // Build an absolute path without requiring existence.
        let cur = std::env::current_dir().map_err(|e| format!("Can't get current dir: {e}"))?;
        let joined = cur.join(path);
        // Try to canonicalize the parent if it exists, otherwise keep joined.
        if let Some(parent) = joined.parent().filter(|p| p.exists()) {
            if let Ok(canonical_parent) = parent.canonicalize() {
                if let Some(file_name) = joined.file_name() {
                    canonical_parent.join(file_name)
                } else {
                    canonical_parent
                }
            } else {
                joined
            }
        } else {
            joined
        }
    };
    // Ensure the directory exists so file managers have something to show.
    if !abs_path.exists() {
        let _ = std::fs::create_dir_all(&abs_path);
    }
    let path_str = abs_path
        .to_str()
        .ok_or_else(|| "Path is not UTF-8".to_string())?;
    let path_str = if std::env::consts::OS == "windows" {
        path_str.strip_prefix("\\\\?\\").unwrap_or(path_str)
    } else {
        path_str
    };
    Ok(format!("file://{path_str}"))
}

/// Get a URI that can be used to open a file manager or similar for the path
/// that [user_data_base_path] represents.
pub fn url_for_opening_user_data_dir() -> Result<String, String> {
    if std::env::consts::OS == "android" {
        // See DocumentsProvider.kt, app/build.gradle and AndroidManifest.xml
        Ok(format!(
            "content://{}/root/root",
            android_documents_authority()
        ))
    } else {
        path_to_file_url(&user_data_base_path().join("."))
    }
}

pub fn url_for_opening_apps_dir() -> Result<String, String> {
    let apps_dir = user_data_base_path().join(APPS_DIR);
    if std::env::consts::OS == "ios" {
        // The Files app's URL scheme; it cannot be pointed at a specific
        // folder, but the app's Documents directory is shown in "On My iPhone".
        Ok("shareddocuments://".to_string())
    } else if std::env::consts::OS == "android" {
        // Opening the apps folder directly, rather than the provider's root,
        // so the file manager starts where the games are.
        Ok(android_document_uri(&format!("root/{APPS_DIR}")))
    } else {
        path_to_file_url(&apps_dir)
    }
}

/// Legacy names for the user-facing artifacts that used to carry the
/// `touchHLE_` prefix, kept here so existing user data is copied forward on
/// the first run of the renamed build instead of being silently orphaned.
const LEGACY_APPS_DIR: &str = "touchHLE_apps";
const LEGACY_USER_OPTIONS_FILE: &str = "touchHLE_options.txt";
const LEGACY_PICKER_SETTINGS_FILE: &str = "touchHLE_picker_settings.txt";
const LEGACY_LAST_APP_FILE: &str = "touchHLE_last_app.txt";
const LEGACY_SANDBOX_DIR: &str = "touchHLE_sandbox";
const LEGACY_SQLITE_DIR: &str = "touchHLE_sqlite";
const LEGACY_WALLPAPER_FILES: &[&str] = &[
    "touchHLE_wallpaper.png",
    "touchHLE_wallpaper.jpg",
    "touchHLE_wallpaper.jpeg",
];

/// Recursively copy a file or directory tree. Returns an error if any part
/// fails, leaving the caller responsible for deciding what to do with the
/// incomplete copy.
fn copy_tree(from: &Path, to: &Path) -> Result<(), std::io::Error> {
    let meta = std::fs::metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        std::fs::copy(from, to)?;
    }
    Ok(())
}

/// Copy one legacy path to its renamed equivalent under the user data
/// directory, leaving the legacy path in place. This never deletes or
/// overwrites anything: if the renamed path already exists, both are left
/// untouched rather than merging.
fn migrate_one(base: &Path, from: &str, to: &str) {
    let old_path = base.join(from);
    let new_path = base.join(to);
    if !old_path.exists() {
        return;
    }
    if new_path.exists() {
        // Both exist: leave both untouched rather than merging or
        // overwriting. The user can clean up the legacy path by hand.
        return;
    }
    match copy_tree(&old_path, &new_path) {
        Ok(()) => log!(
            "Migrated legacy {} -> {}",
            old_path.display(),
            new_path.display()
        ),
        Err(e) => log!(
            "Warning: couldn't migrate {} -> {}: {}; the legacy path is still in place",
            old_path.display(),
            new_path.display(),
            e
        ),
    }
}

/// Copy user-facing `touchHLE_` artifacts to their `HyperHLE_` equivalents.
/// Called on startup before any of the paths is used; safe to call on every
/// launch. Existing renamed files are never created by this call, and the
/// legacy paths are never deleted.
pub fn migrate_legacy_user_data() {
    let base = user_data_base_path();
    // The log file is intentionally not migrated: it is truncated on every
    // launch, so copying it would lose its content anyway. The legacy log is
    // simply left in place.
    migrate_one(&base, LEGACY_APPS_DIR, APPS_DIR);
    migrate_one(&base, LEGACY_USER_OPTIONS_FILE, USER_OPTIONS_FILE);
    migrate_one(&base, LEGACY_PICKER_SETTINGS_FILE, PICKER_SETTINGS_FILE);
    migrate_one(&base, LEGACY_LAST_APP_FILE, LAST_APP_FILE);
    migrate_one(&base, LEGACY_SANDBOX_DIR, SANDBOX_DIR);
    migrate_one(&base, LEGACY_SQLITE_DIR, SQLITE_DIR);
    for (legacy, new) in LEGACY_WALLPAPER_FILES.iter().zip(WALLPAPER_FILES.iter()) {
        migrate_one(&base, legacy, new);
    }
}

/// Remove the legacy on-disk PVRTC decode cache directory. The PVRTC decode
/// cache was removed (both the in-memory tier and this on-disk tier) because
/// decoded RGBA8 entries used a lot of host memory and disk space.
pub fn remove_legacy_pvrtc_disk_cache() {
    let dir = user_data_base_path().join("touchHLE_pvrtc_cache");
    if dir.exists() {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => log!("Removed legacy PVRTC disk cache directory {}", dir.display()),
            Err(e) => log!(
                "Warning: couldn't remove legacy PVRTC disk cache {}: {}",
                dir.display(),
                e
            ),
        }
    }
}

/// Only meaningful on certain OSes: create the user data directory if it
/// doesn't exist, and populate it with templates or README files. (On other
/// platforms these are simply bundled with touchHLE in a ZIP file.)
pub fn prepopulate_user_data_dir() {
    if !matches!(std::env::consts::OS, "android" | "ios" | "macos") {
        return;
    }
    let base_path = user_data_base_path();
    if base_path == Path::new(".") {
        return;
    }

    let has_user_wallpaper = WALLPAPER_FILES
        .iter()
        .chain(LEGACY_WALLPAPER_FILES.iter())
        .any(|name| base_path.join(name).is_file());
    if !has_user_wallpaper {
        match ResourceFile::open(BUNDLED_WALLPAPER_FILE) {
            Ok(mut resource) => {
                let mut image = Vec::new();
                match resource.get().read_to_end(&mut image) {
                    Ok(_) => match std::fs::write(base_path.join(WALLPAPER_FILES[0]), image) {
                        Ok(()) => log!("Created default wallpaper"),
                        Err(e) => log!("Warning: Couldn't create default wallpaper: {}", e),
                    },
                    Err(e) => log!("Warning: Couldn't read default wallpaper: {}", e),
                }
            }
            Err(e) => log!("Warning: Couldn't open default wallpaper: {}", e),
        }
    }

    let apps_dir = base_path.join(APPS_DIR);
    if !apps_dir.is_dir() {
        match std::fs::create_dir(&apps_dir) {
            Ok(()) => {
                log!("Created: {}", apps_dir.display());
            }
            Err(e) => {
                log!("Warning: Couldn't create {}: {}", apps_dir.display(), e);
            }
        }
    }

    let sqlite_dir = base_path.join(SQLITE_DIR);
    if !sqlite_dir.is_dir() {
        match std::fs::create_dir(&sqlite_dir) {
            Ok(()) => {
                log!("Created: {}", sqlite_dir.display());
            }
            Err(e) => {
                log!("Warning: Couldn't create {}: {}", sqlite_dir.display(), e);
            }
        }
    }

    fn create_file(path: &Path, content: &str) {
        match std::fs::write(path, content) {
            Ok(()) => {
                log!("Created: {}", path.display());
            }
            Err(e) => {
                log!("Warning: Couldn't create {}: {}", path.display(), e);
            }
        }
    }

    let apps_dir_readme = apps_dir.join("README.txt");
    if !apps_dir_readme.is_file() {
        let content = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/HyperHLE_apps/README.txt"
        ));
        create_file(&apps_dir_readme, content);
    }

    let user_options = base_path.join(USER_OPTIONS_FILE);
    if !user_options.is_file() {
        let content = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/HyperHLE_options.txt"));
        create_file(&user_options, content);
    }

    let options_help = base_path.join("OPTIONS_HELP.txt");
    if !options_help.is_file() {
        create_file(&options_help, crate::options::OPTIONS_HELP);
    }
}
