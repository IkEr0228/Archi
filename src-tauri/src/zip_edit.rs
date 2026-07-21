use crate::extraction::normalize_entry_name;
use crate::models::{
    CommandError, EditOptions, EditStrategyPref, OperationProgress,
};

pub use crate::models::EditSummary;
use crate::security::{is_link_or_reparse_point, validate_entry_path};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use zip::write::FileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::io_perf::{ProgressGate, IO_BUFFER_SIZE as BUFFER_SIZE};

/// Prefer append for Auto / PreferFast / unspecified; PreferCompact always rebuilds.
fn should_try_append(options: &EditOptions) -> bool {
    !matches!(options.strategy, Some(EditStrategyPref::PreferCompact))
}

/// Whether to attempt ZIP logical delete (CD rewrite) for this strategy and delete set size.
///
/// - PreferCompact: never
/// - PreferFast: always try
/// - Auto / None: logical when `deleted/total ≤ 0.25` or `deleted ≤ 64`; else rebuild
fn should_try_logical_delete(options: &EditOptions, deleted: usize, total: usize) -> bool {
    match options.strategy {
        Some(EditStrategyPref::PreferCompact) => false,
        Some(EditStrategyPref::PreferFast) => true,
        Some(EditStrategyPref::Auto) | None => {
            if deleted == 0 || total == 0 {
                return false;
            }
            let fraction = deleted as f64 / total as f64;
            fraction <= 0.25 || deleted <= 64
        }
    }
}

/// Normalize delete selection the same way as `plan_delete` (for CD filtering).
fn normalize_delete_selection(paths: &[String]) -> Result<Vec<String>, CommandError> {
    if paths.is_empty() {
        return Err(edit_error(
            "invalid_selection",
            "No archive paths specified for delete.",
        ));
    }
    let mut selected = Vec::with_capacity(paths.len());
    for raw in paths {
        selected.push(normalize_and_validate(raw)?);
    }
    Ok(selected)
}

/// New directory / file members only (no Copy) — what append actually writes.
fn new_members_only(planned: &[RebuildMember]) -> Vec<RebuildMember> {
    planned
        .iter()
        .filter(|m| {
            matches!(
                m,
                RebuildMember::NewDirectory { .. } | RebuildMember::NewFile { .. }
            )
        })
        .cloned()
        .collect()
}

#[derive(Clone)]
enum RebuildMember {
    /// Stream-copy an existing archive entry under `out_path`.
    Copy {
        index: usize,
        /// Normalized archive path without trailing slash.
        out_path: String,
        is_dir: bool,
    },
    /// Write a new empty directory entry.
    NewDirectory { path: String },
    /// Write a new or replaced file from disk (Deflated).
    NewFile { path: String, source: PathBuf },
}

impl RebuildMember {
    fn out_path(&self) -> &str {
        match self {
            Self::Copy { out_path, .. } => out_path,
            Self::NewDirectory { path } => path,
            Self::NewFile { path, .. } => path,
        }
    }
}

struct DiskSourceEntry {
    path: PathBuf,
    /// Normalized archive path without trailing slash.
    archive_path: String,
    is_directory: bool,
}

fn edit_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn cancelled_error() -> CommandError {
    edit_error("cancelled", "Archive edit was cancelled.")
}

fn progress_percentage(processed: u64, total: u64) -> f32 {
    if total == 0 {
        100.0
    } else {
        ((processed as f64 * 100.0 / total as f64).min(100.0)) as f32
    }
}

fn normalize_and_validate(path: &str) -> Result<String, CommandError> {
    validate_entry_path(path).map_err(|message| CommandError {
        code: "invalid_entry".into(),
        message,
        path: Some(path.into()),
    })?;
    let normalized = normalize_entry_name(path);
    if normalized.is_empty() {
        return Err(CommandError {
            code: "invalid_entry".into(),
            message: "Archive entry path is empty or malformed.".into(),
            path: Some(path.into()),
        });
    }
    Ok(normalized)
}

fn create_temporary_edit_archive(zip_path: &Path) -> Result<(PathBuf, File), CommandError> {
    let parent = zip_path
        .parent()
        .ok_or_else(|| edit_error("invalid_archive", "Archive path has no parent directory."))?;
    let zip_name = zip_path
        .file_name()
        .ok_or_else(|| edit_error("invalid_archive", "Archive path has no file name."))?
        .to_string_lossy();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| edit_error("temp_create_failed", format!("Cannot get time: {error}")))?
        .as_nanos();

    for attempt in 0_u128.. {
        let temp_path = parent.join(format!(
            "{zip_name}.archi-edit-{}-{}",
            std::process::id(),
            timestamp.saturating_add(attempt)
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
        {
            Ok(file) => return Ok((temp_path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(edit_error(
                    "temp_create_failed",
                    format!("Cannot create temporary archive: {error}"),
                ));
            }
        }
    }

    Err(edit_error(
        "temp_create_failed",
        "Cannot choose a unique temporary archive path.",
    ))
}

/// Byte-for-byte copy of `zip_path` to a sibling temp file, opened for read+write+seek (append).
fn create_temporary_archive_copy(zip_path: &Path) -> Result<(PathBuf, File), CommandError> {
    let (temp_path, temp_file) = create_temporary_edit_archive(zip_path)?;
    drop(temp_file);
    if let Err(error) = fs::copy(zip_path, &temp_path) {
        let _ = fs::remove_file(&temp_path);
        return Err(edit_error(
            "temp_create_failed",
            format!("Cannot copy archive for append: {error}"),
        ));
    }
    match OpenOptions::new().read(true).write(true).open(&temp_path) {
        Ok(file) => Ok((temp_path, file)),
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            Err(edit_error(
                "temp_create_failed",
                format!("Cannot open temporary archive for append: {error}"),
            ))
        }
    }
}

fn cleanup_temp(temp_path: &Path, error: &mut CommandError) {
    if let Err(cleanup_error) = fs::remove_file(temp_path) {
        if cleanup_error.kind() != io::ErrorKind::NotFound {
            error.message.push_str(&format!(
                " Cleanup failed for temporary archive: {cleanup_error}."
            ));
        }
    }
}

#[cfg(windows)]
fn publish_temp_archive(temp_path: &Path, output_path: &Path) -> io::Result<()> {
    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;

    #[link(name = "Kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    let existing: Vec<_> = temp_path.as_os_str().encode_wide().chain(Some(0)).collect();
    let new: Vec<_> = output_path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let moved = unsafe { MoveFileExW(existing.as_ptr(), new.as_ptr(), flags) };
    if moved == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn publish_temp_archive(temp_path: &Path, output_path: &Path) -> io::Result<()> {
    if output_path.exists() {
        fs::remove_file(output_path)?;
    }
    fs::rename(temp_path, output_path)
}

struct SourceMember {
    index: usize,
    /// Normalized path without trailing slash.
    path: String,
    is_dir: bool,
}

fn open_source_members(
    zip_path: &Path,
) -> Result<(ZipArchive<File>, Vec<SourceMember>), CommandError> {
    let file = File::open(zip_path).map_err(|error| {
        edit_error(
            "invalid_archive",
            format!("Failed to open archive: {error}"),
        )
    })?;
    let mut archive = ZipArchive::new(file).map_err(|error| {
        edit_error(
            "invalid_archive",
            format!("Cannot open or read ZIP structure: {error}"),
        )
    })?;

    let mut members = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive.by_index(index).map_err(|error| {
            edit_error(
                "invalid_archive",
                format!("Failed to read zip entry: {error}"),
            )
        })?;
        let raw_name = entry.name().to_string();
        validate_entry_path(&raw_name).map_err(|message| CommandError {
            code: "invalid_entry".into(),
            message,
            path: Some(raw_name.clone()),
        })?;
        let path = normalize_entry_name(&raw_name);
        if path.is_empty() {
            return Err(CommandError {
                code: "invalid_entry".into(),
                message: "Archive entry path is empty or malformed.".into(),
                path: Some(raw_name),
            });
        }
        let is_dir = entry.is_dir() || raw_name.ends_with('/') || raw_name.ends_with('\\');
        members.push(SourceMember {
            index,
            path,
            is_dir,
        });
    }

    Ok((archive, members))
}

fn selection_matches(entry_path: &str, selected: &str) -> bool {
    entry_path == selected || entry_path.starts_with(&(selected.to_owned() + "/"))
}

fn plan_delete(
    members: &[SourceMember],
    paths: &[String],
) -> Result<Vec<RebuildMember>, CommandError> {
    if paths.is_empty() {
        return Err(edit_error(
            "invalid_selection",
            "No archive paths specified for delete.",
        ));
    }

    let mut selected = Vec::with_capacity(paths.len());
    for raw in paths {
        selected.push(normalize_and_validate(raw)?);
    }

    let mut planned = Vec::new();
    let mut matched = false;
    for member in members {
        let delete = selected
            .iter()
            .any(|sel| selection_matches(&member.path, sel));
        if delete {
            matched = true;
        } else {
            planned.push(RebuildMember::Copy {
                index: member.index,
                out_path: member.path.clone(),
                is_dir: member.is_dir,
            });
        }
    }
    if !matched {
        return Err(edit_error(
            "not_found",
            "Delete selection matched no archive entries.",
        ));
    }
    Ok(planned)
}

fn plan_rename(
    members: &[SourceMember],
    from: &str,
    to: &str,
) -> Result<Vec<RebuildMember>, CommandError> {
    let from = normalize_and_validate(from)?;
    let to = normalize_and_validate(to)?;
    if from == to {
        return Err(edit_error(
            "invalid_entry",
            "Rename source and destination are the same.",
        ));
    }

    let has_exact = members.iter().any(|m| m.path == from);
    let has_children = members
        .iter()
        .any(|m| m.path.starts_with(&(from.clone() + "/")));
    if !has_exact && !has_children {
        return Err(CommandError {
            code: "not_found".into(),
            message: format!("Rename source is not in the archive: {from}"),
            path: Some(from),
        });
    }

    let is_directory_rename = has_children || members.iter().any(|m| m.path == from && m.is_dir);

    let mut planned = Vec::with_capacity(members.len());
    let mut rewritten_targets: Vec<String> = Vec::new();

    for member in members {
        let out_path = if member.path == from {
            to.clone()
        } else if is_directory_rename && member.path.starts_with(&(from.clone() + "/")) {
            format!("{to}{}", &member.path[from.len()..])
        } else {
            member.path.clone()
        };

        if out_path != member.path {
            rewritten_targets.push(out_path.clone());
        }

        planned.push(RebuildMember::Copy {
            index: member.index,
            out_path,
            is_dir: member.is_dir,
        });
    }

    // Kept = non-rewritten members. Reject hierarchy collisions against them.
    let kept: Vec<(&str, bool)> = planned
        .iter()
        .filter_map(|p| match p {
            RebuildMember::Copy {
                index,
                out_path,
                is_dir,
            } => {
                let original = members
                    .iter()
                    .find(|m| m.index == *index)
                    .map(|m| m.path.as_str())
                    .unwrap_or("");
                if out_path.as_str() == original {
                    Some((out_path.as_str(), *is_dir))
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    for target in &rewritten_targets {
        if kept.iter().any(|(kept_path, kept_is_dir)| {
            path_hierarchy_collides(kept_path, *kept_is_dir, target)
        }) {
            return Err(CommandError {
                code: "entry_exists".into(),
                message: format!("Rename destination already exists: {target}"),
                path: Some(target.clone()),
            });
        }
    }

    // Also reject duplicate out_paths among planned members.
    let mut seen = std::collections::HashSet::new();
    for member in &planned {
        if !seen.insert(member.out_path().to_string()) {
            return Err(CommandError {
                code: "entry_exists".into(),
                message: format!("Rename destination already exists: {}", member.out_path()),
                path: Some(member.out_path().to_string()),
            });
        }
    }

    Ok(planned)
}

/// Plan moving archive entries into `dest_folder` (empty / "/" = archive root).
/// Each source keeps its leaf name: `docs/a.txt` → `folder/a.txt`.
fn plan_move(
    members: &[SourceMember],
    sources: &[String],
    dest_folder: &str,
) -> Result<Vec<RebuildMember>, CommandError> {
    if sources.is_empty() {
        return Err(edit_error(
            "invalid_selection",
            "No archive paths specified for move.",
        ));
    }

    let dest = if dest_folder.is_empty() || dest_folder == "/" {
        String::new()
    } else {
        normalize_and_validate(dest_folder)?
    };

    // Drop nested sources when a parent is also selected.
    let mut from_paths: Vec<String> = Vec::new();
    for raw in sources {
        let p = normalize_and_validate(raw)?;
        from_paths.push(p);
    }
    from_paths.sort();
    from_paths.dedup();
    let tops: Vec<String> = from_paths
        .iter()
        .filter(|p| {
            !from_paths
                .iter()
                .any(|o| *o != **p && p.starts_with(&(o.clone() + "/")))
        })
        .cloned()
        .collect();

    let mut moves: Vec<(String, String)> = Vec::new();
    for from in &tops {
        let leaf = from.rsplit('/').next().unwrap_or(from.as_str());
        let to = if dest.is_empty() {
            leaf.to_string()
        } else {
            format!("{dest}/{leaf}")
        };
        if from == &to {
            continue;
        }
        // Cannot move a folder into itself or a descendant.
        if !dest.is_empty() && (dest == *from || dest.starts_with(&(from.clone() + "/"))) {
            return Err(edit_error(
                "invalid_entry",
                format!("Cannot move '{from}' into itself or a subfolder."),
            ));
        }
        // Source must exist.
        let exists = members.iter().any(|m| {
            m.path == *from || m.path.starts_with(&(from.clone() + "/"))
        });
        if !exists {
            return Err(CommandError {
                code: "not_found".into(),
                message: format!("Move source is not in the archive: {from}"),
                path: Some(from.clone()),
            });
        }
        moves.push((from.clone(), to));
    }

    if moves.is_empty() {
        return Err(edit_error(
            "invalid_entry",
            "Nothing to move (sources already at destination).",
        ));
    }

    let mut planned = Vec::with_capacity(members.len());
    let mut rewritten_targets: Vec<String> = Vec::new();

    for member in members {
        let mut out_path = member.path.clone();
        for (from, to) in &moves {
            if out_path == *from {
                out_path = to.clone();
            } else if out_path.starts_with(&(from.clone() + "/")) {
                out_path = format!("{to}{}", &out_path[from.len()..]);
            }
        }
        if out_path != member.path {
            rewritten_targets.push(out_path.clone());
        }
        planned.push(RebuildMember::Copy {
            index: member.index,
            out_path,
            is_dir: member.is_dir,
        });
    }

    let kept: Vec<(&str, bool)> = planned
        .iter()
        .filter_map(|p| match p {
            RebuildMember::Copy {
                index,
                out_path,
                is_dir,
            } => {
                let original = members
                    .iter()
                    .find(|m| m.index == *index)
                    .map(|m| m.path.as_str())
                    .unwrap_or("");
                if out_path.as_str() == original {
                    Some((out_path.as_str(), *is_dir))
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    for target in &rewritten_targets {
        if kept.iter().any(|(kept_path, kept_is_dir)| {
            path_hierarchy_collides(kept_path, *kept_is_dir, target)
        }) {
            return Err(CommandError {
                code: "entry_exists".into(),
                message: format!("Move destination already exists: {target}"),
                path: Some(target.clone()),
            });
        }
    }

    let mut seen = std::collections::HashSet::new();
    for member in &planned {
        if !seen.insert(member.out_path().to_string()) {
            return Err(CommandError {
                code: "entry_exists".into(),
                message: format!("Move destination already exists: {}", member.out_path()),
                path: Some(member.out_path().to_string()),
            });
        }
    }

    Ok(planned)
}

/// True when introducing `path` would collide with a kept archive entry.
///
/// Collides when kept equals path, kept is under path/, or kept is a **file**
/// that is a strict path prefix of path (file-as-parent). Directory prefixes are
/// allowed so files can be added under existing folders.
fn path_hierarchy_collides(kept_path: &str, kept_is_dir: bool, path: &str) -> bool {
    if kept_path == path {
        return true;
    }
    let kept_child_prefix = format!("{path}/");
    if kept_path.starts_with(&kept_child_prefix) {
        return true;
    }
    let path_child_prefix = format!("{kept_path}/");
    if path.starts_with(&path_child_prefix) && !kept_is_dir {
        return true;
    }
    false
}

fn entry_hierarchy_collides(members: &[SourceMember], path: &str) -> bool {
    members
        .iter()
        .any(|m| path_hierarchy_collides(&m.path, m.is_dir, path))
}

/// Reject adding sources that are the open archive or contain it (sibling temp risk).
fn reject_add_sources_contain_archive(
    source_paths: &[String],
    zip_path: &Path,
) -> Result<(), CommandError> {
    let canonical_zip = zip_path.canonicalize().map_err(|error| {
        edit_error(
            "invalid_archive",
            format!("Cannot resolve archive path: {error}"),
        )
    })?;

    for raw in source_paths {
        let source = Path::new(raw);
        let metadata = source_metadata(source)?;
        let canonical_source = source.canonicalize().map_err(|error| {
            edit_error(
                if error.kind() == io::ErrorKind::NotFound {
                    "source_not_found"
                } else {
                    "invalid_source"
                },
                format!("Cannot resolve source {raw}: {error}"),
            )
        })?;
        if canonical_zip == canonical_source
            || (metadata.is_dir() && canonical_zip.starts_with(&canonical_source))
        {
            return Err(edit_error(
                "output_inside_source",
                "Open archive must be outside every add source path.",
            ));
        }
    }

    Ok(())
}

fn normalize_archive_parent(parent: &str) -> Result<String, CommandError> {
    let trimmed = parent.replace('\\', "/").trim_matches('/').to_string();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    normalize_and_validate(&trimmed)
}

fn join_archive_path(parent: &str, relative: &str) -> Result<String, CommandError> {
    let relative = relative.replace('\\', "/").trim_matches('/').to_string();
    if relative.is_empty() {
        return Err(edit_error(
            "invalid_entry",
            "Archive entry path is empty or malformed.",
        ));
    }
    let full = if parent.is_empty() {
        relative
    } else {
        format!("{parent}/{relative}")
    };
    normalize_and_validate(&full)
}

fn source_metadata(path: &Path) -> Result<fs::Metadata, CommandError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        edit_error(
            if error.kind() == io::ErrorKind::NotFound {
                "source_not_found"
            } else {
                "source_read"
            },
            format!("Cannot inspect source {}: {error}", path.display()),
        )
    })?;
    if is_link_or_reparse_point(&metadata) {
        return Err(edit_error(
            "invalid_source",
            format!("Source links are not supported: {}", path.display()),
        ));
    }
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(edit_error(
            "invalid_source",
            format!("Source is not a file or directory: {}", path.display()),
        ));
    }
    Ok(metadata)
}

#[cfg(windows)]
fn open_source_file(path: &Path) -> io::Result<crate::windows_fs::PinnedFile> {
    crate::windows_fs::open_source_file(path)
}

#[cfg(not(windows))]
fn open_source_file(path: &Path) -> io::Result<File> {
    File::open(path)
}

fn deflated_file_options() -> FileOptions {
    FileOptions::default().compression_method(CompressionMethod::Deflated)
}

fn enumerate_add_directory(
    source: &Path,
    archive_prefix: &str,
    entries: &mut Vec<DiskSourceEntry>,
    cancelled: &AtomicBool,
) -> Result<(), CommandError> {
    if !source_metadata(source)?.is_dir() {
        return Err(edit_error(
            "invalid_source",
            format!("Source directory changed: {}", source.display()),
        ));
    }
    #[cfg(windows)]
    let _pinned_directory = crate::windows_fs::Directory::open_root(source).map_err(|error| {
        edit_error(
            "invalid_source",
            format!("Cannot securely open source directory: {error}"),
        )
    })?;
    let directory = fs::read_dir(source).map_err(|error| {
        edit_error(
            "source_read",
            format!("Cannot read source directory {}: {error}", source.display()),
        )
    })?;
    let mut directory_entries = Vec::new();
    for entry in directory {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        directory_entries.push(entry.map_err(|error| {
            edit_error("source_read", format!("Cannot read source entry: {error}"))
        })?);
    }
    directory_entries.sort_by_key(|entry| {
        let name = entry.file_name();
        (name.to_string_lossy().to_lowercase(), name)
    });

    let mut child_count = 0_usize;
    for entry in directory_entries {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        let entry_path = entry.path();
        let entry_name = entry.file_name().to_string_lossy().into_owned();
        let entry_metadata = source_metadata(&entry_path)?;
        let archive_path = join_archive_path(archive_prefix, &entry_name)?;
        child_count += 1;

        if entry_metadata.is_dir() {
            let before = entries.len();
            enumerate_add_directory(&entry_path, &archive_path, entries, cancelled)?;
            if entries.len() == before {
                entries.push(DiskSourceEntry {
                    path: entry_path,
                    archive_path,
                    is_directory: true,
                });
            }
        } else {
            entries.push(DiskSourceEntry {
                path: entry_path,
                archive_path,
                is_directory: false,
            });
        }
    }

    if child_count == 0 {
        // Empty directory: keep an explicit directory marker when prefix is valid.
        if !archive_prefix.is_empty() {
            entries.push(DiskSourceEntry {
                path: source.to_path_buf(),
                archive_path: archive_prefix.to_string(),
                is_directory: true,
            });
        }
    }

    Ok(())
}

fn enumerate_add_sources(
    source_paths: &[String],
    archive_parent: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<DiskSourceEntry>, CommandError> {
    if source_paths.is_empty() {
        return Err(edit_error("invalid_source", "No source files specified."));
    }

    let parent = normalize_archive_parent(archive_parent)?;
    let mut entries = Vec::new();

    for raw in source_paths {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        let source = PathBuf::from(raw);
        let metadata = source_metadata(&source)?;
        let source_name = source
            .file_name()
            .ok_or_else(|| edit_error("invalid_source", "Source path has no file name."))?
            .to_string_lossy()
            .into_owned();
        let root_archive_path = join_archive_path(&parent, &source_name)?;

        if metadata.is_dir() {
            let before = entries.len();
            enumerate_add_directory(&source, &root_archive_path, &mut entries, cancelled)?;
            if entries.len() == before {
                entries.push(DiskSourceEntry {
                    path: source,
                    archive_path: root_archive_path,
                    is_directory: true,
                });
            }
        } else {
            entries.push(DiskSourceEntry {
                path: source,
                archive_path: root_archive_path,
                is_directory: false,
            });
        }
    }

    Ok(entries)
}

fn plan_create_folder(
    members: &[SourceMember],
    folder_path: &str,
) -> Result<Vec<RebuildMember>, CommandError> {
    let folder_path = normalize_and_validate(folder_path)?;
    if entry_hierarchy_collides(members, &folder_path) {
        return Err(CommandError {
            code: "entry_exists".into(),
            message: format!("Folder path already exists in archive: {folder_path}"),
            path: Some(folder_path),
        });
    }

    let mut planned: Vec<RebuildMember> = members
        .iter()
        .map(|member| RebuildMember::Copy {
            index: member.index,
            out_path: member.path.clone(),
            is_dir: member.is_dir,
        })
        .collect();
    planned.push(RebuildMember::NewDirectory { path: folder_path });
    Ok(planned)
}

fn plan_add_paths(
    members: &[SourceMember],
    source_paths: &[String],
    archive_parent: &str,
    zip_path: &Path,
    cancelled: &AtomicBool,
) -> Result<Vec<RebuildMember>, CommandError> {
    let disk_entries = enumerate_add_sources(source_paths, archive_parent, cancelled)?;
    // After enumeration: reject sources that are/contain the open archive.
    reject_add_sources_contain_archive(source_paths, zip_path)?;

    let mut seen_targets = std::collections::HashSet::new();
    for entry in &disk_entries {
        if !seen_targets.insert(entry.archive_path.clone()) {
            return Err(CommandError {
                code: "entry_exists".into(),
                message: format!("Duplicate add target: {}", entry.archive_path),
                path: Some(entry.archive_path.clone()),
            });
        }
        if entry_hierarchy_collides(members, &entry.archive_path) {
            return Err(CommandError {
                code: "entry_exists".into(),
                message: format!("Add target already exists: {}", entry.archive_path),
                path: Some(entry.archive_path.clone()),
            });
        }
    }

    let mut planned: Vec<RebuildMember> = members
        .iter()
        .map(|member| RebuildMember::Copy {
            index: member.index,
            out_path: member.path.clone(),
            is_dir: member.is_dir,
        })
        .collect();

    for entry in disk_entries {
        if entry.is_directory {
            planned.push(RebuildMember::NewDirectory {
                path: entry.archive_path,
            });
        } else {
            planned.push(RebuildMember::NewFile {
                path: entry.archive_path,
                source: entry.path,
            });
        }
    }

    Ok(planned)
}

fn plan_replace_file(
    members: &[SourceMember],
    entry_path: &str,
    source_file: &Path,
) -> Result<Vec<RebuildMember>, CommandError> {
    let entry_path = normalize_and_validate(entry_path)?;
    let existing = members.iter().find(|m| m.path == entry_path);
    match existing {
        None => {
            return Err(CommandError {
                code: "not_found".into(),
                message: format!("Replace target is not in the archive: {entry_path}"),
                path: Some(entry_path),
            });
        }
        Some(member) if member.is_dir => {
            return Err(CommandError {
                code: "invalid_entry".into(),
                message: format!("Replace target is a directory: {entry_path}"),
                path: Some(entry_path),
            });
        }
        Some(_) => {}
    }

    let metadata = source_metadata(source_file)?;
    if !metadata.is_file() {
        return Err(edit_error(
            "invalid_source",
            format!(
                "Replace source is not a regular file: {}",
                source_file.display()
            ),
        ));
    }

    let mut planned = Vec::with_capacity(members.len());
    for member in members {
        if member.path == entry_path {
            planned.push(RebuildMember::NewFile {
                path: entry_path.clone(),
                source: source_file.to_path_buf(),
            });
        } else {
            planned.push(RebuildMember::Copy {
                index: member.index,
                out_path: member.path.clone(),
                is_dir: member.is_dir,
            });
        }
    }
    Ok(planned)
}

fn zip_output_name(path: &str, is_dir: bool) -> String {
    if is_dir {
        format!("{path}/")
    } else {
        path.to_string()
    }
}

fn write_new_file_entry(
    zip: &mut ZipWriter<File>,
    archive_path: &str,
    source: &Path,
    cancelled: &AtomicBool,
) -> Result<(), CommandError> {
    // Re-check links immediately before open (TOCTOU).
    let metadata = source_metadata(source)?;
    if !metadata.is_file() {
        return Err(edit_error(
            "invalid_source",
            format!("Source is not a regular file: {}", source.display()),
        ));
    }

    let mut reader = open_source_file(source).map_err(|error| {
        edit_error(
            "source_read",
            format!("Cannot open source {}: {error}", source.display()),
        )
    })?;
    zip.start_file(archive_path, deflated_file_options())
        .map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot start ZIP file {archive_path}: {error}"),
            )
        })?;

    let mut buffer = [0_u8; BUFFER_SIZE];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }
        let read = reader.read(&mut buffer).map_err(|error| {
            edit_error(
                "source_read",
                format!("Cannot read source {}: {error}", source.display()),
            )
        })?;
        if read == 0 {
            break;
        }
        zip.write_all(&buffer[..read]).map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot write ZIP data for {archive_path}: {error}"),
            )
        })?;
    }
    Ok(())
}

fn rebuild_archive(
    zip_path: &Path,
    planned: &[RebuildMember],
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(edit_error("invalid_operation", "Operation ID is empty."));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let (mut archive, _) = open_source_members(zip_path)?;
    let total_files = planned.len() as u64;
    let (temp_path, temp_file) = create_temporary_edit_archive(zip_path)?;

    let result = (|| -> Result<EditSummary, CommandError> {
        let mut zip = ZipWriter::new(temp_file);
        let mut processed = 0_u64;
        let mut progress_gate = ProgressGate::new();

        for member in planned {
            if cancelled.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }

            let current_file = member.out_path().to_string();
            // First member always; mid-members at most every PROGRESS_INTERVAL; final 100% outside loop.
            if progress_gate.should_emit() {
                emit(OperationProgress {
                    operation_id: operation_id.into(),
                    extracted_files: processed,
                    total_files,
                    current_file: current_file.clone(),
                    percentage: progress_percentage(processed, total_files),
                    phase: Some("rebuild".into()),
                });
            }

            match member {
                RebuildMember::Copy {
                    index,
                    out_path,
                    is_dir,
                } => {
                    let entry = archive.by_index(*index).map_err(|error| {
                        edit_error(
                            "invalid_archive",
                            format!("Failed to read zip entry: {error}"),
                        )
                    })?;
                    let out_name = zip_output_name(out_path, *is_dir);

                    if entry.is_dir() || *is_dir {
                        let options = FileOptions::default();
                        zip.add_directory(&out_name, options).map_err(|error| {
                            edit_error(
                                "write_failed",
                                format!("Cannot write directory entry: {error}"),
                            )
                        })?;
                    } else {
                        zip.raw_copy_file_rename(entry, out_name).map_err(|error| {
                            edit_error(
                                "write_failed",
                                format!("Cannot copy archive entry: {error}"),
                            )
                        })?;
                    }
                }
                RebuildMember::NewDirectory { path } => {
                    let out_name = zip_output_name(path, true);
                    zip.add_directory(&out_name, FileOptions::default())
                        .map_err(|error| {
                            edit_error(
                                "write_failed",
                                format!("Cannot write directory entry: {error}"),
                            )
                        })?;
                }
                RebuildMember::NewFile { path, source } => {
                    write_new_file_entry(&mut zip, path, source, cancelled)?;
                }
            }

            processed += 1;
        }

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let temp_file = zip.finish().map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot finalize temporary ZIP: {error}"),
            )
        })?;
        temp_file.sync_all().map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot sync temporary ZIP: {error}"),
            )
        })?;
        drop(temp_file);
        // Release the source ZIP handle before replacing the file on Windows.
        drop(archive);

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        publish_temp_archive(&temp_path, zip_path).map_err(|error| {
            edit_error(
                "finalize_failed",
                format!("Cannot replace archive with edited copy: {error}"),
            )
        })?;

        Ok(EditSummary {
            operation_id: operation_id.into(),
            destination: zip_path.to_string_lossy().into_owned(),
            members_written: processed,
            strategy_used: Some("rebuild".into()),
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

/// Append only new directory/file members onto a byte-copy of the archive via `ZipWriter::new_append`.
///
/// Does not rewrite existing central-directory entries; fails (caller may fall back to rebuild)
/// on unsupported ZIP features or I/O errors. Never mutates `zip_path` until publish succeeds.
fn append_to_archive(
    zip_path: &Path,
    planned: &[RebuildMember],
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(edit_error("invalid_operation", "Operation ID is empty."));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let to_add = new_members_only(planned);
    if to_add.is_empty() {
        return Err(edit_error(
            "invalid_operation",
            "Append strategy requires at least one new member.",
        ));
    }

    let total_files = to_add.len() as u64;
    let (temp_path, temp_file) = create_temporary_archive_copy(zip_path)?;

    let result = (|| -> Result<EditSummary, CommandError> {
        let mut zip = ZipWriter::new_append(temp_file).map_err(|error| {
            edit_error(
                "append_unsupported",
                format!("Cannot open ZIP for append: {error}"),
            )
        })?;

        let mut processed = 0_u64;
        let mut progress_gate = ProgressGate::new();

        for member in &to_add {
            if cancelled.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }

            let current_file = member.out_path().to_string();
            if progress_gate.should_emit() {
                emit(OperationProgress {
                    operation_id: operation_id.into(),
                    extracted_files: processed,
                    total_files,
                    current_file: current_file.clone(),
                    percentage: progress_percentage(processed, total_files),
                    phase: Some("append".into()),
                });
            }

            match member {
                RebuildMember::NewDirectory { path } => {
                    let out_name = zip_output_name(path, true);
                    zip.add_directory(&out_name, FileOptions::default())
                        .map_err(|error| {
                            edit_error(
                                "write_failed",
                                format!("Cannot write directory entry: {error}"),
                            )
                        })?;
                }
                RebuildMember::NewFile { path, source } => {
                    write_new_file_entry(&mut zip, path, source, cancelled)?;
                }
                RebuildMember::Copy { .. } => unreachable!("filtered by new_members_only"),
            }

            processed += 1;
        }

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let temp_file = zip.finish().map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot finalize temporary ZIP: {error}"),
            )
        })?;
        temp_file.sync_all().map_err(|error| {
            edit_error(
                "write_failed",
                format!("Cannot sync temporary ZIP: {error}"),
            )
        })?;
        drop(temp_file);

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        publish_temp_archive(&temp_path, zip_path).map_err(|error| {
            edit_error(
                "finalize_failed",
                format!("Cannot replace archive with edited copy: {error}"),
            )
        })?;

        Ok(EditSummary {
            operation_id: operation_id.into(),
            destination: zip_path.to_string_lossy().into_owned(),
            members_written: processed,
            strategy_used: Some("append".into()),
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
                phase: Some("append".into()),
            });
            Ok(summary)
        }
        Err(mut error) => {
            cleanup_temp(&temp_path, &mut error);
            Err(error)
        }
    }
}

/// Try ZIP append when strategy allows; on any non-cancel append failure, full rebuild.
fn apply_add_like_edit(
    zip_path: &Path,
    planned: &[RebuildMember],
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    if should_try_append(options) {
        match append_to_archive(zip_path, planned, operation_id, cancelled, &mut emit) {
            Ok(summary) => return Ok(summary),
            Err(error) if error.code == "cancelled" => return Err(error),
            Err(_) => {
                // Fall through to full rebuild (unsupported ZIP features, open/write errors).
            }
        }
    }
    rebuild_archive(zip_path, planned, operation_id, cancelled, emit)
}

/// Logical delete: copy archive to temp, rewrite central directory (orphan local data), publish.
///
/// Never mutates `zip_path` until publish. On cancel, cleans up temp and does not rebuild.
fn logical_delete_archive(
    zip_path: &Path,
    selected: &[String],
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(edit_error("invalid_operation", "Operation ID is empty."));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let (temp_path, mut temp_file) = create_temporary_archive_copy(zip_path)?;

    let result = (|| -> Result<EditSummary, CommandError> {
        emit(OperationProgress {
            operation_id: operation_id.into(),
            extracted_files: 0,
            total_files: 1,
            current_file: "logical_delete".into(),
            percentage: 0.0,
            phase: Some("logical_delete".into()),
        });

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let kept = crate::zip_cd::logical_delete_on_file(&mut temp_file, selected, cancelled)?;
        drop(temp_file);

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        // Validate before replacing the original.
        {
            let validate = File::open(&temp_path).map_err(|error| {
                edit_error(
                    "write_failed",
                    format!("Cannot reopen temporary ZIP for validation: {error}"),
                )
            })?;
            ZipArchive::new(validate).map_err(|error| {
                edit_error(
                    "write_failed",
                    format!("Logical delete produced an invalid ZIP: {error}"),
                )
            })?;
        }

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        publish_temp_archive(&temp_path, zip_path).map_err(|error| {
            edit_error(
                "finalize_failed",
                format!("Cannot replace archive with edited copy: {error}"),
            )
        })?;

        Ok(EditSummary {
            operation_id: operation_id.into(),
            destination: zip_path.to_string_lossy().into_owned(),
            members_written: kept,
            strategy_used: Some("logical_delete".into()),
        })
    })();

    match result {
        Ok(summary) => {
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: summary.members_written,
                total_files: summary.members_written.max(1),
                current_file: "Completed".into(),
                percentage: 100.0,
                phase: Some("logical_delete".into()),
            });
            Ok(summary)
        }
        Err(mut error) => {
            cleanup_temp(&temp_path, &mut error);
            Err(error)
        }
    }
}

fn require_zip_file(zip_path: &Path) -> Result<(), CommandError> {
    if !zip_path.is_file() {
        return Err(edit_error(
            "not_found",
            format!("Archive not found: {}", zip_path.display()),
        ));
    }
    Ok(())
}

/// Deletes archive entries matching `paths` (exact or directory prefix).
///
/// Strategy:
/// - PreferCompact: full rebuild
/// - PreferFast: try logical delete (CD rewrite); non-cancel failure → rebuild
/// - Auto / default: logical when `deleted/total ≤ 0.25` or `deleted ≤ 64`; else rebuild
///
/// Cancel during logical delete cleans up temp and does **not** fall back to rebuild.
pub fn delete_entries(
    zip_path: &Path,
    paths: &[String],
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (_, members) = open_source_members(zip_path)?;
    let planned = plan_delete(&members, paths)?;
    let total = members.len();
    let kept = planned.len();
    let deleted = total.saturating_sub(kept);

    if should_try_logical_delete(options, deleted, total) {
        let selected = normalize_delete_selection(paths)?;
        match logical_delete_archive(zip_path, &selected, operation_id, cancelled, &mut emit) {
            Ok(summary) => return Ok(summary),
            Err(error) if error.code == "cancelled" => return Err(error),
            Err(_) => {
                // PreferFast / Auto: fall through to full rebuild on non-cancel failure.
            }
        }
    }

    rebuild_archive(zip_path, &planned, operation_id, cancelled, emit)
}

/// Renames a file entry or directory prefix inside a ZIP archive.
pub fn rename_entry(
    zip_path: &Path,
    from: &str,
    to: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (_, members) = open_source_members(zip_path)?;
    let planned = plan_rename(&members, from, to)?;
    rebuild_archive(zip_path, &planned, operation_id, cancelled, emit)
}

/// Moves entries into `dest_folder` (leaf names preserved). One ZIP rebuild.
pub fn move_entries(
    zip_path: &Path,
    sources: &[String],
    dest_folder: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (_, members) = open_source_members(zip_path)?;
    let planned = plan_move(&members, sources, dest_folder)?;
    rebuild_archive(zip_path, &planned, operation_id, cancelled, emit)
}

/// Creates an empty directory entry in the ZIP archive.
///
/// Uses append when strategy allows (`Auto` / `PreferFast` / default); falls back to rebuild.
pub fn create_folder(
    zip_path: &Path,
    folder_path: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (_, members) = open_source_members(zip_path)?;
    let planned = plan_create_folder(&members, folder_path)?;
    apply_add_like_edit(zip_path, &planned, operation_id, cancelled, emit, options)
}

/// Adds files and directories from disk into the ZIP under `archive_parent`.
///
/// Directory sources include their root folder name (like create with include_root).
/// Rejects symlink/reparse sources and existing archive targets.
/// Uses append when strategy allows (`Auto` / `PreferFast` / default); falls back to rebuild.
pub fn add_paths(
    zip_path: &Path,
    source_paths: &[String],
    archive_parent: &str,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (_, members) = open_source_members(zip_path)?;
    let planned = plan_add_paths(&members, source_paths, archive_parent, zip_path, cancelled)?;
    apply_add_like_edit(zip_path, &planned, operation_id, cancelled, emit, options)
}

/// Replaces an existing file entry's content from a disk file (same archive path).
pub fn replace_file(
    zip_path: &Path,
    entry_path: &str,
    source_file: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    require_zip_file(zip_path)?;
    let (_, members) = open_source_members(zip_path)?;
    let planned = plan_replace_file(&members, entry_path, source_file)?;
    rebuild_archive(zip_path, &planned, operation_id, cancelled, emit)
}
