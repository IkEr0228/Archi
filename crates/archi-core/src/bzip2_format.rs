//! Single-stream BZIP2 open/list/extract (not tar.bz2).
use crate::extraction::ConflictResolver;
use crate::models::{ArchiveInfo, CommandError, OperationProgress, OperationSummary};
use crate::security::validate_entry_path;
use crate::stream_extract;
use bzip2::read::BzDecoder;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn open_decoder(path: &Path) -> Result<BzDecoder<File>, CommandError> {
    let file = File::open(path)
        .map_err(|e| CommandError::new("invalid_archive", format!("Cannot open bzip2: {e}")))?;
    Ok(BzDecoder::new(file))
}

/// Logical entry name for a single bzip2 payload (no embedded filename in format).
pub fn bzip2_entry_name(archive_path: &Path) -> Result<String, CommandError> {
    let file_name = archive_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "data".into());
    let lower = file_name.to_ascii_lowercase();
    let stem = if lower.ends_with(".tbz2") {
        file_name[..file_name.len() - 5].to_string()
    } else if lower.ends_with(".bz2") || lower.ends_with(".tbz") {
        file_name[..file_name.len() - 4].to_string()
    } else {
        format!("{file_name}.out")
    };
    let stem = if stem.is_empty() { "data".into() } else { stem };
    validate_entry_path(&stem).map_err(|message| CommandError {
        code: "invalid_entry".into(),
        message,
        path: Some(stem.clone()),
    })?;
    Ok(stem)
}

pub fn open_bzip2(path: &Path) -> Result<ArchiveInfo, CommandError> {
    if !path.is_file() {
        return Err(CommandError::new(
            "not_found",
            "File not found or is not a file.",
        ));
    }
    let name = bzip2_entry_name(path)?;
    let size = stream_extract::decoded_size(open_decoder(path)?)?;
    Ok(stream_extract::info(path, name, "bzip2", "Bzip2", size))
}

pub fn extract_bzip2(
    path: &Path,
    destination: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    selected_paths: Option<&[String]>,
    conflict_resolver: &dyn ConflictResolver,
    emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if !path.is_file() {
        return Err(CommandError::new(
            "not_found",
            "Source archive does not exist.",
        ));
    }
    let name = bzip2_entry_name(path)?;
    stream_extract::extract(
        destination,
        &name,
        operation_id,
        cancelled,
        selected_paths,
        conflict_resolver,
        emit,
        || Ok(Box::new(open_decoder(path)?)),
    )
}
