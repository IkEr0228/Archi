//! Multi-format edit: ZIP stream-rebuild; 7z stream-rebuild (non-solid) / repack (solid);
//! TAR family stream-rebuild (repack_edit kept as unused fallback helper).

use crate::extraction::{extract_any, FailOnConflict};
use crate::format_detect::{detect_format, ArchiveFormat};
use crate::models::{
    CommandError, CompressionPreset, CreateFormat, CreateOptions, EditOptions, EditSummary,
    OperationProgress,
};
use crate::sevenz_edit;
use crate::sevenz_format::create_sevenz_archive;
use crate::tar_create::create_tar_archive;
use crate::tar_edit;
use crate::zip_edit;
use crate::zipper::create_zip_archive;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn edit_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn format_to_create(fmt: ArchiveFormat) -> Result<CreateFormat, CommandError> {
    match fmt {
        ArchiveFormat::Zip => Ok(CreateFormat::Zip),
        ArchiveFormat::Tar => Ok(CreateFormat::Tar),
        ArchiveFormat::TarGz => Ok(CreateFormat::TarGz),
        ArchiveFormat::TarBz2 => Ok(CreateFormat::TarBz2),
        ArchiveFormat::TarXz => Ok(CreateFormat::TarXz),
        ArchiveFormat::SevenZ => Ok(CreateFormat::SevenZ),
        other => Err(edit_error(
            "unsupported_operation",
            format!(
                "Edit is not supported for single-stream {} archives.",
                other.as_str()
            ),
        )),
    }
}

fn default_compression(fmt: CreateFormat) -> CompressionPreset {
    match fmt {
        CreateFormat::Tar => CompressionPreset::Store,
        // Edit repack (TAR / legacy paths): prefer speed over ratio. Create UI still uses Max when chosen.
        CreateFormat::SevenZ => CompressionPreset::Normal,
        _ => CompressionPreset::Normal,
    }
}

// Repack helpers retained as optional fallback if stream rebuild fails hard for a format.
#[allow(dead_code)]
fn create_work_dir(archive_path: &Path) -> Result<PathBuf, CommandError> {
    let parent = archive_path
        .parent()
        .ok_or_else(|| edit_error("invalid_archive", "Archive has no parent directory."))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = parent.join(format!(
        ".archi-edit-work-{}-{}",
        std::process::id(),
        stamp
    ));
    fs::create_dir_all(&dir).map_err(|e| {
        edit_error(
            "temp_create_failed",
            format!("Cannot create edit work directory: {e}"),
        )
    })?;
    Ok(dir)
}

#[allow(dead_code)]
fn remove_tree(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

#[allow(dead_code)]
fn recreate_from_work(
    work: &Path,
    archive_path: &Path,
    fmt: CreateFormat,
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<u64, CommandError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(edit_error("cancelled", "Archive edit was cancelled."));
    }
    let options = CreateOptions {
        format: fmt,
        compression: default_compression(fmt),
        include_root: false,
        overwrite: true,
    };
    let sources = vec![work.to_string_lossy().into_owned()];
    let summary = match fmt {
        CreateFormat::Zip => create_zip_archive(
            &sources,
            archive_path,
            operation_id,
            cancelled,
            &options,
            &mut emit,
        )?,
        CreateFormat::Tar
        | CreateFormat::TarGz
        | CreateFormat::TarBz2
        | CreateFormat::TarXz => create_tar_archive(
            &sources,
            archive_path,
            operation_id,
            cancelled,
            &options,
            &mut emit,
        )?,
        CreateFormat::SevenZ => create_sevenz_archive(
            &sources,
            archive_path,
            operation_id,
            cancelled,
            &options,
            &mut emit,
        )?,
    };
    Ok(summary.extracted_files)
}

#[allow(dead_code)]
fn extract_all_to_work(
    archive_path: &Path,
    work: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<(), CommandError> {
    extract_any(
        archive_path,
        work,
        operation_id,
        cancelled,
        None,
        &FailOnConflict,
        |p| emit(p),
    )
    .map(|_| ())
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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
        fs::create_dir_all(parent).map_err(|e| {
            edit_error("write_failed", format!("Cannot create rename parent: {e}"))
        })?;
    }
    fs::rename(&src, &dst)
        .map_err(|e| edit_error("write_failed", format!("Cannot rename entry: {e}")))?;
    Ok(())
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
fn repack_edit(
    archive_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
    mutate: impl FnOnce(&Path) -> Result<(), CommandError>,
) -> Result<EditSummary, CommandError> {
    let fmt = detect_format(archive_path)?;
    let create_fmt = format_to_create(fmt)?;
    let work = create_work_dir(archive_path)?;
    let result = (|| {
        extract_all_to_work(archive_path, &work, operation_id, cancelled, &mut emit)?;
        if cancelled.load(Ordering::Relaxed) {
            return Err(edit_error("cancelled", "Archive edit was cancelled."));
        }
        mutate(&work)?;
        let members = recreate_from_work(
            &work,
            archive_path,
            create_fmt,
            operation_id,
            cancelled,
            &mut emit,
        )?;
        Ok(EditSummary {
            operation_id: operation_id.into(),
            destination: archive_path.to_string_lossy().into_owned(),
            members_written: members,
            strategy_used: Some("repack".into()),
        })
    })();
    remove_tree(&work);
    result
}

pub fn delete_entries(
    archive_path: &Path,
    paths: &[String],
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    match detect_format(archive_path)? {
        ArchiveFormat::Zip => {
            zip_edit::delete_entries(archive_path, paths, operation_id, cancelled, emit)
        }
        ArchiveFormat::SevenZ => sevenz_edit::delete_entries(
            archive_path,
            paths,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz => tar_edit::delete_entries(
            archive_path,
            paths,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        other => Err(edit_error(
            "unsupported_operation",
            format!("Edit is not supported for {} archives.", other.as_str()),
        )),
    }
}

pub fn rename_entry(
    archive_path: &Path,
    from_path: &str,
    to_path: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    match detect_format(archive_path)? {
        ArchiveFormat::Zip => {
            zip_edit::rename_entry(archive_path, from_path, to_path, operation_id, cancelled, emit)
        }
        ArchiveFormat::SevenZ => sevenz_edit::rename_entry(
            archive_path,
            from_path,
            to_path,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz => tar_edit::rename_entry(
            archive_path,
            from_path,
            to_path,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        other => Err(edit_error(
            "unsupported_operation",
            format!("Edit is not supported for {} archives.", other.as_str()),
        )),
    }
}

pub fn create_folder(
    archive_path: &Path,
    folder_path: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    match detect_format(archive_path)? {
        ArchiveFormat::Zip => zip_edit::create_folder(
            archive_path,
            folder_path,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        ArchiveFormat::SevenZ => sevenz_edit::create_folder(
            archive_path,
            folder_path,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz => tar_edit::create_folder(
            archive_path,
            folder_path,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        other => Err(edit_error(
            "unsupported_operation",
            format!("Edit is not supported for {} archives.", other.as_str()),
        )),
    }
}

pub fn add_paths(
    archive_path: &Path,
    source_paths: &[String],
    archive_parent: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    match detect_format(archive_path)? {
        ArchiveFormat::Zip => zip_edit::add_paths(
            archive_path,
            source_paths,
            archive_parent,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        ArchiveFormat::SevenZ => sevenz_edit::add_paths(
            archive_path,
            source_paths,
            archive_parent,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz => tar_edit::add_paths(
            archive_path,
            source_paths,
            archive_parent,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        other => Err(edit_error(
            "unsupported_operation",
            format!("Edit is not supported for {} archives.", other.as_str()),
        )),
    }
}

pub fn replace_file(
    archive_path: &Path,
    entry_path: &str,
    source_file: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    match detect_format(archive_path)? {
        ArchiveFormat::Zip => zip_edit::replace_file(
            archive_path,
            entry_path,
            source_file,
            operation_id,
            cancelled,
            emit,
        ),
        ArchiveFormat::SevenZ => sevenz_edit::replace_file(
            archive_path,
            entry_path,
            source_file,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz => tar_edit::replace_file(
            archive_path,
            entry_path,
            source_file,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        other => Err(edit_error(
            "unsupported_operation",
            format!("Edit is not supported for {} archives.", other.as_str()),
        )),
    }
}

/// Move archive entries into `dest_folder` (empty = root). Leaf names preserved.
pub fn move_entries(
    archive_path: &Path,
    sources: &[String],
    dest_folder: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    match detect_format(archive_path)? {
        ArchiveFormat::Zip => zip_edit::move_entries(
            archive_path,
            sources,
            dest_folder,
            operation_id,
            cancelled,
            emit,
        ),
        ArchiveFormat::SevenZ => sevenz_edit::move_entries(
            archive_path,
            sources,
            dest_folder,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz => tar_edit::move_entries(
            archive_path,
            sources,
            dest_folder,
            operation_id,
            cancelled,
            emit,
            options,
        ),
        other => Err(edit_error(
            "unsupported_operation",
            format!("Edit is not supported for {} archives.", other.as_str()),
        )),
    }
}
