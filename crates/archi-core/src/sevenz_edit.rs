//! 7z edit: plan Keep/Rename/Drop/New → apply.
//! Non-solid eligible: pack-stream byte-copy (fallback stream-rebuild).
//! Solid archives: extract → mutate → recreate (repack).

use crate::create_common::{
    cleanup_temp, create_temporary_archive, member_path_for_tar, open_source_file,
    progress_percentage, publish_temp_archive, ProgressGate,
};
use crate::edit_plan::{
    plan_add_paths, plan_create_folder, plan_delete, plan_move, plan_rename, plan_replace_file,
    RebuildMember, SourceMember,
};
use crate::edit_sources::source_metadata;
use crate::extraction::{extract_any, FailOnConflict};
use crate::models::{
    CommandError, CompressionPreset, CreateFormat, CreateOptions, EditOptions, EditSummary,
    OperationProgress,
};
use crate::sevenz_format::create_sevenz_archive;
use crate::sevenz_format::normalize_member_name;
use crate::sevenz_pack_copy::{
    assess_pack_copy_eligibility, pack_stream_rebuild, PackStreamMember,
};
use ahash::AHashMap;
use sevenz_rust2::encoder_options::AesEncoderOptions;
use sevenz_rust2::{ArchiveEntry as SzEntry, ArchiveReader, ArchiveWriter, Password};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::io_perf::IO_BUFFER_SIZE as BUFFER_SIZE;

fn edit_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn cancelled_error() -> CommandError {
    edit_error("cancelled", "Archive edit was cancelled.")
}

fn map_sz_error(error: sevenz_rust2::Error) -> CommandError {
    match &error {
        _ => {
            let message = error.to_string();
            let lower = message.to_ascii_lowercase();
            if lower.contains("password") || lower.contains("encrypt") {
                edit_error(
                    "password_required",
                    "Invalid password provided. Please try again.",
                )
            } else {
                if lower.contains("cancelled") {
                    return cancelled_error();
                }
                edit_error("invalid_archive", format!("7z error: {message}"))
            }
        }
    }
}

fn sz_cb_err(msg: impl Into<String>) -> sevenz_rust2::Error {
    sevenz_rust2::Error::Other(msg.into().into())
}

fn edit_compression(options: &EditOptions) -> CompressionPreset {
    options.compression.unwrap_or(CompressionPreset::Normal)
}

fn require_sevenz_file(path: &Path) -> Result<(), CommandError> {
    if !path.is_file() {
        return Err(edit_error(
            "not_found",
            format!("Archive not found: {}", path.display()),
        ));
    }
    Ok(())
}

/// List physical members and whether the archive is solid.
fn open_source_members(
    path: &Path,
    password: Option<&str>,
) -> Result<(bool, Vec<SourceMember>), CommandError> {
    let pw = password.map_or_else(Password::empty, Password::new);
    let reader = ArchiveReader::open(path, pw).map_err(map_sz_error)?;
    let is_solid = reader.archive().is_solid;
    let mut members = Vec::with_capacity(reader.archive().files.len());
    for file in &reader.archive().files {
        if file.is_anti_item {
            continue;
        }
        let path = normalize_member_name(file.name())?;
        members.push(SourceMember {
            index: members.len(),
            path,
            is_dir: file.is_directory,
        });
    }
    Ok((is_solid, members))
}

fn drain_reader(reader: &mut dyn Read, cancelled: &AtomicBool) -> Result<(), sevenz_rust2::Error> {
    let mut sink = [0_u8; BUFFER_SIZE];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(sz_cb_err("cancelled"));
        }
        match reader.read(&mut sink) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn write_new_file_entry(
    writer: &mut ArchiveWriter<File>,
    archive_path: &str,
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
    let reader = open_source_file(source).map_err(|error| {
        edit_error(
            "source_read",
            format!("Cannot open source {}: {error}", source.display()),
        )
    })?;
    let member = member_path_for_tar(archive_path);
    writer
        .push_archive_entry(SzEntry::from_path(source, member), Some(reader))
        .map_err(map_sz_error)?;
    Ok(())
}

/// Stream rebuild: one sequential decode pass over the source; re-encode kept members;
/// write new/replace from disk. No full work-tree extract.
fn stream_rebuild(
    archive_path: &Path,
    members: &[SourceMember],
    planned: &[RebuildMember],
    operation_id: &str,
    cancelled: &AtomicBool,
    compression: CompressionPreset,
    password: Option<&str>,
    mut emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(edit_error("invalid_operation", "Operation ID is empty."));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    // source path → (out_path, is_dir) for kept/renamed members.
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
        let mut writer = ArchiveWriter::new(temp_file).map_err(map_sz_error)?;
        let lzma2_opt = crate::sevenz_format::lzma2_options(compression);
        let methods = match password {
            Some(pw) => vec![
                AesEncoderOptions::new(Password::new(pw)).into(),
                lzma2_opt.into(),
            ],
            None => vec![lzma2_opt.into()],
        };
        writer.set_content_methods(methods);

        let mut processed = 0_u64;
        let mut progress_gate = ProgressGate::new();
        let mut kept_written: HashSet<String> = HashSet::new();

        let mut reader = ArchiveReader::open(
            archive_path,
            password.map_or_else(Password::empty, Password::new),
        )
        .map_err(map_sz_error)?;

        let for_each_result = reader.for_each_entries(|entry, data| {
            if cancelled.load(Ordering::Relaxed) {
                return Err(sz_cb_err("cancelled"));
            }
            if entry.is_anti_item {
                return Ok(true);
            }
            let normalized = match normalize_member_name(entry.name()) {
                Ok(n) => n,
                Err(e) => return Err(sz_cb_err(e.message)),
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
                    let member_name = member_path_for_tar(out_path);
                    if *is_dir || entry.is_directory {
                        writer
                            .push_archive_entry(SzEntry::new_directory(&member_name), None::<File>)
                            .map_err(|e| sz_cb_err(e.to_string()))?;
                    } else {
                        writer
                            .push_archive_entry(SzEntry::new_file(&member_name), Some(data))
                            .map_err(|e| sz_cb_err(e.to_string()))?;
                    }
                    kept_written.insert(normalized);
                    processed = processed.saturating_add(1);
                    Ok(true)
                }
                None => {
                    // Dropped or replaced: drain so solid-style blocks stay consistent.
                    drain_reader(data, cancelled)?;
                    Ok(true)
                }
            }
        });

        if let Err(error) = for_each_result {
            let msg = error.to_string();
            if msg.contains("cancelled") {
                return Err(cancelled_error());
            }
            return Err(map_sz_error(error));
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
                    let member_name = member_path_for_tar(path);
                    writer
                        .push_archive_entry(SzEntry::new_directory(&member_name), None::<File>)
                        .map_err(map_sz_error)?;
                }
                RebuildMember::NewFile { path, source } => {
                    write_new_file_entry(&mut writer, path, source, cancelled)?;
                }
                RebuildMember::Copy { .. } => unreachable!("filtered into keep_map"),
            }
            processed = processed.saturating_add(1);
        }

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let finished = writer.finish().map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot finalize temporary 7z: {error}"),
            )
        })?;
        finished.sync_all().map_err(|error| {
            edit_error("write_failed", format!("Cannot sync temporary 7z: {error}"))
        })?;
        drop(finished);
        drop(reader);

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

// ── Solid fallback: extract → mutate → recreate (no Max; Normal default) ──

fn create_work_dir(archive_path: &Path) -> Result<PathBuf, CommandError> {
    let parent = archive_path
        .parent()
        .ok_or_else(|| edit_error("invalid_archive", "Archive has no parent directory."))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = parent.join(format!(".archi-edit-work-{}-{}", std::process::id(), stamp));
    fs::create_dir_all(&dir).map_err(|e| {
        edit_error(
            "temp_create_failed",
            format!("Cannot create edit work directory: {e}"),
        )
    })?;
    Ok(dir)
}

fn remove_tree(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn apply_deletes(work: &Path, paths: &[String]) -> Result<(), CommandError> {
    for raw in paths {
        let rel = raw.trim_matches('/').replace('\\', "/");
        if rel.is_empty() || rel.split('/').any(|p| p == "..") {
            return Err(edit_error(
                "invalid_entry",
                format!("Invalid edit path: {raw}"),
            ));
        }
        let target = work.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if target.is_dir() {
            fs::remove_dir_all(&target).map_err(|e| {
                edit_error(
                    "write_failed",
                    format!("Cannot remove directory {rel}: {e}"),
                )
            })?;
        } else if target.exists() {
            fs::remove_file(&target).map_err(|e| {
                edit_error("write_failed", format!("Cannot remove file {rel}: {e}"))
            })?;
        }
    }
    Ok(())
}

fn apply_rename(work: &Path, from: &str, to: &str) -> Result<(), CommandError> {
    let from_rel = from.trim_matches('/').replace('\\', "/");
    let to_rel = to.trim_matches('/').replace('\\', "/");
    if from_rel.is_empty()
        || to_rel.is_empty()
        || from_rel.split('/').any(|p| p == "..")
        || to_rel.split('/').any(|p| p == "..")
    {
        return Err(edit_error("invalid_entry", "Invalid rename paths."));
    }
    let src = work.join(from_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let dst = work.join(to_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| edit_error("write_failed", format!("Cannot create rename parent: {e}")))?;
    }
    fs::rename(&src, &dst)
        .map_err(|e| edit_error("write_failed", format!("Cannot rename entry: {e}")))?;
    Ok(())
}

fn apply_mkdir(work: &Path, folder: &str) -> Result<(), CommandError> {
    let rel = folder.trim_matches('/').replace('\\', "/");
    if rel.is_empty() || rel.split('/').any(|p| p == "..") {
        return Err(edit_error("invalid_entry", "Invalid folder path."));
    }
    let target = work.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    fs::create_dir_all(&target)
        .map_err(|e| edit_error("write_failed", format!("Cannot create folder: {e}")))?;
    Ok(())
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<(), CommandError> {
    fs::create_dir_all(dest)
        .map_err(|e| edit_error("write_failed", format!("Cannot create directory: {e}")))?;
    for entry in fs::read_dir(src)
        .map_err(|e| edit_error("write_failed", format!("Cannot read directory: {e}")))?
    {
        let entry =
            entry.map_err(|e| edit_error("write_failed", format!("Cannot read entry: {e}")))?;
        let ty = entry
            .file_type()
            .map_err(|e| edit_error("write_failed", format!("Cannot stat entry: {e}")))?;
        let to = dest.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else if ty.is_file() {
            fs::copy(entry.path(), &to)
                .map_err(|e| edit_error("write_failed", format!("Cannot copy file: {e}")))?;
        }
    }
    Ok(())
}

fn apply_add(work: &Path, archive_parent: &str, sources: &[String]) -> Result<(), CommandError> {
    let parent_rel = archive_parent.trim_matches('/').replace('\\', "/");
    if parent_rel.split('/').any(|p| p == "..") {
        return Err(edit_error("invalid_entry", "Invalid archive parent path."));
    }
    let dest_base = if parent_rel.is_empty() {
        work.to_path_buf()
    } else {
        work.join(parent_rel.replace('/', std::path::MAIN_SEPARATOR_STR))
    };
    fs::create_dir_all(&dest_base).map_err(|e| {
        edit_error(
            "write_failed",
            format!("Cannot create add destination: {e}"),
        )
    })?;
    for src in sources {
        let src_path = Path::new(src);
        let name = src_path
            .file_name()
            .ok_or_else(|| edit_error("invalid_entry", "Source has no file name."))?;
        let dest = dest_base.join(name);
        if src_path.is_dir() {
            copy_dir_recursive(src_path, &dest)?;
        } else {
            fs::copy(src_path, &dest)
                .map_err(|e| edit_error("write_failed", format!("Cannot add file: {e}")))?;
        }
    }
    Ok(())
}

fn apply_replace(work: &Path, entry_path: &str, source_file: &Path) -> Result<(), CommandError> {
    let rel = entry_path.trim_matches('/').replace('\\', "/");
    if rel.is_empty() || rel.split('/').any(|p| p == "..") {
        return Err(edit_error("invalid_entry", "Invalid replace path."));
    }
    let dest = work.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            edit_error(
                "write_failed",
                format!("Cannot create parent for replace: {e}"),
            )
        })?;
    }
    fs::copy(source_file, &dest)
        .map_err(|e| edit_error("write_failed", format!("Cannot replace file: {e}")))?;
    Ok(())
}

fn repack_edit(
    archive_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
    password: Option<&str>,
    options: &EditOptions,
    mutate: impl FnOnce(&Path) -> Result<(), CommandError>,
) -> Result<EditSummary, CommandError> {
    let work = create_work_dir(archive_path)?;
    let compression = edit_compression(options);
    let result = (|| {
        extract_any(
            archive_path,
            &work,
            operation_id,
            cancelled,
            None,
            password.map(str::to_owned),
            &FailOnConflict,
            |mut p: OperationProgress| {
                p.phase = Some("repack".into());
                emit(p);
            },
        )
        .map(|_| ())?;
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        mutate(&work)?;
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        let create_options = CreateOptions {
            format: CreateFormat::SevenZ,
            compression,
            include_root: false,
            overwrite: true,
            password: password.map(str::to_owned),
        };
        let sources = vec![work.to_string_lossy().into_owned()];
        let password = match &create_options.password {
            Some(pw) => Password::new(pw.as_str()),
            None => Password::empty(),
        };
        let summary = create_sevenz_archive(
            &sources,
            archive_path,
            operation_id,
            cancelled,
            password,
            &create_options,
            |mut p| {
                p.phase = Some("repack".into());
                emit(p);
            },
        )?;
        Ok(EditSummary {
            operation_id: operation_id.into(),
            destination: archive_path.to_string_lossy().into_owned(),
            members_written: summary.extracted_files,
            strategy_used: Some("repack".into()),
        })
    })();
    remove_tree(&work);
    result
}

/// Convert product rebuild plan into pack-copy plan (path-based).
fn to_pack_stream_members(
    members: &[SourceMember],
    planned: &[RebuildMember],
) -> Result<Vec<PackStreamMember>, CommandError> {
    let mut out = Vec::with_capacity(planned.len());
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
                out.push(PackStreamMember::Copy {
                    source_path: src.path.clone(),
                    out_path: out_path.clone(),
                    is_dir: *is_dir,
                });
            }
            RebuildMember::NewDirectory { path } => {
                out.push(PackStreamMember::NewDirectory { path: path.clone() });
            }
            RebuildMember::NewFile { path, source } => {
                out.push(PackStreamMember::NewFile {
                    path: path.clone(),
                    source: source.clone(),
                });
            }
        }
    }
    Ok(out)
}

/// Non-solid path: try pack-copy when archive is eligible; on non-cancel failure fall back
/// to full stream rebuild (mirrors ZIP append / logical-delete fallback).
fn apply_nonsolid_planned(
    archive_path: &Path,
    members: &[SourceMember],
    planned: &[RebuildMember],
    operation_id: &str,
    cancelled: &AtomicBool,
    compression: CompressionPreset,
    password: Option<&str>,
    mut emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    let pw = password.map_or_else(Password::empty, Password::new);
    let pack_eligible = match ArchiveReader::open(archive_path, pw) {
        Ok(reader) => assess_pack_copy_eligibility(reader.archive()).is_ok(),
        Err(_) => false,
    };

    if pack_eligible {
        if let Ok(pack_planned) = to_pack_stream_members(members, planned) {
            match pack_stream_rebuild(
                archive_path,
                &pack_planned,
                operation_id,
                cancelled,
                compression,
                &mut emit,
            ) {
                Ok(summary) => return Ok(summary),
                Err(error) if error.code == "cancelled" => return Err(error),
                Err(_) => {
                    // Non-cancel pack-copy failure → stream_rebuild.
                }
            }
        }
    }

    stream_rebuild(
        archive_path,
        members,
        planned,
        operation_id,
        cancelled,
        compression,
        password,
        emit,
    )
}

fn apply_planned(
    archive_path: &Path,
    is_solid: bool,
    members: &[SourceMember],
    planned: Vec<RebuildMember>,
    operation_id: &str,
    cancelled: &AtomicBool,
    password: Option<String>,
    options: &EditOptions,
    emit: impl FnMut(OperationProgress),
    solid_mutate: impl FnOnce(&Path) -> Result<(), CommandError>,
) -> Result<EditSummary, CommandError> {
    if is_solid {
        repack_edit(
            archive_path,
            operation_id,
            cancelled,
            emit,
            password.as_deref(),
            options,
            solid_mutate,
        )
    } else {
        apply_nonsolid_planned(
            archive_path,
            members,
            &planned,
            operation_id,
            cancelled,
            edit_compression(options),
            password.as_deref(),
            emit,
        )
    }
}

// ── Public ops ──────────────────────────────────────────────────────────────

/// Deletes archive entries matching `paths` (exact or directory prefix).
pub fn delete_entries(
    archive_path: &Path,
    paths: &[String],
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    password: Option<String>,
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path, password.as_deref())?;
    let planned = plan_delete(&members, paths)?;
    let paths = paths.to_vec();
    apply_planned(
        archive_path,
        is_solid,
        &members,
        planned,
        operation_id,
        cancelled,
        password,
        options,
        emit,
        move |work| apply_deletes(work, &paths),
    )
}

/// Renames a file entry or directory prefix inside a 7z archive.
pub fn rename_entry(
    archive_path: &Path,
    from: &str,
    to: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    password: Option<String>,
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path, password.as_deref())?;
    let planned = plan_rename(&members, from, to)?;
    let from = from.to_string();
    let to = to.to_string();
    apply_planned(
        archive_path,
        is_solid,
        &members,
        planned,
        operation_id,
        cancelled,
        password,
        options,
        emit,
        move |work| apply_rename(work, &from, &to),
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
    password: Option<String>,
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path, password.as_deref())?;
    let planned = plan_move(&members, sources, dest_folder)?;
    let sources = sources.to_vec();
    let dest_folder = dest_folder.to_string();
    apply_planned(
        archive_path,
        is_solid,
        &members,
        planned,
        operation_id,
        cancelled,
        password,
        options,
        emit,
        move |work| {
            let dest = if dest_folder.is_empty() || dest_folder == "/" {
                String::new()
            } else {
                dest_folder.trim_matches('/').replace('\\', "/")
            };
            let mut from_paths: Vec<String> = sources
                .iter()
                .map(|s| s.trim_matches('/').replace('\\', "/"))
                .filter(|s| !s.is_empty())
                .collect();
            from_paths.sort();
            from_paths.dedup();
            let tops: Vec<String> = from_paths
                .iter()
                .filter(|p| {
                    !from_paths
                        .iter()
                        .any(|o| o != *p && p.starts_with(&(o.clone() + "/")))
                })
                .cloned()
                .collect();
            for from in &tops {
                if !dest.is_empty() && (dest == *from || dest.starts_with(&(from.clone() + "/"))) {
                    return Err(edit_error(
                        "invalid_entry",
                        format!("Cannot move '{from}' into itself or a subfolder."),
                    ));
                }
                let leaf = from.rsplit('/').next().unwrap_or(from.as_str());
                let to = if dest.is_empty() {
                    leaf.to_string()
                } else {
                    format!("{dest}/{leaf}")
                };
                if from == &to {
                    continue;
                }
                apply_rename(work, from, &to)?;
            }
            Ok(())
        },
    )
}

/// Creates an empty directory entry in the 7z archive.
pub fn create_folder(
    archive_path: &Path,
    folder_path: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    password: Option<String>,
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path, password.as_deref())?;
    let planned = plan_create_folder(&members, folder_path)?;
    let folder_path = folder_path.to_string();
    apply_planned(
        archive_path,
        is_solid,
        &members,
        planned,
        operation_id,
        cancelled,
        password,
        options,
        emit,
        move |work| apply_mkdir(work, &folder_path),
    )
}

/// Adds files and directories from disk into the 7z under `archive_parent`.
pub fn add_paths(
    archive_path: &Path,
    source_paths: &[String],
    archive_parent: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    password: Option<String>,
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path, password.as_deref())?;
    let planned = plan_add_paths(
        &members,
        source_paths,
        archive_parent,
        archive_path,
        cancelled,
    )?;
    let source_paths = source_paths.to_vec();
    let archive_parent = archive_parent.to_string();
    apply_planned(
        archive_path,
        is_solid,
        &members,
        planned,
        operation_id,
        cancelled,
        password,
        options,
        emit,
        move |work| apply_add(work, &archive_parent, &source_paths),
    )
}

/// Full rewrite keeping every member (normalizes layout; drops unused packs when pack-copy path runs).
pub fn compact_archive(
    archive_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    password: Option<String>,
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path, password.as_deref())?;
    let planned: Vec<RebuildMember> = members
        .iter()
        .enumerate()
        .map(|(index, member)| RebuildMember::Copy {
            index,
            out_path: member.path.clone(),
            is_dir: member.is_dir,
        })
        .collect();
    let mut summary = apply_planned(
        archive_path,
        is_solid,
        &members,
        planned,
        operation_id,
        cancelled,
        password,
        options,
        emit,
        |_work| Ok(()),
    )?;
    summary.strategy_used = Some("compact".into());
    Ok(summary)
}

/// Replaces an existing file entry's content from a disk file (same archive path).
pub fn replace_file(
    archive_path: &Path,
    entry_path: &str,
    source_file: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    password: Option<String>,
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path, password.as_deref())?;
    let planned = plan_replace_file(&members, entry_path, source_file)?;
    let entry_path = entry_path.to_string();
    let source_file = source_file.to_path_buf();
    apply_planned(
        archive_path,
        is_solid,
        &members,
        planned,
        operation_id,
        cancelled,
        password,
        options,
        emit,
        move |work| apply_replace(work, &entry_path, &source_file),
    )
}
