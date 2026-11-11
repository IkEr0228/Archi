//! Single-stream XZ open/list/extract (not tar.xz).
use crate::extraction::ConflictResolver;
use crate::models::{ArchiveInfo, CommandError, OperationProgress, OperationSummary};
use crate::security::validate_entry_path;
use crate::stream_extract;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use xz2::read::XzDecoder;

fn open_decoder(path: &Path) -> Result<XzDecoder<File>, CommandError> {
    let file = File::open(path)
        .map_err(|e| CommandError::new("invalid_archive", format!("Cannot open xz: {e}")))?;
    Ok(XzDecoder::new(file))
}

/// Logical entry name for a single xz payload (no embedded filename in format).
pub fn xz_entry_name(archive_path: &Path) -> Result<String, CommandError> {
    let file_name = archive_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "data".into());
    let lower = file_name.to_ascii_lowercase();
    let stem = if lower.ends_with(".txz") {
        file_name[..file_name.len() - 4].to_string()
    } else if lower.ends_with(".xz") {
        file_name[..file_name.len() - 3].to_string()
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

pub fn open_xz(path: &Path) -> Result<ArchiveInfo, CommandError> {
    if !path.is_file() {
        return Err(CommandError::new(
            "not_found",
            "File not found or is not a file.",
        ));
    }
    let name = xz_entry_name(path)?;
    let size = stream_extract::decoded_size(open_decoder(path)?)?;
    Ok(stream_extract::info(path, name, "xz", "XZ", size))
}

pub fn extract_xz(
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
    let name = xz_entry_name(path)?;
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
