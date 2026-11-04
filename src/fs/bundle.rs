/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! IPA file format support, allowing it to be used as part of the guest
//! filesystem.
use crate::fs::{FsNode, GuestPath};
use crate::libc::time::{calendar_date_to_timestamp, time_t, tm};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use zip::result::ZipError;
use zip::ZipArchive;

/// A helper struct to build an FsNode with files and directories coming in
/// arbitrary order. This is required, because ZIP files are allowed to store
/// entries in arbitrary order.
struct FsNodeBuilder {
    root: FsNode,
}

impl FsNodeBuilder {
    fn new() -> Self {
        Self {
            root: FsNode::dir(),
        }
    }

    fn find_or_make_directory(&mut self, path: &GuestPath) -> &mut FsNode {
        let mut current = &mut self.root;
        for part in path.as_str().split('/') {
            if part.is_empty() {
                continue;
            }
            assert_ne!(part, "..", "unexpected .. in path: {path:?}");
            let FsNode::Directory { children, .. } = current else {
                panic!("expected directory, got {current:?}");
            };

            let next = children.entry(part.to_string()).or_insert_with(FsNode::dir);
            current = next;
        }
        current
    }

    fn add_file(&mut self, path: &GuestPath, node: FsNode) {
        let (parent_name, file_name) = path.parent_and_file_name().unwrap();
        assert_ne!(file_name, "..", "unexpected .. in path: {path:?}");
        let dir = self.find_or_make_directory(parent_name);
        let FsNode::Directory { children, .. } = dir else {
            panic!("expected directory, got {dir:?}");
        };

        children.insert(file_name.to_string(), node);
    }

    fn add_directory(&mut self, path: &GuestPath) {
        self.find_or_make_directory(path);
    }

    fn build(self) -> FsNode {
        self.root
    }
}

/// Returns the path of a zip entry.
///
/// Entries without the zip "UTF-8 filename" flag are supposed to be CP437,
/// and that is how the `zip` crate decodes them. But many IPAs (zipped on
/// macOS or by IPA tools) store UTF-8 names without setting the flag. Decoding
/// those as CP437 garbles any non-ASCII name, e.g. an Arabic app bundle name,
/// which then no longer matches the `CFBundleExecutable` in the Info.plist and
/// the executable can't be found. Real CP437 names with non-ASCII characters
/// are almost never valid UTF-8, so prefer UTF-8 whenever the bytes allow it.
fn zip_entry_name(file: &zip::read::ZipFile) -> String {
    match std::str::from_utf8(file.name_raw()) {
        Ok(name) => name.to_string(),
        Err(_) => file.name().to_string(),
    }
}

/// Warn once about IPA entries that can't be read. The damage is in the file
/// itself (e.g. a truncated or corrupted download), so the app will see these
/// files as missing; naming them makes that diagnosable from a log.
fn log_damaged_entries(count: usize, names: &[&str], error: &str) {
    const SHOWN: usize = 10;
    let mut list = names
        .iter()
        .take(SHOWN)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > SHOWN {
        list.push_str(&format!(", and {} more", names.len() - SHOWN));
    }
    log!(
        "Warning: {} IPA archive entries are damaged ({}) and will be missing from the app bundle: [{}]. The IPA file is probably corrupt; try downloading it again.",
        count,
        error,
        list
    );
}

/// Represents an open app bundle, either a directory or a zip file.
pub enum BundleData {
    HostDirectory(PathBuf),
    Zip {
        zip: ZipArchive<std::fs::File>,
        /// Path to the app bundle inside the zip file.
        /// It should be `"Payload/<app name>.app"` (no trailing slash!).
        bundle_path: String,
    },
}

impl BundleData {
    fn find_bundle_path_in_archive(zip: &mut ZipArchive<std::fs::File>) -> Result<String, String> {
        for i in 0..zip.len() {
            // Some IPAs found in the wild contain entries whose local file
            // header is corrupt (even though the central directory is fine).
            // Skipping such entries is better than rejecting the whole IPA.
            let path = match zip.by_index(i).map(|file| zip_entry_name(&file)) {
                Ok(path) => path,
                Err(e) => {
                    log!(
                        "Warning: BundleData::find_bundle_path_in_archive(): skipping unreadable IPA archive entry #{}: {}",
                        i,
                        e
                    );
                    continue;
                }
            };
            if let Some(name) = path
                .strip_prefix("Payload/")
                .and_then(|path| path.split_once('/'))
                .and_then(|(name, _)| name.strip_suffix(".app"))
            {
                return Ok(format!("Payload/{name}.app"));
            }
        }
        Err("no app bundle found in the IPA archive".to_string())
    }

    pub fn bundle_name(&self) -> &str {
        match self {
            BundleData::HostDirectory(bundle_path) => {
                bundle_path.file_stem().unwrap().to_str().unwrap()
            }
            BundleData::Zip { bundle_path, .. } => bundle_path
                .rsplit_once('/')
                .unwrap()
                .1
                .strip_suffix(".app")
                .unwrap(),
        }
    }

    pub fn open_host_dir(path: &Path) -> Result<BundleData, String> {
        Ok(BundleData::HostDirectory(path.to_path_buf()))
    }

    pub fn open_ipa(path: &Path) -> Result<BundleData, String> {
        let file =
            std::fs::File::open(path).map_err(|e| format!("Could not open IPA file: {e}"))?;
        let mut zip =
            ZipArchive::new(file).map_err(|e| format!("Could not open IPA archive: {e}"))?;
        let bundle_path = Self::find_bundle_path_in_archive(&mut zip)?;
        Ok(BundleData::Zip { zip, bundle_path })
    }

    pub fn open_any(path: &Path) -> Result<BundleData, String> {
        if path.is_file()
            && path
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("ipa"))
                .unwrap_or(false)
        {
            Ok(Self::open_ipa(path)?)
        } else if path.is_dir() {
            Ok(Self::open_host_dir(path)?)
        } else {
            Err(format!(
                "{} is not a directory or an IPA file",
                path.display()
            ))
        }
    }

    pub(super) fn into_fs_node(self) -> FsNode {
        match self {
            BundleData::HostDirectory(path) => FsNode::from_host_dir(&path, false),
            BundleData::Zip { zip, bundle_path } => {
                let archive = Rc::new(RefCell::new(zip));
                let archive_cache = Rc::new(RefCell::new(HashMap::new()));
                let metadata_map = Rc::new(RefCell::new(HashMap::new()));

                let mut archive_guard = (*archive).borrow_mut();

                let mut builder = FsNodeBuilder::new();
                // Central-directory names of the entries that could be read,
                // used to name the damaged ones afterwards.
                let mut readable_names = HashSet::new();
                let mut first_error = None;
                let mut damaged_count = 0;
                for i in 0..archive_guard.len() {
                    // Unreadable entries (e.g. with corrupt local file
                    // headers) are skipped rather than causing a panic; the
                    // rest of the bundle is still loaded, and the app sees
                    // the skipped files as missing.
                    let (name, is_dir) = match archive_guard.by_index(i).map(|file| {
                        readable_names.insert(file.name().to_string());
                        (zip_entry_name(&file), file.is_dir())
                    }) {
                        Ok(name_and_dir) => name_and_dir,
                        Err(e) => {
                            first_error.get_or_insert_with(|| e.to_string());
                            damaged_count += 1;
                            continue;
                        }
                    };
                    if let Some(path) = name.strip_prefix(&bundle_path) {
                        let path = GuestPath::new(path);
                        if is_dir {
                            builder.add_directory(path);
                        } else {
                            builder.add_file(
                                path,
                                FsNode::bundle_zip_file(IpaFileRef {
                                    archive: archive.clone(),
                                    archive_files_cache: archive_cache.clone(),
                                    metadata_map: metadata_map.clone(),
                                    index: i,
                                }),
                            );
                        }
                    }
                }
                if let Some(error) = first_error {
                    let mut damaged: Vec<&str> = archive_guard
                        .file_names()
                        .filter(|name| !readable_names.contains(*name))
                        .collect();
                    damaged.sort_unstable();
                    log_damaged_entries(damaged_count, &damaged, &error);
                }
                builder.build()
            }
        }
    }

    pub fn read_plist(&mut self) -> Result<Vec<u8>, String> {
        match self {
            BundleData::HostDirectory(path) => {
                std::fs::read(path.join("Info.plist")).map_err(|e| {
                    format!("Could not read Info.plist from the app bundle directory: {e}")
                })
            }
            BundleData::Zip { zip, bundle_path } => {
                // Look the entry up by our own decoding of the names (see
                // zip_entry_name()) rather than with ZipArchive::by_name(),
                // which only knows the CP437-decoded names.
                let plist_path = format!("{bundle_path}/Info.plist");
                let index = (0..zip.len())
                    .find(|&i| {
                        zip.by_index_raw(i)
                            .is_ok_and(|file| zip_entry_name(&file) == plist_path)
                    })
                    .ok_or_else(|| {
                        "Could not open Info.plist from the IPA archive: file not found".to_string()
                    })?;
                let mut file = zip
                    .by_index(index)
                    .map_err(|e| format!("Could not open Info.plist from the IPA archive: {e}"))?;
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)
                    .map_err(|e| format!("Could not read Info.plist from the IPA archive: {e}"))?;
                Ok(buf)
            }
        }
    }
}

#[derive(Debug)]
struct ArchivedFileMetadata {
    /// Unix timestamp of file modification
    last_modified: i64,
    /// Uncompressed file size
    size: u64,
}

/// Shared (refcounted) copy of the decompressed version of a file in an IPA.
///
/// Seeking in compressed files is hard, so the simple solution is to read the
/// whole file into memory. This is shared so having multiple copies of the same
/// file open won't waste memory.
type DecompressedFile = Rc<[u8]>;

/// Represents a file inside an IPA bundle that can be opened.
#[derive(Debug)]
pub struct IpaFileRef {
    archive: Rc<RefCell<ZipArchive<std::fs::File>>>,
    archive_files_cache: Rc<RefCell<HashMap<usize, DecompressedFile>>>,
    metadata_map: Rc<RefCell<HashMap<usize, ArchivedFileMetadata>>>,
    index: usize,
}

impl IpaFileRef {
    pub fn open(&self) -> IpaFile {
        // Some games, like THPS2, use a single resource bundle file which is
        // opened each time a new game resource is being read.
        // As IPA is basically an archive, this pattern requires unzipping to be
        // done each time, which is extremely slow.
        // The solution here is to cache unzipped data in memory, which should
        // be OK as early iOS IPA files are relatively small in size.
        let mut archive_cache = (*self.archive_files_cache).borrow_mut();
        archive_cache.entry(self.index).or_insert_with(|| {
            // Read the zip entry into an owned buffer inside its own block so
            // the `archive` RefMut is released before we touch the caches.
            let mut archive = (*self.archive).borrow_mut();
            let decoded: Option<(Vec<u8>, ArchivedFileMetadata)> = match archive
                .by_index(self.index)
            {
                Ok(mut file) => {
                    let modified = file.last_modified();
                    // This is not the cleanest way!
                    // TODO: just use `time` or `chrono` crates for time conversions
                    // (this also entails a lot of refactoring in [crate::libc:time])
                    let tm = tm::from(
                        modified.year(),
                        modified.month(),
                        modified.day(),
                        modified.hour(),
                        modified.minute(),
                        modified.second(),
                    );
                    let timestamp = calendar_date_to_timestamp(tm);
                    let size = file.size();
                    let mut buf = Vec::new();
                    if let Err(e) = file.read_to_end(&mut buf) {
                        // The central directory can be readable even when the
                        // compressed payload is truncated or has a bad CRC.
                        // Discarding a partial archive is safer than letting a
                        // guest parse corrupted Unity/player data and continue
                        // in a damaged state.
                        log!(
                            "Warning: IpaFileRef::open(): IO error decompressing IPA entry {}: {}; rejecting partial buffer ({} bytes).",
                            self.index,
                            e,
                            buf.len()
                        );
                        None
                    } else {
                        Some((
                            buf,
                            ArchivedFileMetadata {
                                last_modified: timestamp.into(),
                                size,
                            },
                        ))
                    }
                }
                Err(ZipError::Io(e)) => {
                    log!(
                        "Warning: IpaFileRef::open(): IO error reading IPA entry {}: {}; returning empty file to guest.",
                        self.index,
                        e
                    );
                    None
                }
                Err(e) => {
                    log!(
                        "Warning: IpaFileRef::open(): could not open IPA entry {}: {}; returning empty file to guest.",
                        self.index,
                        e
                    );
                    None
                }
            };
            drop(archive);

            match decoded {
                Some((buf, meta)) => {
                    (*self.metadata_map)
                        .borrow_mut()
                        .entry(self.index)
                        .or_insert(meta);
                    Rc::from(buf)
                }
                None => {
                    // Cache an explicit zero-size metadata record as well as
                    // the empty data. This prevents repeated stat() probes of
                    // the same bad ZIP entry from retrying decompression or
                    // panicking on absent metadata.
                    (*self.metadata_map)
                        .borrow_mut()
                        .entry(self.index)
                        .or_insert(ArchivedFileMetadata {
                            last_modified: 0,
                            size: 0,
                        });
                    Rc::from(Vec::new())
                }
            }
        });
        let cached_file = Rc::clone(archive_cache.get(&self.index).unwrap());
        IpaFile {
            file: Cursor::new(cached_file),
        }
    }

    pub fn get_last_modified(&self) -> time_t {
        if !self.metadata_map.borrow().contains_key(&self.index) {
            // This will force metadata loading. Broken ZIP entries cache
            // explicit zero metadata, so a guest stat() remains safe.
            _ = self.open();
        }
        let last_modified = {
            self.metadata_map
                .borrow()
                .get(&self.index)
                .map(|metadata| metadata.last_modified)
        };
        match last_modified {
            Some(last_modified) => last_modified.try_into().unwrap_or(0),
            None => {
                log!(
                    "Warning: IpaFileRef::get_last_modified(): IPA entry {} has \
                     no readable metadata; reporting timestamp zero.",
                    self.index
                );
                0
            }
        }
    }
    pub fn get_size(&self) -> u64 {
        if !self.metadata_map.borrow().contains_key(&self.index) {
            // See get_last_modified(): unreadable archive entries must remain
            // visible as an empty/unusable guest file rather than panicking the
            // emulator while probing their size.
            _ = self.open();
        }
        let size = {
            self.metadata_map
                .borrow()
                .get(&self.index)
                .map(|metadata| metadata.size)
        };
        match size {
            Some(size) => size,
            None => {
                log!(
                    "Warning: IpaFileRef::get_size(): IPA entry {} has no readable \
                     metadata; reporting size zero.",
                    self.index
                );
                0
            }
        }
    }
}

/// Represents an opened file in an IPA bundle.
#[derive(Clone)]
pub struct IpaFile {
    file: Cursor<DecompressedFile>,
}

impl Debug for IpaFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpaFile")
            .field("size", &self.file.get_ref().len())
            .finish()
    }
}

impl Read for IpaFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl std::io::Seek for IpaFile {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::FileLocation;
    use std::io::{Read, Write};

    /// Regression test: IPAs with entries whose local file header is corrupt
    /// used to cause a panic (`InvalidArchive("Invalid local file header")`)
    /// when the bundle was loaded. Now such entries are skipped instead.
    #[test]
    fn test_ipa_with_corrupt_local_file_header() {
        let dir = std::env::temp_dir().join("touchHLE_test_corrupt_ipa");
        std::fs::create_dir_all(&dir).unwrap();
        let ipa_path = dir.join("TestApp.ipa");

        // Build a minimal IPA.
        let file = std::fs::File::create(&ipa_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::FileOptions::default();
        zip.start_file("Payload/TestApp.app/Info.plist", options)
            .unwrap();
        zip.write_all(b"plist").unwrap();
        zip.start_file("Payload/TestApp.app/broken.bin", options)
            .unwrap();
        zip.write_all(b"data").unwrap();
        zip.finish().unwrap();

        // Corrupt the local file header of the second entry (keep the
        // central directory intact), mimicking IPAs found in the wild.
        {
            let mut zip = ZipArchive::new(std::fs::File::open(&ipa_path).unwrap()).unwrap();
            let header_start = zip.by_index(1).unwrap().header_start();
            drop(zip);
            let mut data = std::fs::read(&ipa_path).unwrap();
            let start = header_start as usize;
            data[start..start + 4].copy_from_slice(b"XXXX");
            std::fs::write(&ipa_path, data).unwrap();
        }

        // Opening the IPA must succeed ...
        let mut bundle = BundleData::open_any(&ipa_path).unwrap();
        assert_eq!(bundle.bundle_name(), "TestApp");
        // ... reading the plist must work ...
        assert_eq!(bundle.read_plist().unwrap(), b"plist");
        // ... and building the filesystem node must not panic; the broken
        // entry is skipped, the valid one is still present.
        let node = bundle.into_fs_node();
        let FsNode::Directory { children, .. } = &node else {
            panic!("expected directory");
        };
        assert!(children.contains_key("Info.plist"));
        assert!(!children.contains_key("broken.bin"));
    }

    /// Many IPAs store UTF-8 entry names without setting the zip UTF-8 flag.
    /// Those names must not be decoded as CP437, or a non-ASCII bundle name
    /// (e.g. Arabic) stops matching the executable name from Info.plist.
    #[test]
    fn test_ipa_with_unflagged_utf8_names() {
        let dir = std::env::temp_dir().join("touchHLE_test_unflagged_utf8_ipa");
        std::fs::create_dir_all(&dir).unwrap();
        let ipa_path = dir.join("TestApp.ipa");

        let app = "الابراج الفلكية";
        let file = std::fs::File::create(&ipa_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file(format!("Payload/{app}.app/Info.plist"), options)
            .unwrap();
        zip.write_all(b"plist").unwrap();
        zip.start_file(format!("Payload/{app}.app/{app}"), options)
            .unwrap();
        zip.write_all(b"exec").unwrap();
        zip.finish().unwrap();

        // The writer sets the UTF-8 flag (bit 11) for these names; clear it in
        // every local file header and central directory header.
        let mut bytes = std::fs::read(&ipa_path).unwrap();
        for i in 0..bytes.len().saturating_sub(10) {
            let flags_offset = match &bytes[i..i + 4] {
                b"PK\x03\x04" => 6,
                b"PK\x01\x02" => 8,
                _ => continue,
            };
            bytes[i + flags_offset + 1] &= !0x08;
        }
        std::fs::write(&ipa_path, bytes).unwrap();
        {
            let mut zip = ZipArchive::new(std::fs::File::open(&ipa_path).unwrap()).unwrap();
            // Sanity check: the zip crate itself now sees CP437 mojibake.
            assert_ne!(
                zip.by_index(0).unwrap().name(),
                format!("Payload/{app}.app/Info.plist")
            );
        }

        let mut bundle = BundleData::open_any(&ipa_path).unwrap();
        assert_eq!(bundle.bundle_name(), app);
        assert_eq!(bundle.read_plist().unwrap(), b"plist");
        let node = bundle.into_fs_node();
        let FsNode::Directory { children, .. } = &node else {
            panic!("expected directory");
        };
        assert!(children.contains_key("Info.plist"));
        assert!(children.contains_key(app));
    }

    /// A central-directory entry can be listed successfully but fail its CRC
    /// while decompressing. Its metadata queries must report the safe empty
    /// representation rather than panicking the emulator.
    #[test]
    fn test_ipa_with_corrupt_payload_is_empty_not_panic() {
        let dir = std::env::temp_dir().join("touchHLE_test_corrupt_ipa_payload");
        std::fs::create_dir_all(&dir).unwrap();
        let ipa_path = dir.join("TestApp.ipa");

        let file = std::fs::File::create(&ipa_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("Payload/TestApp.app/Info.plist", options)
            .unwrap();
        zip.write_all(b"plist").unwrap();
        zip.start_file("Payload/TestApp.app/Data/data.unity3d", options)
            .unwrap();
        zip.write_all(b"player-data").unwrap();
        zip.finish().unwrap();

        // Alter stored payload bytes but retain the local header and central
        // directory. `by_index` can still find this entry; read_to_end checks
        // its CRC and reports an error.
        let data_start = {
            let mut zip = ZipArchive::new(std::fs::File::open(&ipa_path).unwrap()).unwrap();
            let data_start = zip.by_index(1).unwrap().data_start();
            data_start
        };
        let mut bytes = std::fs::read(&ipa_path).unwrap();
        bytes[data_start as usize] ^= 0xff;
        std::fs::write(&ipa_path, bytes).unwrap();

        let bundle = BundleData::open_any(&ipa_path).unwrap();
        let node = bundle.into_fs_node();
        let FsNode::Directory { children, .. } = &node else {
            panic!("expected bundle directory");
        };
        let FsNode::Directory { children, .. } = children.get("Data").unwrap() else {
            panic!("expected Data directory");
        };
        let FsNode::File {
            location: FileLocation::IpaFileRef(file_ref),
            ..
        } = children.get("data.unity3d").unwrap()
        else {
            panic!("expected IPA player archive");
        };

        assert_eq!(file_ref.get_size(), 0);
        assert_eq!(file_ref.get_last_modified(), 0);
        let mut contents = Vec::new();
        file_ref.open().read_to_end(&mut contents).unwrap();
        assert!(contents.is_empty());
    }
}
