use crate::models::CommandError;
use crate::security::is_link_or_reparse_point;
use std::fs;
use std::path::Path;

/// Ensures a single leaf directory exists under an already-existing parent.
/// Does not create intermediate parents — parent must already resolve as a directory.
pub fn ensure_directory_path(path: &Path) -> Result<String, CommandError> {
    if path.as_os_str().is_empty() {
        return Err(CommandError::new(
            "invalid_destination",
            "Destination path is empty.",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        CommandError::new(
            "invalid_destination",
            "Destination has no parent directory.",
        )
    })?;
    if parent.as_os_str().is_empty() {
        return Err(CommandError::new(
            "invalid_destination",
            "Destination parent is invalid.",
        ));
    }
    let parent_meta = fs::symlink_metadata(parent).map_err(|e| {
        CommandError::new(
            "invalid_destination",
            format!("Cannot resolve destination parent: {e}"),
        )
    })?;
    if is_link_or_reparse_point(&parent_meta) || !parent_meta.is_dir() {
        return Err(CommandError::new(
            "invalid_destination",
            "Destination parent is not a usable directory.",
        ));
    }
    let parent = parent.canonicalize().map_err(|e| {
        CommandError::new(
            "invalid_destination",
            format!("Cannot resolve destination parent: {e}"),
        )
    })?;
    if !parent.is_dir() {
        return Err(CommandError::new(
            "invalid_destination",
            "Destination parent is not a directory.",
        ));
    }
    let name = path.file_name().ok_or_else(|| {
        CommandError::new("invalid_destination", "Destination leaf name is missing.")
    })?;
    let candidate = parent.join(name);
    if candidate.exists() {
        let meta = fs::symlink_metadata(&candidate).map_err(|e| {
            CommandError::new(
                "invalid_destination",
                format!("Cannot inspect destination: {e}"),
            )
        })?;
        if is_link_or_reparse_point(&meta) || !meta.is_dir() {
            return Err(CommandError::new(
                "invalid_destination",
                "Destination exists and is not a usable directory.",
            ));
        }
    } else {
        fs::create_dir(&candidate).map_err(|e| {
            CommandError::new(
                "invalid_destination",
                format!("Cannot create destination directory: {e}"),
            )
        })?;
    }
    let canonical = candidate.canonicalize().map_err(|e| {
        CommandError::new(
            "invalid_destination",
            format!("Cannot resolve created destination: {e}"),
        )
    })?;
    if !canonical.starts_with(&parent) {
        return Err(CommandError::new(
            "invalid_destination",
            "Destination escaped parent directory.",
        ));
    }
    Ok(canonical.to_string_lossy().into_owned())
}
