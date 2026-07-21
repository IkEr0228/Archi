//! Shared source enumeration, validation, temp file, and publish for multi-format create.

use crate::models::CommandError;
use crate::security::is_link_or_reparse_point;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) use crate::io_perf::{ProgressGate, IO_BUFFER_SIZE as BUFFER_SIZE, PROGRESS_INTERVAL};

#[derive(Debug, Clone)]
pub(crate) struct SourceEntry {
    pub path: PathBuf,
    /// Logical path inside the archive (dirs may end with `/`).
    pub archive_path: String,
    pub is_directory: bool,
}

pub(crate) fn create_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

pub(crate) fn cancelled_error() -> CommandError {
    create_error("cancelled", "Archive creation was cancelled.")
}

pub(crate) fn source_metadata(path: &Path) -> Result<fs::Metadata, CommandError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        create_error(
            if error.kind() == std::io::ErrorKind::NotFound {
                "source_not_found"
            } else {
                "source_read"
            },
            format!("Cannot inspect source {}: {error}", path.display()),
        )
    })?;
    if is_link_or_reparse_point(&metadata) {
        return Err(create_error(
            "invalid_source",
            format!("Source links are not supported: {}", path.display()),
        ));
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(create_error(
            "invalid_source",
            format!("Source is not a file or directory: {}", path.display()),
        ));
    }
    Ok(metadata)
}

pub(crate) fn revalidate_source_entry(entry: &SourceEntry) -> Result<(), CommandError> {
    if source_metadata(&entry.path)?.is_dir() != entry.is_directory {
        return Err(create_error(
            "invalid_source",
            format!("Source type changed: {}", entry.path.display()),
        ));
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn open_source_file(path: &Path) -> io::Result<crate::windows_fs::PinnedFile> {
    crate::windows_fs::open_source_file(path)
}

#[cfg(not(windows))]
pub(crate) fn open_source_file(path: &Path) -> io::Result<File> {
    File::open(path)
}

fn collision_key(name: &str) -> String {
    name.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// Unique archive member name to avoid duplicate entries.
pub(crate) fn get_unique_archive_name(
    seen: &mut HashSet<String>,
    base_name: &str,
    is_dir: bool,
) -> String {
    let mut name = base_name.replace('\\', "/");
    if is_dir && !name.ends_with('/') {
        name.push('/');
    }

    if seen.insert(collision_key(&name)) {
        return name;
    }

    let base = if is_dir {
        &name[..name.len() - 1]
    } else {
        &name
    };

    let mut counter = 1;
    loop {
        let candidate = if is_dir {
            format!("{}({})/", base, counter)
        } else if let Some(pos) = base.rfind('.') {
            let (first, last) = base.split_at(pos);
            format!("{}({}){}", first, counter, last)
        } else {
            format!("{}({})", base, counter)
        };

        if seen.insert(collision_key(&candidate)) {
            return candidate;
        }
        counter += 1;
    }
}

fn enumerate_directory(
    source: &Path,
    relative_prefix: &str,
    seen: &mut HashSet<String>,
    entries: &mut Vec<SourceEntry>,
    cancelled: &AtomicBool,
) -> Result<(), CommandError> {
    if !source_metadata(source)?.is_dir() {
        return Err(create_error(
            "invalid_source",
            format!("Source directory changed: {}", source.display()),
        ));
    }
    #[cfg(windows)]
    let _pinned_directory = crate::windows_fs::Directory::open_root(source).map_err(|error| {
        create_error(
            "invalid_source",
            format!("Cannot securely open source directory: {error}"),
        )
    })?;
    let directory = fs::read_dir(source).map_err(|error| {
        create_error(
            "source_read",
            format!("Cannot read source directory {}: {error}", source.display()),
        )
    })?;
    let mut directory_entries = Vec::new();

    for entry in directory {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        directory_entries.push(entry.map_err(|error| {
            create_error("source_read", format!("Cannot read source entry: {error}"))
        })?);
    }
    directory_entries.sort_by_key(|entry| {
        let name = entry.file_name();
        (name.to_string_lossy().to_lowercase(), name)
    });

    for entry in directory_entries {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let entry_path = entry.path();
        let entry_name = entry.file_name().to_string_lossy().into_owned();
        let entry_metadata = source_metadata(&entry_path)?;
        let archive_path = format!("{relative_prefix}{entry_name}");

        if entry_metadata.is_dir() {
            let unique_dir = get_unique_archive_name(seen, &archive_path, true);
            let child_count = entries.len();
            enumerate_directory(&entry_path, &unique_dir, seen, entries, cancelled)?;
            if entries.len() == child_count {
                entries.push(SourceEntry {
                    path: entry_path,
                    archive_path: unique_dir,
                    is_directory: true,
                });
            }
        } else {
            let unique_file = get_unique_archive_name(seen, &archive_path, false);
            entries.push(SourceEntry {
                path: entry_path,
                archive_path: unique_file,
                is_directory: false,
            });
        }
    }

    Ok(())
}

pub(crate) fn enumerate_sources(
    source_paths: &[PathBuf],
    include_root: bool,
    cancelled: &AtomicBool,
) -> Result<Vec<SourceEntry>, CommandError> {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    for source in source_paths {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let metadata = source_metadata(source)?;
        let source_name = source
            .file_name()
            .ok_or_else(|| create_error("invalid_source", "Source path has no file name."))?
            .to_string_lossy()
            .into_owned();

        if metadata.is_dir() {
            if include_root {
                let unique_dir = get_unique_archive_name(&mut seen, &source_name, true);
                let child_count = entries.len();
                enumerate_directory(source, &unique_dir, &mut seen, &mut entries, cancelled)?;
                if entries.len() == child_count {
                    entries.push(SourceEntry {
                        path: source.to_path_buf(),
                        archive_path: unique_dir,
                        is_directory: true,
                    });
                }
            } else {
                let child_count = entries.len();
                enumerate_directory(source, "", &mut seen, &mut entries, cancelled)?;
                if entries.len() == child_count {
                    let unique_dir = get_unique_archive_name(&mut seen, &source_name, true);
                    entries.push(SourceEntry {
                        path: source.to_path_buf(),
                        archive_path: unique_dir,
                        is_directory: true,
                    });
                }
            }
        } else {
            let unique_file = get_unique_archive_name(&mut seen, &source_name, false);
            entries.push(SourceEntry {
                path: source.to_path_buf(),
                archive_path: unique_file,
                is_directory: false,
            });
        }
    }

    Ok(entries)
}

fn canonical_output_path(output_path: &Path) -> Result<PathBuf, CommandError> {
    let parent = output_path.parent().map_or(Path::new("."), |path| path);
    let parent = parent.canonicalize().map_err(|error| {
        create_error(
            "invalid_output",
            format!("Cannot resolve output directory: {error}"),
        )
    })?;
    let file_name = output_path
        .file_name()
        .ok_or_else(|| create_error("invalid_output", "Output path has no file name."))?;
    Ok(parent.join(file_name))
}

pub(crate) fn validate_sources_and_output(
    source_paths: &[String],
    output_path: &Path,
    overwrite: bool,
) -> Result<(PathBuf, Vec<PathBuf>), CommandError> {
    let output_path = canonical_output_path(output_path)?;
    let mut sources = Vec::with_capacity(source_paths.len());
    #[cfg(windows)]
    let mut pinned_directories = Vec::new();
    #[cfg(windows)]
    let mut pinned_files = Vec::new();

    for source_path in source_paths {
        let source = Path::new(source_path);
        let metadata = source_metadata(source)?;
        #[cfg(windows)]
        if metadata.is_dir() {
            pinned_directories.push(crate::windows_fs::Directory::open_root(source).map_err(
                |error| {
                    create_error(
                        "invalid_source",
                        format!("Cannot securely pin source directory: {error}"),
                    )
                },
            )?);
        } else {
            pinned_files.push(
                crate::windows_fs::open_source_file(source).map_err(|error| {
                    create_error(
                        "invalid_source",
                        format!("Cannot securely pin source file: {error}"),
                    )
                })?,
            );
        }
        let canonical_source = source.canonicalize().map_err(|error| {
            create_error(
                "source_not_found",
                format!("Cannot resolve source {source_path}: {error}"),
            )
        })?;
        sources.push((canonical_source, metadata.is_dir()));
    }

    if sources.iter().any(|(source, is_directory)| {
        output_path == *source || (*is_directory && output_path.starts_with(source))
    }) {
        return Err(create_error(
            "output_inside_source",
            "Output archive must be outside every source path.",
        ));
    }

    match fs::symlink_metadata(&output_path) {
        Ok(meta) => {
            if !overwrite {
                return Err(create_error(
                    "output_exists",
                    "Output archive already exists.",
                ));
            }
            if is_link_or_reparse_point(&meta) {
                return Err(create_error(
                    "invalid_output",
                    "Output path is a link or reparse point and cannot be overwritten.",
                ));
            }
            if meta.is_dir() {
                return Err(create_error(
                    "invalid_output",
                    "Output path is a directory and cannot be overwritten.",
                ));
            }
            if !meta.is_file() {
                return Err(create_error(
                    "invalid_output",
                    "Output path is not a regular file and cannot be overwritten.",
                ));
            }
            Ok((
                output_path,
                sources.into_iter().map(|(source, _)| source).collect(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((
            output_path,
            sources.into_iter().map(|(source, _)| source).collect(),
        )),
        Err(error) => Err(create_error(
            "invalid_output",
            format!("Cannot inspect output path: {error}"),
        )),
    }
}

pub(crate) fn create_temporary_archive(
    output_path: &Path,
) -> Result<(PathBuf, File), CommandError> {
    let parent = output_path
        .parent()
        .ok_or_else(|| create_error("invalid_output", "Output path has no parent directory."))?;
    let output_name = output_path
        .file_name()
        .ok_or_else(|| create_error("invalid_output", "Output path has no file name."))?
        .to_string_lossy();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| create_error("temp_create_failed", format!("Cannot get time: {error}")))?
        .as_nanos();

    for attempt in 0_u128.. {
        let temp_path = parent.join(format!(
            "{output_name}.archi-part-{}-{}",
            std::process::id(),
            timestamp.saturating_add(attempt)
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(create_error(
                    "temp_create_failed",
                    format!("Cannot create temporary archive: {error}"),
                ));
            }
        }
    }

    Err(create_error(
        "temp_create_failed",
        "Cannot choose a unique temporary archive path.",
    ))
}

pub(crate) fn progress_percentage(processed: u64, total: u64) -> f32 {
    if total == 0 {
        100.0
    } else {
        ((processed as f64 * 100.0 / total as f64).min(100.0)) as f32
    }
}

pub(crate) fn cleanup_temp(temp_path: &Path, error: &mut CommandError) {
    if let Err(cleanup_error) = fs::remove_file(temp_path) {
        if cleanup_error.kind() != std::io::ErrorKind::NotFound {
            error.message.push_str(&format!(
                " Cleanup failed for temporary archive: {cleanup_error}."
            ));
        }
    }
}

#[cfg(windows)]
pub(crate) fn publish_temp_archive(
    temp_path: &Path,
    output_path: &Path,
    overwrite: bool,
) -> std::io::Result<()> {
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let flags = if overwrite {
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH
    } else {
        MOVEFILE_WRITE_THROUGH
    };

    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let existing: Vec<_> = temp_path.as_os_str().encode_wide().chain(Some(0)).collect();
    let new: Vec<_> = output_path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let moved = unsafe { MoveFileExW(existing.as_ptr(), new.as_ptr(), flags) };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub(crate) fn publish_temp_archive(
    temp_path: &Path,
    output_path: &Path,
    overwrite: bool,
) -> std::io::Result<()> {
    if overwrite && output_path.exists() {
        fs::remove_file(output_path)?;
    }
    fs::hard_link(temp_path, output_path)?;
    fs::remove_file(temp_path)
}

/// Strip trailing slash for tar member paths.
pub(crate) fn member_path_for_tar(archive_path: &str) -> String {
    archive_path
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string()
}

/// Read wrapper that aborts when cancel is set.
pub(crate) struct CancellableRead<'a, R> {
    inner: R,
    cancelled: &'a AtomicBool,
}

impl<'a, R> CancellableRead<'a, R> {
    pub fn new(inner: R, cancelled: &'a AtomicBool) -> Self {
        Self { inner, cancelled }
    }
}

impl<R: Read> Read for CancellableRead<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        self.inner.read(buf)
    }
}
