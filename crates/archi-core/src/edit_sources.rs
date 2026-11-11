//! Shared edit source traversal. Revalidate links and pin directory ancestors on Windows.
use crate::edit_plan::normalize_and_validate;
use crate::models::CommandError;
use crate::security::is_link_or_reparse_point;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) struct DiskSourceEntry {
    pub path: PathBuf,
    pub archive_path: String,
    pub is_directory: bool,
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), CommandError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(CommandError::new(
            "cancelled",
            "Archive edit was cancelled.",
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn source_metadata(path: &Path) -> Result<fs::Metadata, CommandError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        CommandError::new(
            if error.kind() == io::ErrorKind::NotFound {
                "source_not_found"
            } else {
                "source_read"
            },
            format!("Cannot inspect source {}: {error}", path.display()),
        )
    })?;
    if is_link_or_reparse_point(&metadata) {
        return Err(CommandError::new(
            "invalid_source",
            format!("Source links are not supported: {}", path.display()),
        ));
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(CommandError::new(
            "invalid_source",
            format!("Source is not a file or directory: {}", path.display()),
        ));
    }
    Ok(metadata)
}

pub(crate) fn normalize_archive_parent(parent: &str) -> Result<String, CommandError> {
    if parent.is_empty() || parent == "/" {
        return Ok(String::new());
    }
    normalize_and_validate(parent)
}

fn join_archive_path(parent: &str, relative: &str) -> Result<String, CommandError> {
    let relative = normalize_and_validate(relative)?;
    normalize_and_validate(&if parent.is_empty() {
        relative
    } else {
        format!("{parent}/{relative}")
    })
}

pub(crate) fn reject_add_sources_contain_archive(
    sources: &[String],
    archive: &Path,
) -> Result<(), CommandError> {
    let archive = archive.canonicalize().map_err(|e| {
        CommandError::new(
            "invalid_archive",
            format!("Cannot resolve archive path: {e}"),
        )
    })?;
    for raw in sources {
        let source = Path::new(raw);
        let metadata = source_metadata(source)?;
        let source = source.canonicalize().map_err(|e| {
            CommandError::new(
                if e.kind() == io::ErrorKind::NotFound {
                    "source_not_found"
                } else {
                    "invalid_source"
                },
                format!("Cannot resolve source {raw}: {e}"),
            )
        })?;
        if archive == source || (metadata.is_dir() && archive.starts_with(&source)) {
            return Err(CommandError::new(
                "output_inside_source",
                "Open archive must be outside every add source path.",
            ));
        }
    }
    Ok(())
}

fn enumerate_directory(
    source: &Path,
    prefix: &str,
    entries: &mut Vec<DiskSourceEntry>,
    cancelled: &AtomicBool,
) -> Result<(), CommandError> {
    check_cancelled(cancelled)?;
    if !source_metadata(source)?.is_dir() {
        return Err(CommandError::new(
            "invalid_source",
            format!("Source directory changed: {}", source.display()),
        ));
    }
    #[cfg(windows)]
    let _pinned = crate::windows_fs::Directory::open_root(source).map_err(|e| {
        CommandError::new(
            "invalid_source",
            format!("Cannot securely open source directory: {e}"),
        )
    })?;
    let directory = fs::read_dir(source).map_err(|e| {
        CommandError::new(
            "source_read",
            format!("Cannot read source directory {}: {e}", source.display()),
        )
    })?;
    let mut children = Vec::new();
    for child in directory {
        check_cancelled(cancelled)?;
        children.push(child.map_err(|e| {
            CommandError::new("source_read", format!("Cannot read source entry: {e}"))
        })?);
    }
    children.sort_by_cached_key(|entry| {
        let name = entry.file_name();
        (name.to_string_lossy().to_lowercase(), name)
    });
    if children.is_empty() {
        entries.push(DiskSourceEntry {
            path: source.to_path_buf(),
            archive_path: prefix.into(),
            is_directory: true,
        });
    }
    for child in children {
        check_cancelled(cancelled)?;
        let path = child.path();
        let metadata = source_metadata(&path)?;
        let archive_path = join_archive_path(prefix, &child.file_name().to_string_lossy())?;
        if metadata.is_dir() {
            enumerate_directory(&path, &archive_path, entries, cancelled)?;
        } else {
            entries.push(DiskSourceEntry {
                path,
                archive_path,
                is_directory: false,
            });
        }
    }
    Ok(())
}

pub(crate) fn enumerate_add_sources(
    sources: &[String],
    parent: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<DiskSourceEntry>, CommandError> {
    if sources.is_empty() {
        return Err(CommandError::new(
            "invalid_source",
            "No source files specified.",
        ));
    }
    let parent = normalize_archive_parent(parent)?;
    let mut entries = Vec::new();
    for raw in sources {
        check_cancelled(cancelled)?;
        let path = PathBuf::from(raw);
        let metadata = source_metadata(&path)?;
        let name = path
            .file_name()
            .ok_or_else(|| CommandError::new("invalid_source", "Source path has no file name."))?;
        let archive_path = join_archive_path(&parent, &name.to_string_lossy())?;
        if metadata.is_dir() {
            enumerate_directory(&path, &archive_path, &mut entries, cancelled)?;
        } else {
            entries.push(DiskSourceEntry {
                path,
                archive_path,
                is_directory: false,
            });
        }
    }
    Ok(entries)
}
