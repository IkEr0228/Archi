//! Multi-format edit dispatcher. Planning and execution live in the format modules.

use crate::format_detect::{detect_format, ArchiveFormat};
use crate::models::{CommandError, EditOptions, EditSummary, OperationProgress};
use crate::sevenz_edit;
use crate::tar_edit;
use crate::zip_edit;
use std::path::Path;
use std::sync::atomic::AtomicBool;

fn edit_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

pub fn delete_entries(
    archive_path: &Path,
    paths: &[String],
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    password: Option<String>,
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    match detect_format(archive_path)? {
        ArchiveFormat::Zip => {
            zip_edit::delete_entries(archive_path, paths, operation_id, cancelled, emit, options)
        }
        ArchiveFormat::SevenZ => sevenz_edit::delete_entries(
            archive_path,
            paths,
            operation_id,
            cancelled,
            emit,
            password,
            options,
        ),
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz => {
            tar_edit::delete_entries(archive_path, paths, operation_id, cancelled, emit, options)
        }
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
    password: Option<String>,
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    match detect_format(archive_path)? {
        ArchiveFormat::Zip => zip_edit::rename_entry(
            archive_path,
            from_path,
            to_path,
            operation_id,
            cancelled,
            emit,
        ),
        ArchiveFormat::SevenZ => sevenz_edit::rename_entry(
            archive_path,
            from_path,
            to_path,
            operation_id,
            cancelled,
            emit,
            password,
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
    password: Option<String>,
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
            password,
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
    password: Option<String>,
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
            password,
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
    password: Option<String>,
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
            password,
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

/// Full clean rewrite of the archive (ZIP reclaims orphan local data after logical delete).
pub fn compact_archive(
    archive_path: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    password: Option<String>,
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    match detect_format(archive_path)? {
        ArchiveFormat::Zip => {
            zip_edit::compact_archive(archive_path, operation_id, cancelled, emit)
        }
        ArchiveFormat::SevenZ => sevenz_edit::compact_archive(
            archive_path,
            operation_id,
            cancelled,
            emit,
            password,
            options,
        ),
        ArchiveFormat::Tar
        | ArchiveFormat::TarGz
        | ArchiveFormat::TarBz2
        | ArchiveFormat::TarXz => {
            tar_edit::compact_archive(archive_path, operation_id, cancelled, emit, options)
        }
        other => Err(edit_error(
            "unsupported_operation",
            format!("Compact is not supported for {} archives.", other.as_str()),
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
    password: Option<String>,
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
            password,
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
