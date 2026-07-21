//! ZIP archive creation.

use crate::create_common::{
    cancelled_error, cleanup_temp, create_error, create_temporary_archive, enumerate_sources,
    open_source_file, progress_percentage, publish_temp_archive, revalidate_source_entry,
    validate_sources_and_output, ProgressGate, BUFFER_SIZE,
};
use crate::models::{
    CommandError, CompressionPreset, CreateOptions, OperationProgress, OperationSummary,
};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use zip::write::FileOptions;
use zip::ZipWriter;

fn zip_file_options(preset: CompressionPreset) -> FileOptions {
    match preset {
        CompressionPreset::Store => {
            FileOptions::default().compression_method(zip::CompressionMethod::Stored)
        }
        CompressionPreset::Fast => FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(1)),
        CompressionPreset::Normal => FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(6)),
        CompressionPreset::Max => FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(9)),
    }
}

/// Creates a ZIP archive atomically from the given source files and directories.
pub fn create_zip_archive(
    source_paths: &[String],
    output_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    options: &CreateOptions,
    mut emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(create_error("invalid_operation", "Operation ID is empty."));
    }
    if source_paths.is_empty() {
        return Err(create_error("invalid_source", "No source files specified."));
    }

    let (output_path, source_paths) =
        validate_sources_and_output(source_paths, output_path, options.overwrite)?;
    let entries = enumerate_sources(&source_paths, options.include_root, cancelled)?;
    let total_files = entries.len() as u64;
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let (temp_path, temp_file) = create_temporary_archive(&output_path)?;
    let result = (|| -> Result<OperationSummary, CommandError> {
        let mut zip = ZipWriter::new(temp_file);
        let mut processed_files = 0_u64;
        let mut progress_gate = ProgressGate::new();
        let file_opts = zip_file_options(options.compression);

        for entry in &entries {
            if cancelled.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }
            revalidate_source_entry(entry)?;

            // First entry always; mid-entries at most every PROGRESS_INTERVAL; final 100% outside loop.
            if progress_gate.should_emit() {
                emit(OperationProgress {
                    operation_id: operation_id.into(),
                    extracted_files: processed_files,
                    total_files,
                    current_file: entry.archive_path.clone(),
                    percentage: progress_percentage(processed_files, total_files),
                });
            }

            let mut source = if entry.is_directory {
                zip.add_directory(&entry.archive_path, FileOptions::default())
                    .map_err(|error| {
                        create_error("write_failed", format!("Cannot add directory: {error}"))
                    })?;
                None
            } else {
                let source = open_source_file(&entry.path).map_err(|error| {
                    create_error(
                        "source_read",
                        format!("Cannot open source {}: {error}", entry.path.display()),
                    )
                })?;
                zip.start_file(&entry.archive_path, file_opts)
                    .map_err(|error| {
                        create_error("write_failed", format!("Cannot start ZIP file: {error}"))
                    })?;
                Some(source)
            };

            if let Some(source) = source.as_mut() {
                let mut buffer = [0_u8; BUFFER_SIZE];
                loop {
                    if cancelled.load(Ordering::Relaxed) {
                        return Err(cancelled_error());
                    }
                    let read = source.read(&mut buffer).map_err(|error| {
                        create_error(
                            "source_read",
                            format!("Cannot read source {}: {error}", entry.path.display()),
                        )
                    })?;
                    if read == 0 {
                        break;
                    }
                    zip.write_all(&buffer[..read]).map_err(|error| {
                        create_error("write_failed", format!("Cannot write ZIP data: {error}"))
                    })?;
                }
            }
            processed_files += 1;
        }

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let temp_file = zip.finish().map_err(|error| {
            create_error(
                "write_failed",
                format!("Cannot finalize ZIP archive: {error}"),
            )
        })?;
        temp_file.sync_all().map_err(|error| {
            create_error("write_failed", format!("Cannot sync ZIP archive: {error}"))
        })?;
        drop(temp_file);

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        publish_temp_archive(&temp_path, &output_path, options.overwrite).map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                create_error(
                    "output_exists",
                    "Output archive appeared before finalization.",
                )
            } else {
                create_error(
                    "finalize_failed",
                    format!("Cannot finalize output archive: {error}"),
                )
            }
        })?;

        Ok(OperationSummary {
            operation_id: operation_id.into(),
            extracted_files: processed_files,
            total_files,
            skipped_files: 0,
            destination: output_path.to_string_lossy().into_owned(),
        })
    })();

    match result {
        Ok(summary) => {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: summary.extracted_files,
                total_files: summary.total_files,
                current_file: "Completed".into(),
                percentage: 100.0,
            });
            Ok(summary)
        }
        Err(mut error) => {
            cleanup_temp(&temp_path, &mut error);
            Err(error)
        }
    }
}
