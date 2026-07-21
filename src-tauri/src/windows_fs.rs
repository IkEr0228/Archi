use std::ffi::c_void;
use std::fs::File;
use std::io::{self, Read};
use std::mem::{align_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};
use std::path::{Component, Path, PathBuf, PrefixComponent};
use std::sync::Arc;

const OBJ_CASE_INSENSITIVE: u32 = 0x40;
const OBJ_DONT_REPARSE: u32 = 0x1000;
const FILE_SHARE_READ_WRITE: u32 = 0x3;
const FILE_SHARE_READ_WRITE_DELETE: u32 = 0x7;
const FILE_OPEN: u32 = 1;
const FILE_CREATE: u32 = 2;
const FILE_DIRECTORY_FILE: u32 = 0x1;
const FILE_NON_DIRECTORY_FILE: u32 = 0x40;
const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x20;
const FILE_OPEN_REPARSE_POINT: u32 = 0x20_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_LIST_DIRECTORY: u32 = 0x1;
const FILE_TRAVERSE: u32 = 0x20;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const DELETE: u32 = 0x1_0000;
const SYNCHRONIZE: u32 = 0x10_0000;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_RENAME_INFORMATION: u32 = 10;
const FILE_DISPOSITION_INFORMATION: u32 = 13;

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: RawHandle,
    object_name: *mut UnicodeString,
    attributes: u32,
    security_descriptor: *mut c_void,
    security_quality_of_service: *mut c_void,
}

#[repr(C)]
struct IoStatusBlock {
    status: i32,
    information: usize,
}

#[repr(C)]
struct FileDispositionInfo {
    delete_file: u8,
}

#[link(name = "ntdll")]
extern "system" {
    fn NtCreateFile(
        file_handle: *mut RawHandle,
        desired_access: u32,
        object_attributes: *mut ObjectAttributes,
        io_status_block: *mut IoStatusBlock,
        allocation_size: *mut i64,
        file_attributes: u32,
        share_access: u32,
        create_disposition: u32,
        create_options: u32,
        ea_buffer: *mut c_void,
        ea_length: u32,
    ) -> i32;
    fn NtSetInformationFile(
        file: RawHandle,
        io_status_block: *mut IoStatusBlock,
        file_information: *mut c_void,
        length: u32,
        file_information_class: u32,
    ) -> i32;
}

#[derive(Clone)]
pub struct Directory {
    current: Arc<File>,
    ancestors: Vec<Arc<File>>,
}

pub struct CreatedEntry {
    handle: Arc<File>,
}

/// Result of probing a leaf name under a directory handle without following reparse points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeafProbe {
    NotFound,
    Reparse,
    Directory,
    File,
}

pub struct PinnedFile {
    _parent: Directory,
    file: File,
}

impl Read for PinnedFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.file.read(buffer)
    }
}

pub fn open_source_file(path: &Path) -> io::Result<PinnedFile> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Source path has no parent."))?;
    let file_name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "Source path has no file name.")
    })?;
    let directory = Directory::open_root(parent)?;
    let name = file_name.encode_wide().collect::<Vec<_>>();
    let file = directory.open_existing_file(&name)?;
    Ok(PinnedFile {
        _parent: directory,
        file,
    })
}

impl Directory {
    pub fn open_root(path: &Path) -> io::Result<Self> {
        let mut components = path.components();
        let prefix = match components.next() {
            Some(Component::Prefix(prefix)) => prefix,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Destination path has no Windows prefix.",
                ))
            }
        };
        if !matches!(components.next(), Some(Component::RootDir)) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Destination path has no Windows root.",
            ));
        }
        let root_path = dos_root_path(prefix);
        let root = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(root_path)?;
        let mut current = Self {
            current: Arc::new(root),
            ancestors: Vec::new(),
        };
        for component in components {
            let name = component.as_os_str().encode_wide().collect::<Vec<_>>();
            current = current.open_existing_directory(&name)?;
        }
        Ok(current)
    }

    /// Resolve (and create) the parent directory for `destination` under `root`.
    ///
    /// Reuses directory handles from `cache` keyed by relative path under `root`.
    /// Required when extracting multiple entries under a newly created parent:
    /// create handles hold DELETE without sharing it, so a second NtCreateFile
    /// open would hit STATUS_SHARING_VIOLATION.
    pub fn parent_for(
        &self,
        root: &Path,
        destination: &Path,
        created: &mut Vec<CreatedEntry>,
        cache: &mut std::collections::HashMap<PathBuf, Self>,
    ) -> io::Result<Self> {
        let parent = destination.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Archive entry has no parent.")
        })?;
        let relative = parent.strip_prefix(root).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Archive entry escaped destination.",
            )
        })?;
        if relative.as_os_str().is_empty() {
            return Ok(self.clone());
        }
        if let Some(cached) = cache.get(relative) {
            return Ok(cached.clone());
        }
        let mut current = self.clone();
        let mut walked = PathBuf::new();
        for component in relative.components() {
            walked.push(component);
            if let Some(cached) = cache.get(&walked) {
                current = cached.clone();
                continue;
            }
            let name = component.as_os_str().encode_wide().collect::<Vec<_>>();
            let (next, was_created) = current.open_or_create_directory(&name)?;
            if was_created {
                created.push(next.created_entry());
            }
            cache.insert(walked.clone(), next.clone());
            current = next;
        }
        Ok(current)
    }

    /// Ensure `directory_path` exists under `root`, creating intermediates as needed.
    ///
    /// Walks and caches every path segment (same cache as [`Self::parent_for`]) so
    /// later file entries under this directory reuse the same handles and do not
    /// re-open a create handle that holds exclusive DELETE access.
    pub fn ensure_path(
        &self,
        root: &Path,
        directory_path: &Path,
        created: &mut Vec<CreatedEntry>,
        cache: &mut std::collections::HashMap<PathBuf, Self>,
    ) -> io::Result<Self> {
        let relative = directory_path.strip_prefix(root).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Archive entry escaped destination.",
            )
        })?;
        if relative.as_os_str().is_empty() {
            return Ok(self.clone());
        }
        if let Some(cached) = cache.get(relative) {
            return Ok(cached.clone());
        }
        let mut current = self.clone();
        let mut walked = PathBuf::new();
        for component in relative.components() {
            walked.push(component);
            if let Some(cached) = cache.get(&walked) {
                current = cached.clone();
                continue;
            }
            let name = component.as_os_str().encode_wide().collect::<Vec<_>>();
            let (next, was_created) = current.open_or_create_directory(&name)?;
            if was_created {
                created.push(next.created_entry());
            }
            cache.insert(walked.clone(), next.clone());
            current = next;
        }
        Ok(current)
    }

    #[allow(dead_code)]
    pub fn ensure_directory(
        &self,
        name: Vec<u16>,
        created: &mut Vec<CreatedEntry>,
    ) -> io::Result<()> {
        let (directory, was_created) = self.open_or_create_directory(&name)?;
        if was_created {
            created.push(directory.created_entry());
        }
        Ok(())
    }

    pub fn create_file(
        &self,
        name: &[u16],
        created: &mut Vec<CreatedEntry>,
    ) -> io::Result<Arc<File>> {
        let handle = Arc::new(open_path(
            Some(&self.current),
            name.to_vec(),
            0x4000_0000 | DELETE | SYNCHRONIZE,
            FILE_CREATE,
            FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
        )?);
        created.push(CreatedEntry {
            handle: Arc::clone(&handle),
        });
        Ok(handle)
    }

    pub fn rename_new_file(&self, source: &CreatedEntry, destination: &[u16]) -> io::Result<()> {
        let header_size = file_rename_name_offset();
        let name_size = destination.len().checked_mul(2).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "Destination name is too long.")
        })?;
        let mut bytes = vec![0_u8; header_size + name_size + size_of::<u16>()];
        let handle_offset = file_rename_handle_offset();
        unsafe {
            bytes[0] = 0;
            bytes
                .as_mut_ptr()
                .add(handle_offset)
                .cast::<RawHandle>()
                .write_unaligned(self.current.as_raw_handle());
            bytes
                .as_mut_ptr()
                .add(header_size - size_of::<u32>())
                .cast::<u32>()
                .write_unaligned(name_size as u32);
            std::ptr::copy_nonoverlapping(
                destination.as_ptr().cast::<u8>(),
                bytes.as_mut_ptr().add(header_size),
                name_size,
            );
        }
        let mut status = IoStatusBlock {
            status: 0,
            information: 0,
        };
        let result = unsafe {
            NtSetInformationFile(
                source.handle.as_raw_handle(),
                &mut status,
                bytes.as_mut_ptr().cast(),
                bytes.len() as u32,
                FILE_RENAME_INFORMATION,
            )
        };
        if result < 0 {
            return Err(io::Error::other(format!(
                "NtSetInformationFile failed with status {result:#x}"
            )));
        }
        Ok(())
    }

    /// Probe a leaf name under this directory without following reparse points.
    ///
    /// Uses handle-relative `NtCreateFile` with `OBJ_DONT_REPARSE` +
    /// `FILE_OPEN_REPARSE_POINT` so existence checks cannot race through a
    /// swapped-in symlink parent or leaf.
    pub fn try_probe_file(&self, name: &[u16]) -> io::Result<LeafProbe> {
        match open_path(
            Some(&self.current),
            name.to_vec(),
            FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            FILE_OPEN,
            FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
        ) {
            Ok(file) => {
                let metadata = file.metadata()?;
                if is_handle_reparse(&metadata) {
                    Ok(LeafProbe::Reparse)
                } else if metadata.is_dir() {
                    Ok(LeafProbe::Directory)
                } else {
                    Ok(LeafProbe::File)
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(LeafProbe::NotFound),
            Err(error) => Err(error),
        }
    }

    /// Open an existing non-directory leaf with DELETE access (reparse-safe).
    pub fn open_existing_file_for_delete(&self, name: &[u16]) -> io::Result<File> {
        // Share DELETE so an existing non-exclusive reader does not block overwrite
        // of a pre-existing conflict leaf. Create-time handles still omit DELETE
        // share to pin partial extracts against replacement.
        open_path_with_share(
            Some(&self.current),
            name.to_vec(),
            FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            FILE_SHARE_READ_WRITE_DELETE,
        )
        .map(|(file, _)| file)
    }

    /// Delete a regular file leaf by name via handle disposition (not path `remove_file`).
    ///
    /// Refuses reparse points. Treats `NotFound` as success (raced free path).
    pub fn delete_file_by_name(&self, name: &[u16]) -> io::Result<()> {
        let file = match self.open_existing_file_for_delete(name) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let metadata = file.metadata()?;
        if is_handle_reparse(&metadata) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Destination entry is a symbolic link or reparse point.",
            ));
        }
        mark_handle_for_delete(&file)?;
        drop(file);
        Ok(())
    }

    fn open_or_create_directory(&self, name: &[u16]) -> io::Result<(Self, bool)> {
        // Create handles include DELETE without FILE_SHARE_DELETE so ancestors stay
        // pinned during extract. Callers must reuse handles via `parent_for` /
        // `ensure_path` cache — never re-open a just-created directory.
        match self.open_existing_directory(name) {
            Ok(directory) => Ok((directory, false)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => match open_path(
                Some(&self.current),
                name.to_vec(),
                FILE_LIST_DIRECTORY | FILE_TRAVERSE | DELETE | SYNCHRONIZE,
                FILE_CREATE,
                FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            ) {
                Ok(file) => Ok((self.with_child(file), true)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => self
                    .open_existing_directory(name)
                    .map(|directory| (directory, false)),
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        }
    }

    fn open_existing_directory(&self, name: &[u16]) -> io::Result<Self> {
        open_path(
            Some(&self.current),
            name.to_vec(),
            FILE_LIST_DIRECTORY | FILE_TRAVERSE | SYNCHRONIZE,
            FILE_OPEN,
            FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
        )
        .map(|file| self.with_child(file))
    }

    fn open_existing_file(&self, name: &[u16]) -> io::Result<File> {
        open_path(
            Some(&self.current),
            name.to_vec(),
            0x8000_0000 | SYNCHRONIZE,
            FILE_OPEN,
            FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
        )
    }

    fn with_child(&self, file: File) -> Self {
        let mut ancestors = self.ancestors.clone();
        ancestors.push(Arc::clone(&self.current));
        Self {
            current: Arc::new(file),
            ancestors,
        }
    }

    fn created_entry(&self) -> CreatedEntry {
        CreatedEntry {
            handle: Arc::clone(&self.current),
        }
    }
}

fn file_rename_name_offset() -> usize {
    file_rename_handle_offset() + size_of::<RawHandle>() + size_of::<u32>()
}

fn file_rename_handle_offset() -> usize {
    (size_of::<u8>() + align_of::<RawHandle>() - 1) & !(align_of::<RawHandle>() - 1)
}

pub fn cleanup_created(entries: &mut Vec<CreatedEntry>) -> Vec<String> {
    let mut failures = Vec::new();
    while let Some(entry) = entries.pop() {
        if let Err(error) = mark_handle_for_delete(entry.handle.as_ref()) {
            failures.push(error.to_string());
        }
        drop(entry);
    }
    failures
}

fn is_handle_reparse(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
        || (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

fn mark_handle_for_delete(file: &File) -> io::Result<()> {
    let mut disposition = FileDispositionInfo { delete_file: 1 };
    let mut status = IoStatusBlock {
        status: 0,
        information: 0,
    };
    let result = unsafe {
        NtSetInformationFile(
            file.as_raw_handle(),
            &mut status,
            (&mut disposition as *mut FileDispositionInfo).cast(),
            size_of::<FileDispositionInfo>() as u32,
            FILE_DISPOSITION_INFORMATION,
        )
    };
    if result < 0 {
        return Err(io::Error::other(format!(
            "NtSetInformationFile failed with status {result:#x}"
        )));
    }
    Ok(())
}

fn dos_root_path(prefix: PrefixComponent<'_>) -> PathBuf {
    let prefix = prefix.as_os_str().to_string_lossy();
    PathBuf::from(format!("{}\\", prefix))
}

fn nt_error(status: i32) -> io::Error {
    let status_u = status as u32;
    let (kind, detail) = match status_u {
        0xC000_0034 | 0xC000_003A => (io::ErrorKind::NotFound, "not found"),
        0xC000_0035 => (io::ErrorKind::AlreadyExists, "already exists"),
        // STATUS_SHARING_VIOLATION — share access incompatible with open handles.
        0xC000_0043 => (io::ErrorKind::Other, "sharing violation"),
        _ => (io::ErrorKind::Other, "open failed"),
    };
    io::Error::new(kind, format!("NtCreateFile {detail} (status {status:#x})"))
}

fn open_path(
    root: Option<&File>,
    name: Vec<u16>,
    access: u32,
    disposition: u32,
    options: u32,
) -> io::Result<File> {
    open_path_with_share(
        root,
        name,
        access,
        disposition,
        options,
        FILE_SHARE_READ_WRITE,
    )
    .map(|(file, _)| file)
}

fn open_path_with_share(
    root: Option<&File>,
    mut name: Vec<u16>,
    access: u32,
    disposition: u32,
    options: u32,
    share_access: u32,
) -> io::Result<(File, usize)> {
    if name.is_empty() || name.len() > (u16::MAX as usize) / 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Path is invalid.",
        ));
    }
    let mut object_name = UnicodeString {
        length: (name.len() * 2) as u16,
        maximum_length: (name.len() * 2) as u16,
        buffer: name.as_mut_ptr(),
    };
    let mut attributes = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: root.map_or(std::ptr::null_mut(), AsRawHandle::as_raw_handle),
        object_name: &mut object_name,
        attributes: OBJ_CASE_INSENSITIVE | OBJ_DONT_REPARSE,
        security_descriptor: std::ptr::null_mut(),
        security_quality_of_service: std::ptr::null_mut(),
    };
    let mut status = IoStatusBlock {
        status: 0,
        information: 0,
    };
    let mut handle = std::ptr::null_mut();
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &mut attributes,
            &mut status,
            std::ptr::null_mut(),
            0,
            share_access,
            disposition,
            options,
            std::ptr::null_mut(),
            0,
        )
    };
    if result < 0 {
        return Err(nt_error(result));
    }
    Ok((unsafe { File::from_raw_handle(handle) }, status.information))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn retained_handles_block_replacement_and_cleanup_created_paths() {
        assert_eq!(size_of::<FileDispositionInfo>(), 1);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "archi-handle-cleanup-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();

        let destination = root.join("nested").join("file.bin");
        let directory = Directory::open_root(&root).unwrap();
        let mut created = Vec::new();
        let mut cache = std::collections::HashMap::new();
        let parent = directory
            .parent_for(&root, &destination, &mut created, &mut cache)
            .unwrap();
        let name = destination
            .file_name()
            .unwrap()
            .encode_wide()
            .collect::<Vec<_>>();
        let file = parent.create_file(&name, &mut created).unwrap();
        let mut writer = file.as_ref();
        writer.write_all(b"content").unwrap();
        drop(file);

        assert!(std::fs::rename(&destination, root.join("replacement.bin")).is_err());
        drop(parent);
        drop(cache);
        assert!(cleanup_created(&mut created).is_empty());
        assert!(!destination.exists());
        assert!(!root.join("nested").exists());

        drop(directory);
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn retained_chain_blocks_renaming_existing_ancestors() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "archi-ancestor-chain-{}-{nonce}",
            std::process::id()
        ));
        let first = root.join("first");
        let second = first.join("second");
        std::fs::create_dir_all(&second).unwrap();

        let directory = Directory::open_root(&root).unwrap();
        let mut created = Vec::new();
        let mut cache = std::collections::HashMap::new();
        let parent = directory
            .parent_for(&root, &second.join("file.bin"), &mut created, &mut cache)
            .unwrap();

        assert!(created.is_empty());
        assert!(std::fs::rename(&first, root.join("first-moved")).is_err());
        assert!(std::fs::rename(&second, first.join("second-moved")).is_err());

        drop(parent);
        drop(cache);
        drop(directory);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn probe_and_handle_delete_replace_existing_leaf() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("archi-probe-delete-{}-{nonce}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let destination = root.join("file.bin");
        std::fs::write(&destination, b"old").unwrap();

        let directory = Directory::open_root(&root).unwrap();
        let name = destination
            .file_name()
            .unwrap()
            .encode_wide()
            .collect::<Vec<_>>();

        assert_eq!(directory.try_probe_file(&name).unwrap(), LeafProbe::File);
        directory.delete_file_by_name(&name).unwrap();
        assert_eq!(
            directory.try_probe_file(&name).unwrap(),
            LeafProbe::NotFound
        );
        assert!(!destination.exists());

        // Missing leaf is a free path, not an error.
        directory.delete_file_by_name(&name).unwrap();

        let mut created = Vec::new();
        let file = directory.create_file(&name, &mut created).unwrap();
        {
            let mut writer = file.as_ref();
            writer.write_all(b"new").unwrap();
        }
        drop(file);
        drop(created);
        drop(directory);

        assert_eq!(std::fs::read(&destination).unwrap(), b"new");
        std::fs::remove_dir_all(root).unwrap();
    }
}
