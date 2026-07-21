use serde::Serialize;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

pub const MAX_PATH_DEPTH: usize = 128;

#[derive(Debug, Clone, Copy)]
pub struct ArchiveRiskInput {
    pub entry_count: usize,
    pub total_uncompressed: u64,
    pub total_compressed: u64,
    pub largest_entry: u64,
    pub deepest_path: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ArchiveWarning {
    pub code: String,
    pub message: String,
}

fn is_windows_device_component(component: &str) -> bool {
    let stem = component
        .split_once('.')
        .map_or(component, |(stem, _)| stem);
    let stem = stem.trim_end_matches(|character| character == '.' || character == ' ');
    let upper = stem.to_ascii_uppercase();
    let bytes = upper.as_bytes();

    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "CLOCK$"
            | "COM\u{00B9}"
            | "COM\u{00B2}"
            | "COM\u{00B3}"
            | "LPT\u{00B9}"
            | "LPT\u{00B2}"
            | "LPT\u{00B3}"
    ) || (bytes.len() == 4
        && matches!(&bytes[..3], b"COM" | b"LPT")
        && matches!(bytes[3], b'1'..=b'9'))
}

pub fn validate_entry_path(entry: &str) -> Result<PathBuf, String> {
    if entry.is_empty() || entry.contains('\0') {
        return Err("Archive entry path is empty or malformed.".into());
    }

    let normalized = entry.replace('\\', "/");
    if normalized.starts_with('/') || normalized.starts_with("//") || normalized.contains(':') {
        return Err("Archive entry path is absolute or Windows-prefixed.".into());
    }

    let trimmed = normalized.trim_end_matches('/');
    let parts: Vec<&str> = trimmed.split('/').collect();
    if parts.is_empty()
        || parts.len() > MAX_PATH_DEPTH
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || part.ends_with('.')
                || part.ends_with(' ')
                || is_windows_device_component(part)
        })
    {
        return Err("Archive entry path contains invalid components.".into());
    }

    Ok(parts.iter().collect())
}

/// Resolve an archive entry under `root` without re-canonicalizing `root`.
///
/// `root` must already be canonical (caller responsibility). Hot extract paths
/// that canonicalize the destination once should call this per entry.
pub fn safe_destination_path_under_canonical(root: &Path, entry: &str) -> Result<PathBuf, String> {
    let relative = validate_entry_path(entry)?;
    let candidate = root.join(relative);
    let mut current = root.to_path_buf();

    // Preflight defense; Windows extraction repeats this with handle-relative no-reparse writes.
    let components: Vec<_> = candidate
        .strip_prefix(root)
        .map_err(|_| "Entry escaped destination.".to_string())?
        .components()
        .collect();
    for (index, part) in components.iter().enumerate() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if is_link_or_reparse_point(&metadata) => {
                return Err("Archive entry crosses a symbolic link.".into());
            }
            Ok(metadata) if index + 1 < components.len() && !metadata.is_dir() => {
                return Err("Archive entry has a non-directory parent.".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Cannot inspect destination: {error}")),
        }
    }

    Ok(candidate)
}

/// Resolve an archive entry under `root`, canonicalizing `root` once first.
/// Prefer [`safe_destination_path_under_canonical`] when the root is already
/// canonical (extract hot paths).
pub fn safe_destination_path(root: &Path, entry: &str) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("Cannot resolve destination: {error}"))?;
    safe_destination_path_under_canonical(&root, entry)
}

pub(crate) fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        metadata.file_attributes() & 0x400 != 0
    }

    #[cfg(not(windows))]
    {
        false
    }
}

pub fn assess_archive(input: ArchiveRiskInput) -> Vec<ArchiveWarning> {
    let mut warnings = Vec::new();
    let mut push = |code: &str, message: &str| {
        warnings.push(ArchiveWarning {
            code: code.into(),
            message: message.into(),
        });
    };

    if input.entry_count > 1_000_000 {
        push(
            "entry_count",
            "Archive contains more than 1,000,000 entries.",
        );
    }
    if input.deepest_path > MAX_PATH_DEPTH {
        push(
            "path_depth",
            "Archive contains paths deeper than 128 components.",
        );
    }
    if input.largest_entry > 100 * 1024 * 1024 * 1024 {
        push(
            "entry_size",
            "Archive contains an entry larger than 100 GiB.",
        );
    }
    if input.total_uncompressed > 1024 * 1024 * 1024
        && input.total_compressed > 0
        && (input.total_uncompressed as u128) > (input.total_compressed as u128) * 1_000
    {
        push("expansion_ratio", "Archive expansion ratio exceeds 1000:1.");
    }

    warnings
}
