//! TAR family stream-rebuild edit (plain / gz / bz2 / xz).
//! Plan Keep/Rename/Drop/New → sequential decode → re-encode to temp → atomic publish.
//! No full work-tree extract for pure metadata ops (rename/delete/move/mkdir).

use crate::create_common::{
    cleanup_temp, create_temporary_archive, member_path_for_tar, open_source_file,
    progress_percentage, publish_temp_archive, CancellableRead, ProgressGate,
};
use crate::edit_plan::{
    plan_add_paths, plan_create_folder, plan_delete, plan_move, plan_rename, plan_replace_file,
    RebuildMember, SourceMember,
};
use crate::edit_sources::source_metadata;
use crate::format_detect::{detect_format, ArchiveFormat};
use crate::models::{CommandError, CompressionPreset, EditOptions, EditSummary, OperationProgress};
use crate::security::validate_entry_path;
use ahash::AHashMap;
use bzip2::read::BzDecoder;
use bzip2::write::BzEncoder;
use bzip2::Compression as BzCompression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression as GzCompression;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use tar::{Archive, Builder, EntryType, Header};
use xz2::read::XzDecoder;
use xz2::write::XzEncoder;

use crate::io_perf::IO_BUFFER_SIZE as BUFFER_SIZE;

fn edit_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn cancelled_error() -> CommandError {
    edit_error("cancelled", "Archive edit was cancelled.")
}

fn normalize_member_name(raw: &str) -> Result<String, CommandError> {
    let mut normalized = raw.replace('\\', "/");
    while normalized.starts_with("./") {
        normalized = normalized[2..].to_string();
    }
    normalized = normalized.trim_matches('/').to_string();
    if normalized.is_empty() {
        return Err(edit_error("invalid_entry", "Archive entry path is empty."));
    }
    validate_entry_path(&normalized).map_err(|message| CommandError {
        code: "invalid_entry".into(),
        message,
        path: Some(normalized.clone()),
    })?;
    Ok(normalized)
}

fn require_tar_family(path: &Path) -> Result<ArchiveFormat, CommandError> {
    if !path.is_file() {
        return Err(edit_error(
            "not_found",
            format!("Archive not found: {}", path.display()),
        ));
    }
    let fmt = detect_format(path)?;
    match fmt {
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz => Ok(fmt),
        other => Err(edit_error(
            "unsupported_operation",
            format!(
                "TAR edit requires a TAR-family archive, got {}.",
                other.as_str()
            ),
        )),
    }
}

/// Compression for outer stream. Plain TAR is always Store; others honor EditOptions or Normal.
fn edit_compression(fmt: ArchiveFormat, options: &EditOptions) -> CompressionPreset {
    match fmt {
        ArchiveFormat::Tar => CompressionPreset::Store,
        _ => options.compression.unwrap_or(CompressionPreset::Normal),
    }
}

fn gz_level(preset: CompressionPreset) -> GzCompression {
    match preset {
        CompressionPreset::Store => GzCompression::new(0),
        CompressionPreset::Fast => GzCompression::new(1),
        CompressionPreset::Normal => GzCompression::new(6),
        CompressionPreset::Max => GzCompression::new(9),
    }
}

fn bz_level(preset: CompressionPreset) -> BzCompression {
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

fn collect_members_from_archive<R: Read>(
    archive: &mut Archive<R>,
) -> Result<Vec<SourceMember>, CommandError> {
    let mut members = Vec::new();
    let entries = archive.entries().map_err(|error| {
        edit_error(
            "invalid_archive",
            format!("Cannot read tar entries: {error}"),
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|error| {
            edit_error("invalid_archive", format!("Cannot read tar entry: {error}"))
        })?;
        let path = entry.path().map_err(|error| {
            edit_error("invalid_entry", format!("Invalid tar entry path: {error}"))
        })?;
        let raw = path.to_string_lossy().replace('\\', "/");
        let normalized = normalize_member_name(&raw)?;

        let header = entry.header();
        let entry_type = header.entry_type();
        let is_dir = entry_type.is_dir() || raw.ends_with('/');
        let is_link_or_special = matches!(
            entry_type,
            EntryType::Symlink
                | EntryType::Link
                | EntryType::Char
                | EntryType::Block
                | EntryType::Fifo
                | EntryType::Continuous
        ) || entry_type.is_symlink()
            || entry_type.is_hard_link();

        // Match open/list: skip non-extractable specials from edit plan space.
        if is_link_or_special {
            continue;
        }

        members.push(SourceMember {
            index: members.len(),
            path: normalized,
            is_dir,
        });
    }
    Ok(members)
}

fn open_source_members(path: &Path, fmt: ArchiveFormat) -> Result<Vec<SourceMember>, CommandError> {
    let file = File::open(path)
        .map_err(|error| edit_error("invalid_archive", format!("Cannot open archive: {error}")))?;
    match fmt {
        ArchiveFormat::Tar => {
            let mut archive = Archive::new(file);
            collect_members_from_archive(&mut archive)
        }
        ArchiveFormat::TarGz => {
            let mut archive = Archive::new(GzDecoder::new(file));
            collect_members_from_archive(&mut archive)
        }
        ArchiveFormat::TarBz2 => {
            let mut archive = Archive::new(BzDecoder::new(file));
            collect_members_from_archive(&mut archive)
        }
        ArchiveFormat::TarXz => {
            let mut archive = Archive::new(XzDecoder::new(file));
            collect_members_from_archive(&mut archive)
        }
        other => Err(edit_error(
            "unsupported_operation",
            format!("Not a TAR-family format: {}", other.as_str()),
        )),
    }
}

fn drain_reader(reader: &mut dyn Read, cancelled: &AtomicBool) -> Result<(), CommandError> {
    let mut sink = [0_u8; BUFFER_SIZE];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        match reader.read(&mut sink) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                return Err(edit_error(
                    "invalid_archive",
                    format!("Cannot drain tar member: {error}"),
                ));
            }
        }
    }
    Ok(())
}

fn write_directory_member<W: Write>(
    builder: &mut Builder<W>,
    path: &str,
) -> Result<(), CommandError> {
    let member = member_path_for_tar(path);
    if member.is_empty() {
        return Err(edit_error("invalid_entry", "Archive entry path is empty."));
    }
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Directory);
    header.set_path(&member).map_err(|error| {
        edit_error(
            "write_failed",
            format!("Cannot set tar directory path: {error}"),
        )
    })?;
    header.set_size(0);
    header.set_mode(0o755);
    header.set_cksum();
    builder.append(&header, io::empty()).map_err(|error| {
        edit_error(
            "write_failed",
            format!("Cannot write tar directory: {error}"),
        )
    })?;
    Ok(())
}

fn write_new_file_member<W: Write>(
    builder: &mut Builder<W>,
    path: &str,
    source: &Path,
    cancelled: &AtomicBool,
) -> Result<(), CommandError> {
    let metadata = source_metadata(source)?;
    if !metadata.is_file() {
        return Err(edit_error(
            "invalid_source",
            format!("Source is not a regular file: {}", source.display()),
        ));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let member = member_path_for_tar(path);
    if member.is_empty() {
        return Err(edit_error("invalid_entry", "Archive entry path is empty."));
    }
    let size = metadata.len();
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_path(&member).map_err(|error| {
        edit_error("write_failed", format!("Cannot set tar file path: {error}"))
    })?;
    header.set_size(size);
    header.set_mode(0o644);
    header.set_cksum();

    let file = open_source_file(source).map_err(|error| {
        edit_error(
            "source_read",
            format!("Cannot open source {}: {error}", source.display()),
        )
    })?;
    let mut reader = CancellableRead::new(file, cancelled);
    if let Err(error) = builder.append_data(&mut header, &member, &mut reader) {
        if cancelled.load(Ordering::Relaxed)
            || error.to_string().contains("cancelled")
            || error.kind() == io::ErrorKind::Interrupted
        {
            return Err(cancelled_error());
        }
        return Err(edit_error(
            "write_failed",
            format!("Cannot write tar member {}: {error}", source.display()),
        ));
    }
    Ok(())
}

fn write_copied_entry<W: Write, R: Read>(
    builder: &mut Builder<W>,
    entry: &mut tar::Entry<'_, R>,
    out_path: &str,
    is_dir: bool,
    cancelled: &AtomicBool,
) -> Result<(), CommandError> {
    let member = member_path_for_tar(out_path);
    if member.is_empty() {
        return Err(edit_error("invalid_entry", "Archive entry path is empty."));
    }

    let src_header = entry.header().clone();
    let entry_type = src_header.entry_type();
    let treat_as_dir = is_dir || entry_type.is_dir();

    if treat_as_dir {
        // Still drain any payload (usually empty) so the reader advances.
        drain_reader(entry, cancelled)?;
        write_directory_member(builder, out_path)?;
        return Ok(());
    }

    let size = src_header.size().map_err(|error| {
        edit_error(
            "invalid_archive",
            format!("Cannot read tar member size: {error}"),
        )
    })?;
    let mode = src_header.mode().unwrap_or(0o644);
    let mtime = src_header.mtime().unwrap_or(0);

    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_path(&member).map_err(|error| {
        edit_error("write_failed", format!("Cannot set tar file path: {error}"))
    })?;
    header.set_size(size);
    header.set_mode(mode);
    header.set_mtime(mtime);
    header.set_cksum();

    let mut reader = CancellableRead::new(entry, cancelled);
    if let Err(error) = builder.append_data(&mut header, &member, &mut reader) {
        if cancelled.load(Ordering::Relaxed)
            || error.to_string().contains("cancelled")
            || error.kind() == io::ErrorKind::Interrupted
        {
            return Err(cancelled_error());
        }
        return Err(edit_error(
            "write_failed",
            format!("Cannot copy tar member {out_path}: {error}"),
        ));
    }
    Ok(())
}

/// Sequential decode of source; re-encode kept/renamed members; append new/replace from disk.
fn process_entries_into<W: Write, R: Read>(
    archive: &mut Archive<R>,
    builder: &mut Builder<W>,
    keep_map: &AHashMap<String, (String, bool)>,
    new_members: &[&RebuildMember],
    operation_id: &str,
    cancelled: &AtomicBool,
    total_files: u64,
    emit: &mut dyn FnMut(OperationProgress),
) -> Result<u64, CommandError> {
    let mut processed = 0_u64;
    let mut progress_gate = ProgressGate::new();
    let mut kept_written: HashSet<String> = HashSet::new();

    let entries = archive.entries().map_err(|error| {
        edit_error(
            "invalid_archive",
            format!("Cannot read tar entries: {error}"),
        )
    })?;

    for entry in entries {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        let mut entry = entry.map_err(|error| {
            edit_error("invalid_archive", format!("Cannot read tar entry: {error}"))
        })?;

        let path = entry.path().map_err(|error| {
            edit_error("invalid_entry", format!("Invalid tar entry path: {error}"))
        })?;
        let raw = path.to_string_lossy().replace('\\', "/");
        let entry_type = entry.header().entry_type();
        let is_link_or_special = matches!(
            entry_type,
            EntryType::Symlink
                | EntryType::Link
                | EntryType::Char
                | EntryType::Block
                | EntryType::Fifo
                | EntryType::Continuous
        ) || entry_type.is_symlink()
            || entry_type.is_hard_link();

        if is_link_or_special {
            // Not part of edit plan space; drop on rebuild (same as extract/repack).
            drain_reader(&mut entry, cancelled)?;
            continue;
        }

        let normalized = match normalize_member_name(&raw) {
            Ok(n) => n,
            Err(e) => {
                drain_reader(&mut entry, cancelled)?;
                return Err(e);
            }
        };

        match keep_map.get(&normalized) {
            Some((out_path, is_dir)) => {
                if progress_gate.should_emit() {
                    emit(OperationProgress {
                        operation_id: operation_id.into(),
                        extracted_files: processed,
                        total_files,
                        current_file: out_path.clone(),
                        percentage: progress_percentage(processed, total_files),
                        phase: Some("rebuild".into()),
                    });
                }
                // Honor cancel set inside progress callback before writing.
                if cancelled.load(Ordering::Relaxed) {
                    return Err(cancelled_error());
                }
                write_copied_entry(builder, &mut entry, out_path, *is_dir, cancelled)?;
                kept_written.insert(normalized);
                processed = processed.saturating_add(1);
            }
            None => {
                // Dropped or replaced: consume payload so the stream advances.
                drain_reader(&mut entry, cancelled)?;
            }
        }
    }

    if kept_written.len() != keep_map.len() {
        return Err(edit_error(
            "invalid_archive",
            format!(
                "Stream rebuild missed {} kept member(s).",
                keep_map.len().saturating_sub(kept_written.len())
            ),
        ));
    }

    for member in new_members {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        let current = member.out_path().to_string();
        if progress_gate.should_emit() {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: processed,
                total_files,
                current_file: current.clone(),
                percentage: progress_percentage(processed, total_files),
                phase: Some("rebuild".into()),
            });
        }
        match member {
            RebuildMember::NewDirectory { path } => {
                write_directory_member(builder, path)?;
            }
            RebuildMember::NewFile { path, source } => {
                write_new_file_member(builder, path, source, cancelled)?;
            }
            RebuildMember::Copy { .. } => unreachable!("filtered into keep_map"),
        }
        processed = processed.saturating_add(1);
    }

    Ok(processed)
}

fn finish_plain_tar(builder: Builder<File>) -> Result<File, CommandError> {
    let file = builder.into_inner().map_err(|error| {
        edit_error(
            "write_failed",
            format!("Cannot finalize tar archive: {error}"),
        )
    })?;
    file.sync_all()
        .map_err(|error| edit_error("write_failed", format!("Cannot sync tar archive: {error}")))?;
    Ok(file)
}

fn finish_gz_tar(builder: Builder<GzEncoder<File>>) -> Result<File, CommandError> {
    let encoder = builder
        .into_inner()
        .map_err(|error| edit_error("write_failed", format!("Cannot finalize tar.gz: {error}")))?;
    let file = encoder.finish().map_err(|error| {
        edit_error(
            "write_failed",
            format!("Cannot finish gzip stream: {error}"),
        )
    })?;
    file.sync_all()
        .map_err(|error| edit_error("write_failed", format!("Cannot sync tar.gz: {error}")))?;
    Ok(file)
}

fn finish_bz_tar(builder: Builder<BzEncoder<File>>) -> Result<File, CommandError> {
    let encoder = builder
        .into_inner()
        .map_err(|error| edit_error("write_failed", format!("Cannot finalize tar.bz2: {error}")))?;
    let file = encoder.finish().map_err(|error| {
        edit_error(
            "write_failed",
            format!("Cannot finish bzip2 stream: {error}"),
        )
    })?;
    file.sync_all()
        .map_err(|error| edit_error("write_failed", format!("Cannot sync tar.bz2: {error}")))?;
    Ok(file)
}

fn finish_xz_tar(builder: Builder<XzEncoder<File>>) -> Result<File, CommandError> {
    let encoder = builder
        .into_inner()
        .map_err(|error| edit_error("write_failed", format!("Cannot finalize tar.xz: {error}")))?;
    let file = encoder
        .finish()
        .map_err(|error| edit_error("write_failed", format!("Cannot finish xz stream: {error}")))?;
    file.sync_all()
        .map_err(|error| edit_error("write_failed", format!("Cannot sync tar.xz: {error}")))?;
    Ok(file)
}

/// Stream rebuild: one sequential decode pass; re-encode to temp; atomic publish.
fn stream_rebuild(
    archive_path: &Path,
    fmt: ArchiveFormat,
    members: &[SourceMember],
    planned: &[RebuildMember],
    operation_id: &str,
    cancelled: &AtomicBool,
    compression: CompressionPreset,
    mut emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(edit_error("invalid_operation", "Operation ID is empty."));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let mut keep_map: AHashMap<String, (String, bool)> = AHashMap::new();
    let mut new_members: Vec<&RebuildMember> = Vec::new();
    for member in planned {
        match member {
            RebuildMember::Copy {
                index,
                out_path,
                is_dir,
            } => {
                let src = members.get(*index).ok_or_else(|| {
                    edit_error("invalid_archive", "Planned copy index out of range.")
                })?;
                keep_map.insert(src.path.clone(), (out_path.clone(), *is_dir));
            }
            RebuildMember::NewDirectory { .. } | RebuildMember::NewFile { .. } => {
                new_members.push(member);
            }
        }
    }

    let total_files = planned.len() as u64;
    let (temp_path, temp_file) = create_temporary_archive(archive_path)?;

    let result = (|| -> Result<EditSummary, CommandError> {
        let source = File::open(archive_path).map_err(|error| {
            edit_error(
                "invalid_archive",
                format!("Cannot open archive for rebuild: {error}"),
            )
        })?;

        let processed = match fmt {
            ArchiveFormat::Tar => {
                let mut builder = Builder::new(temp_file);
                let mut archive = Archive::new(source);
                let processed = process_entries_into(
                    &mut archive,
                    &mut builder,
                    &keep_map,
                    &new_members,
                    operation_id,
                    cancelled,
                    total_files,
                    &mut emit,
                )?;
                drop(finish_plain_tar(builder)?);
                processed
            }
            ArchiveFormat::TarGz => {
                let encoder = GzEncoder::new(temp_file, gz_level(compression));
                let mut builder = Builder::new(encoder);
                let mut archive = Archive::new(GzDecoder::new(source));
                let processed = process_entries_into(
                    &mut archive,
                    &mut builder,
                    &keep_map,
                    &new_members,
                    operation_id,
                    cancelled,
                    total_files,
                    &mut emit,
                )?;
                drop(finish_gz_tar(builder)?);
                processed
            }
            ArchiveFormat::TarBz2 => {
                let encoder = BzEncoder::new(temp_file, bz_level(compression));
                let mut builder = Builder::new(encoder);
                let mut archive = Archive::new(BzDecoder::new(source));
                let processed = process_entries_into(
                    &mut archive,
                    &mut builder,
                    &keep_map,
                    &new_members,
                    operation_id,
                    cancelled,
                    total_files,
                    &mut emit,
                )?;
                drop(finish_bz_tar(builder)?);
                processed
            }
            ArchiveFormat::TarXz => {
                let encoder = XzEncoder::new(temp_file, xz_level(compression));
                let mut builder = Builder::new(encoder);
                let mut archive = Archive::new(XzDecoder::new(source));
                let processed = process_entries_into(
                    &mut archive,
                    &mut builder,
                    &keep_map,
                    &new_members,
                    operation_id,
                    cancelled,
                    total_files,
                    &mut emit,
                )?;
                drop(finish_xz_tar(builder)?);
                processed
            }
            other => {
                return Err(edit_error(
                    "unsupported_operation",
                    format!("Not a TAR-family format: {}", other.as_str()),
                ));
            }
        };

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        publish_temp_archive(&temp_path, archive_path, true).map_err(|error| {
            edit_error(
                "finalize_failed",
                format!("Cannot replace archive with edited copy: {error}"),
            )
        })?;

        Ok(EditSummary {
            operation_id: operation_id.into(),
            destination: archive_path.to_string_lossy().into_owned(),
            members_written: processed,
            strategy_used: Some("stream_rebuild".into()),
        })
    })();

    match result {
        Ok(summary) => {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: summary.members_written,
                total_files: summary.members_written,
                current_file: "Completed".into(),
                percentage: 100.0,
                phase: Some("rebuild".into()),
            });
            Ok(summary)
        }
        Err(mut error) => {
            cleanup_temp(&temp_path, &mut error);
            Err(error)
        }
    }
}

fn apply_edit(
    archive_path: &Path,
    planned: Vec<RebuildMember>,
    members: &[SourceMember],
    fmt: ArchiveFormat,
    operation_id: &str,
    cancelled: &AtomicBool,
    options: &EditOptions,
    emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    stream_rebuild(
        archive_path,
        fmt,
        members,
        &planned,
        operation_id,
        cancelled,
        edit_compression(fmt, options),
        emit,
    )
}

// ── Public ops ──────────────────────────────────────────────────────────────

/// Deletes archive entries matching `paths` (exact or directory prefix).
pub fn delete_entries(
    archive_path: &Path,
    paths: &[String],
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    let fmt = require_tar_family(archive_path)?;
    let members = open_source_members(archive_path, fmt)?;
    let planned = plan_delete(&members, paths)?;
    apply_edit(
        archive_path,
        planned,
        &members,
        fmt,
        operation_id,
        cancelled,
        options,
        emit,
    )
}

/// Renames a file entry or directory prefix inside a TAR-family archive.
pub fn rename_entry(
    archive_path: &Path,
    from: &str,
    to: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    let fmt = require_tar_family(archive_path)?;
    let members = open_source_members(archive_path, fmt)?;
    let planned = plan_rename(&members, from, to)?;
    apply_edit(
        archive_path,
        planned,
        &members,
        fmt,
        operation_id,
        cancelled,
        options,
        emit,
    )
}

/// Moves entries into `dest_folder` (leaf names preserved).
pub fn move_entries(
    archive_path: &Path,
    sources: &[String],
    dest_folder: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    let fmt = require_tar_family(archive_path)?;
    let members = open_source_members(archive_path, fmt)?;
    let planned = plan_move(&members, sources, dest_folder)?;
    apply_edit(
        archive_path,
        planned,
        &members,
        fmt,
        operation_id,
        cancelled,
        options,
        emit,
    )
}

/// Creates an empty directory entry in the archive.
pub fn create_folder(
    archive_path: &Path,
    folder_path: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    let fmt = require_tar_family(archive_path)?;
    let members = open_source_members(archive_path, fmt)?;
    let planned = plan_create_folder(&members, folder_path)?;
    apply_edit(
        archive_path,
        planned,
        &members,
        fmt,
        operation_id,
        cancelled,
        options,
        emit,
    )
}

/// Adds files and directories from disk into the archive under `archive_parent`.
pub fn add_paths(
    archive_path: &Path,
    source_paths: &[String],
    archive_parent: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    let fmt = require_tar_family(archive_path)?;
    let members = open_source_members(archive_path, fmt)?;
    let planned = plan_add_paths(
        &members,
        source_paths,
        archive_parent,
        archive_path,
        cancelled,
    )?;
    apply_edit(
        archive_path,
        planned,
        &members,
        fmt,
        operation_id,
        cancelled,
        options,
        emit,
    )
}

/// Replaces an existing file member with content from `source_file`.
pub fn replace_file(
    archive_path: &Path,
    entry_path: &str,
    source_file: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    let fmt = require_tar_family(archive_path)?;
    let members = open_source_members(archive_path, fmt)?;
    let planned = plan_replace_file(&members, entry_path, source_file)?;
    apply_edit(
        archive_path,
        planned,
        &members,
        fmt,
        operation_id,
        cancelled,
        options,
        emit,
    )
}

/// Full stream rewrite of every member (normalizes container; reclaim layout).
pub fn compact_archive(
    archive_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    let fmt = require_tar_family(archive_path)?;
    let members = open_source_members(archive_path, fmt)?;
    let planned: Vec<RebuildMember> = members
        .iter()
        .enumerate()
        .map(|(index, member)| RebuildMember::Copy {
            index,
            out_path: member.path.clone(),
            is_dir: member.is_dir,
        })
        .collect();
    let mut summary = apply_edit(
        archive_path,
        planned,
        &members,
        fmt,
        operation_id,
        cancelled,
        options,
        emit,
    )?;
    summary.strategy_used = Some("compact".into());
    Ok(summary)
}
