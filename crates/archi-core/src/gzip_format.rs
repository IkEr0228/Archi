//! Single-stream GZIP open/list/extract (not tar.gz).
use crate::extraction::ConflictResolver;
use crate::models::{ArchiveInfo, CommandError, OperationProgress, OperationSummary};
use crate::security::validate_entry_path;
use crate::stream_extract;
use flate2::read::GzDecoder;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn open_decoder(path: &Path) -> Result<GzDecoder<File>, CommandError> {
    let file = File::open(path)
        .map_err(|e| CommandError::new("invalid_archive", format!("Cannot open gzip: {e}")))?;
    Ok(GzDecoder::new(file))
}

/// Gzip ISIZE is uncompressed length modulo 2^32, not a decoding limit.
fn gzip_isize_from_trailer(path: &Path, on_disk: u64) -> Result<u64, CommandError> {
    if on_disk < 8 {
        return Ok(0);
    }
    let mut file = File::open(path)
        .map_err(|e| CommandError::new("invalid_archive", format!("Cannot open gzip: {e}")))?;
    file.seek(SeekFrom::End(-4)).map_err(|e| {
        CommandError::new(
            "invalid_archive",
            format!("Cannot seek gzip ISIZE trailer: {e}"),
        )
    })?;
    let mut buffer = [0_u8; 4];
    file.read_exact(&mut buffer).map_err(|e| {
        CommandError::new(
            "invalid_archive",
            format!("Cannot read gzip ISIZE trailer: {e}"),
        )
    })?;
    Ok(u32::from_le_bytes(buffer) as u64)
}

/// Logical entry name for a single gzip payload.
pub fn gzip_entry_name(archive_path: &Path) -> Result<String, CommandError> {
    let decoder = open_decoder(archive_path)?;
    if let Some(name) = decoder.header().and_then(|h| h.filename()) {
        let raw = String::from_utf8_lossy(name).replace('\\', "/");
        let base = raw
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(raw.as_str())
            .trim_matches('/');
        if !base.is_empty() && validate_entry_path(base).is_ok() {
            return Ok(base.to_string());
        }
    }
    let file_name = archive_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "data".into());
    let stem = if let Some(stripped) = file_name
        .strip_suffix(".gz")
        .or_else(|| file_name.strip_suffix(".GZ"))
    {
        if stripped.is_empty() {
            "data".into()
        } else {
            stripped.to_string()
        }
    } else {
        format!("{file_name}.out")
    };
    validate_entry_path(&stem).map_err(|message| CommandError {
        code: "invalid_entry".into(),
        message,
        path: Some(stem.clone()),
    })?;
    Ok(stem)
}

pub fn open_gzip(path: &Path) -> Result<ArchiveInfo, CommandError> {
    if !path.is_file() {
        return Err(CommandError::new(
            "not_found",
            "File not found or is not a file.",
        ));
    }
    let name = gzip_entry_name(path)?;
    let on_disk = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let size = gzip_isize_from_trailer(path, on_disk)?;
    Ok(stream_extract::info(path, name, "gzip", "Gzip", size))
}

pub fn extract_gzip(
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
    let name = gzip_entry_name(path)?;
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
