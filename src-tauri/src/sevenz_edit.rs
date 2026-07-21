//! 7z stream-rebuild edit (non-solid): plan Keep/Rename/Drop/New → decode → re-encode.
//! Solid archives fall back to extract → mutate → recreate (repack) at Normal compression.

use crate::create_common::{
    cleanup_temp, create_temporary_archive, member_path_for_tar, open_source_file,
    progress_percentage, publish_temp_archive, ProgressGate,
};
use crate::extraction::{extract_any, normalize_entry_name, FailOnConflict};
use crate::models::{
    CommandError, CompressionPreset, CreateFormat, CreateOptions, EditOptions, EditSummary,
    OperationProgress,
};
use crate::security::{is_link_or_reparse_point, validate_entry_path};
use crate::sevenz_format::create_sevenz_archive;
use sevenz_rust2::encoder_options::Lzma2Options;
use sevenz_rust2::{ArchiveEntry as SzEntry, ArchiveReader, ArchiveWriter, Password};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::io_perf::IO_BUFFER_SIZE as BUFFER_SIZE;

#[derive(Clone)]
enum RebuildMember {
    /// Decode existing archive member and re-encode under `out_path`.
    Copy {
        /// Index into the source member list (not archive.files raw index).
        index: usize,
        out_path: String,
        is_dir: bool,
    },
    NewDirectory { path: String },
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

struct SourceMember {
    /// Normalized path without trailing slash.
    path: String,
    is_dir: bool,
}

struct DiskSourceEntry {
    path: PathBuf,
    archive_path: String,
    is_directory: bool,
}

fn edit_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn cancelled_error() -> CommandError {
    edit_error("cancelled", "Archive edit was cancelled.")
}

fn map_sz_error(error: sevenz_rust2::Error) -> CommandError {
    use sevenz_rust2::Error as E;
    match &error {
        E::PasswordRequired | E::MaybeBadPassword(_) => edit_error(
            "password_required",
            "Encrypted 7z archives are not supported yet. Open an unencrypted archive.",
        ),
        _ => {
            let message = error.to_string();
            let lower = message.to_ascii_lowercase();
            if lower.contains("password") || lower.contains("encrypt") {
                return edit_error(
                    "password_required",
                    "Encrypted 7z archives are not supported yet. Open an unencrypted archive.",
                );
            }
            if lower.contains("cancelled") {
                return cancelled_error();
            }
            edit_error("invalid_archive", format!("7z error: {message}"))
        }
    }
}

fn sz_cb_err(msg: impl Into<String>) -> sevenz_rust2::Error {
    sevenz_rust2::Error::Other(msg.into().into())
}

fn lzma2_level(preset: CompressionPreset) -> u32 {
    match preset {
        CompressionPreset::Store => 0,
        CompressionPreset::Fast => 3,
        CompressionPreset::Normal => 5,
        CompressionPreset::Max => 9,
    }
}

fn edit_compression(options: &EditOptions) -> CompressionPreset {
    options.compression.unwrap_or(CompressionPreset::Normal)
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
fn open_source_members(path: &Path) -> Result<(bool, Vec<SourceMember>), CommandError> {
    let reader = ArchiveReader::open(path, Password::empty()).map_err(map_sz_error)?;
    let is_solid = reader.archive().is_solid;
    let mut members = Vec::with_capacity(reader.archive().files.len());
    for file in &reader.archive().files {
        if file.is_anti_item {
            continue;
        }
        let path = normalize_member_name(file.name())?;
        members.push(SourceMember {
            path,
            is_dir: file.is_directory,
        });
    }
    Ok((is_solid, members))
}

fn selection_matches(entry_path: &str, selected: &str) -> bool {
    entry_path == selected || entry_path.starts_with(&(selected.to_owned() + "/"))
}

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
    for (index, member) in members.iter().enumerate() {
        let delete = selected
            .iter()
            .any(|sel| selection_matches(&member.path, sel));
        if delete {
            matched = true;
        } else {
            planned.push(RebuildMember::Copy {
                index,
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

    for (index, member) in members.iter().enumerate() {
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
            index,
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
                let original = members.get(*index).map(|m| m.path.as_str()).unwrap_or("");
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

    let mut seen = HashSet::new();
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

    let mut from_paths: Vec<String> = Vec::new();
    for raw in sources {
        from_paths.push(normalize_and_validate(raw)?);
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
        if !dest.is_empty() && (dest == *from || dest.starts_with(&(from.clone() + "/"))) {
            return Err(edit_error(
                "invalid_entry",
                format!("Cannot move '{from}' into itself or a subfolder."),
            ));
        }
        let exists = members
            .iter()
            .any(|m| m.path == *from || m.path.starts_with(&(from.clone() + "/")));
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

    for (index, member) in members.iter().enumerate() {
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
            index,
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
                let original = members.get(*index).map(|m| m.path.as_str()).unwrap_or("");
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

    let mut seen = HashSet::new();
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

fn reject_add_sources_contain_archive(
    source_paths: &[String],
    archive_path: &Path,
) -> Result<(), CommandError> {
    let canonical_archive = archive_path.canonicalize().map_err(|error| {
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
        if canonical_archive == canonical_source
            || (metadata.is_dir() && canonical_archive.starts_with(&canonical_source))
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

    if child_count == 0 && !archive_prefix.is_empty() {
        entries.push(DiskSourceEntry {
            path: source.to_path_buf(),
            archive_path: archive_prefix.to_string(),
            is_directory: true,
        });
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
        .enumerate()
        .map(|(index, member)| RebuildMember::Copy {
            index,
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
    archive_path: &Path,
    cancelled: &AtomicBool,
) -> Result<Vec<RebuildMember>, CommandError> {
    let disk_entries = enumerate_add_sources(source_paths, archive_parent, cancelled)?;
    reject_add_sources_contain_archive(source_paths, archive_path)?;

    let mut seen_targets = HashSet::new();
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
        .enumerate()
        .map(|(index, member)| RebuildMember::Copy {
            index,
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
    for (index, member) in members.iter().enumerate() {
        if member.path == entry_path {
            planned.push(RebuildMember::NewFile {
                path: entry_path.clone(),
                source: source_file.to_path_buf(),
            });
        } else {
            planned.push(RebuildMember::Copy {
                index,
                out_path: member.path.clone(),
                is_dir: member.is_dir,
            });
        }
    }
    Ok(planned)
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
    mut emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(edit_error("invalid_operation", "Operation ID is empty."));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    // source path → (out_path, is_dir) for kept/renamed members.
    let mut keep_map: HashMap<String, (String, bool)> = HashMap::new();
    let mut new_members: Vec<&RebuildMember> = Vec::new();
    for member in planned {
        match member {
            RebuildMember::Copy {
                index,
                out_path,
                is_dir,
            } => {
                let src = members
                    .get(*index)
                    .ok_or_else(|| edit_error("invalid_archive", "Planned copy index out of range."))?;
                keep_map.insert(src.path.clone(), (out_path.clone(), *is_dir));
            }
            RebuildMember::NewDirectory { .. } | RebuildMember::NewFile { .. } => {
                new_members.push(member);
            }
        }
    }

    let total_files = planned.len() as u64;
    let (temp_path, temp_file) = create_temporary_archive(archive_path)?;
    let level = lzma2_level(compression);

    let result = (|| -> Result<EditSummary, CommandError> {
        let mut writer = ArchiveWriter::new(temp_file).map_err(map_sz_error)?;
        writer.set_content_methods(vec![Lzma2Options::from_level(level).into()]);

        let mut processed = 0_u64;
        let mut progress_gate = ProgressGate::new();
        let mut kept_written: HashSet<String> = HashSet::new();

        let mut reader = ArchiveReader::open(archive_path, Password::empty()).map_err(map_sz_error)?;

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
            edit_error(
                "write_failed",
                format!("Cannot sync temporary 7z: {error}"),
            )
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
        fs::create_dir_all(parent).map_err(|e| {
            edit_error("write_failed", format!("Cannot create rename parent: {e}"))
        })?;
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
            &FailOnConflict,
            |mut p| {
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
        };
        let sources = vec![work.to_string_lossy().into_owned()];
        let summary = create_sevenz_archive(
            &sources,
            archive_path,
            operation_id,
            cancelled,
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

fn apply_planned(
    archive_path: &Path,
    is_solid: bool,
    members: &[SourceMember],
    planned: Vec<RebuildMember>,
    operation_id: &str,
    cancelled: &AtomicBool,
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
            options,
            solid_mutate,
        )
    } else {
        stream_rebuild(
            archive_path,
            members,
            &planned,
            operation_id,
            cancelled,
            edit_compression(options),
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
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path)?;
    let planned = plan_delete(&members, paths)?;
    let paths = paths.to_vec();
    apply_planned(
        archive_path,
        is_solid,
        &members,
        planned,
        operation_id,
        cancelled,
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
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path)?;
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
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path)?;
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
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path)?;
    let planned = plan_create_folder(&members, folder_path)?;
    let folder_path = folder_path.to_string();
    apply_planned(
        archive_path,
        is_solid,
        &members,
        planned,
        operation_id,
        cancelled,
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
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path)?;
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
        options,
        emit,
        move |work| apply_add(work, &archive_parent, &source_paths),
    )
}

/// Replaces an existing file entry's content from a disk file (same archive path).
pub fn replace_file(
    archive_path: &Path,
    entry_path: &str,
    source_file: &Path,
    operation_id: &str,
    cancelled: &AtomicBool,
    emit: impl FnMut(OperationProgress),
    options: &EditOptions,
) -> Result<EditSummary, CommandError> {
    require_sevenz_file(archive_path)?;
    let (is_solid, members) = open_source_members(archive_path)?;
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
        options,
        emit,
        move |work| apply_replace(work, &entry_path, &source_file),
    )
}
