//! TAR / TAR.GZ / TAR.BZ2 / TAR.XZ archive creation.

use crate::create_common::{
    cancelled_error, cleanup_temp, create_error, create_temporary_archive, enumerate_sources,
    member_path_for_tar, open_source_file, progress_percentage, publish_temp_archive,
    revalidate_source_entry, validate_sources_and_output, CancellableRead, ProgressGate,
};
use crate::models::{
    CommandError, CompressionPreset, CreateFormat, CreateOptions, OperationProgress,
    OperationSummary,
};
use bzip2::write::BzEncoder;
use bzip2::Compression as BzCompression;
use flate2::write::GzEncoder;
use flate2::Compression as GzCompression;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use tar::{Builder, EntryType, Header};
use xz2::write::XzEncoder;

fn gz_level(preset: CompressionPreset) -> GzCompression {
    match preset {
        CompressionPreset::Store => GzCompression::new(0),
        CompressionPreset::Fast => GzCompression::new(1),
        CompressionPreset::Normal => GzCompression::new(6),
        CompressionPreset::Max => GzCompression::new(9),
    }
}

fn bz_level(preset: CompressionPreset) -> BzCompression {
    // bzip2 has no true store; map Store → Fast (level 1).
    match preset {
        CompressionPreset::Store | CompressionPreset::Fast => BzCompression::new(1),
        CompressionPreset::Normal => BzCompression::new(6),
        CompressionPreset::Max => BzCompression::new(9),
    }
}

fn xz_level(preset: CompressionPreset) -> u32 {
    match preset {
        CompressionPreset::Store => 0,
        CompressionPreset::Fast => 1,
        CompressionPreset::Normal => 6,
        CompressionPreset::Max => 9,
    }
}

fn append_entries<W: Write>(
    builder: &mut Builder<W>,
    entries: &[crate::create_common::SourceEntry],
    operation_id: &str,
    cancelled: &AtomicBool,
    total_files: u64,
    mut emit: impl FnMut(OperationProgress),
) -> Result<u64, CommandError> {
    let mut processed_files = 0_u64;
    let mut progress_gate = ProgressGate::new();

    for entry in entries {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        revalidate_source_entry(entry)?;

        // First entry always; mid-entries at most every PROGRESS_INTERVAL; final 100% at command end.
        if progress_gate.should_emit() {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: processed_files,
                total_files,
                current_file: entry.archive_path.clone(),
                percentage: progress_percentage(processed_files, total_files),
            });
        }

        let member = member_path_for_tar(&entry.archive_path);
        if member.is_empty() {
            return Err(create_error(
                "invalid_source",
                "Archive member path is empty.",
            ));
        }

        let mut header = Header::new_gnu();
        if entry.is_directory {
            header.set_entry_type(EntryType::Directory);
            header.set_path(&member).map_err(|error| {
                create_error(
                    "write_failed",
                    format!("Cannot set tar directory path: {error}"),
                )
            })?;
            header.set_size(0);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, std::io::empty()).map_err(|error| {
                create_error(
                    "write_failed",
                    format!("Cannot write tar directory: {error}"),
                )
            })?;
        } else {
            let meta = std::fs::metadata(&entry.path).map_err(|error| {
                create_error(
                    "source_read",
                    format!("Cannot read source size {}: {error}", entry.path.display()),
                )
            })?;
            let size = meta.len();
            header.set_entry_type(EntryType::Regular);
            header.set_path(&member).map_err(|error| {
                create_error("write_failed", format!("Cannot set tar file path: {error}"))
            })?;
            header.set_size(size);
            header.set_mode(0o644);
            header.set_cksum();

            let source = open_source_file(&entry.path).map_err(|error| {
                create_error(
                    "source_read",
                    format!("Cannot open source {}: {error}", entry.path.display()),
                )
            })?;
            let mut reader = CancellableRead::new(source, cancelled);
            // Stream via append_data; cancel surfaces as Interrupted → map to cancelled.
            if let Err(error) = builder.append_data(&mut header, &member, &mut reader) {
                if cancelled.load(Ordering::Relaxed)
                    || error.to_string().contains("cancelled")
                    || error.kind() == std::io::ErrorKind::Interrupted
                {
                    return Err(cancelled_error());
                }
                return Err(create_error(
                    "write_failed",
                    format!("Cannot write tar member {}: {error}", entry.path.display()),
                ));
            }
        }

        processed_files += 1;
    }

    Ok(processed_files)
}

fn finish_plain_tar(builder: Builder<File>) -> Result<File, CommandError> {
    let file = builder.into_inner().map_err(|error| {
        create_error(
            "write_failed",
            format!("Cannot finalize tar archive: {error}"),
        )
    })?;
    file.sync_all().map_err(|error| {
        create_error("write_failed", format!("Cannot sync tar archive: {error}"))
    })?;
    Ok(file)
}

fn finish_gz_tar(builder: Builder<GzEncoder<File>>) -> Result<File, CommandError> {
    let encoder = builder.into_inner().map_err(|error| {
        create_error("write_failed", format!("Cannot finalize tar.gz: {error}"))
    })?;
    let file = encoder.finish().map_err(|error| {
        create_error(
            "write_failed",
            format!("Cannot finish gzip stream: {error}"),
        )
    })?;
    file.sync_all()
        .map_err(|error| create_error("write_failed", format!("Cannot sync tar.gz: {error}")))?;
    Ok(file)
}

fn finish_bz_tar(builder: Builder<BzEncoder<File>>) -> Result<File, CommandError> {
    let encoder = builder.into_inner().map_err(|error| {
        create_error("write_failed", format!("Cannot finalize tar.bz2: {error}"))
    })?;
    let file = encoder.finish().map_err(|error| {
        create_error(
            "write_failed",
            format!("Cannot finish bzip2 stream: {error}"),
        )
    })?;
    file.sync_all()
        .map_err(|error| create_error("write_failed", format!("Cannot sync tar.bz2: {error}")))?;
    Ok(file)
}

fn finish_xz_tar(builder: Builder<XzEncoder<File>>) -> Result<File, CommandError> {
    let encoder = builder.into_inner().map_err(|error| {
        create_error("write_failed", format!("Cannot finalize tar.xz: {error}"))
    })?;
    let file = encoder.finish().map_err(|error| {
        create_error("write_failed", format!("Cannot finish xz stream: {error}"))
    })?;
    file.sync_all()
        .map_err(|error| create_error("write_failed", format!("Cannot sync tar.xz: {error}")))?;
    Ok(file)
}

/// Creates a TAR-family archive (plain or compressed) atomically.
pub fn create_tar_archive(
    source_paths: &[String],
    output_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    options: &CreateOptions,
    mut emit: impl FnMut(OperationProgress),
) -> Result<OperationSummary, CommandError> {
    let format = options.format;
    if !matches!(
        format,
        CreateFormat::Tar | CreateFormat::TarGz | CreateFormat::TarBz2 | CreateFormat::TarXz
    ) {
        return Err(create_error(
            "invalid_operation",
            format!("Not a TAR create format: {}", format.as_str()),
        ));
    }
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
        let processed_files = match format {
            CreateFormat::Tar => {
                let mut builder = Builder::new(temp_file);
                let processed = append_entries(
                    &mut builder,
                    &entries,
                    operation_id,
                    cancelled,
                    total_files,
                    &mut emit,
                )?;
                drop(finish_plain_tar(builder)?);
                processed
            }
            CreateFormat::TarGz => {
                let encoder = GzEncoder::new(temp_file, gz_level(options.compression));
                let mut builder = Builder::new(encoder);
                let processed = append_entries(
                    &mut builder,
                    &entries,
                    operation_id,
                    cancelled,
                    total_files,
                    &mut emit,
                )?;
                drop(finish_gz_tar(builder)?);
                processed
            }
            CreateFormat::TarBz2 => {
                let encoder = BzEncoder::new(temp_file, bz_level(options.compression));
                let mut builder = Builder::new(encoder);
                let processed = append_entries(
                    &mut builder,
                    &entries,
                    operation_id,
                    cancelled,
                    total_files,
                    &mut emit,
                )?;
                drop(finish_bz_tar(builder)?);
                processed
            }
            CreateFormat::TarXz => {
                let encoder = XzEncoder::new(temp_file, xz_level(options.compression));
                let mut builder = Builder::new(encoder);
                let processed = append_entries(
                    &mut builder,
                    &entries,
                    operation_id,
                    cancelled,
                    total_files,
                    &mut emit,
                )?;
                drop(finish_xz_tar(builder)?);
                processed
            }
            CreateFormat::Zip | CreateFormat::SevenZ => {
                unreachable!("ZIP/7z handled elsewhere")
            }
        };

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
