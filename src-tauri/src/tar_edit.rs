//! TAR family stream-rebuild edit (plain / gz / bz2 / xz).
//! Plan Keep/Rename/Drop/New → sequential decode → re-encode to temp → atomic publish.
//! No full work-tree extract for pure metadata ops (rename/delete/move/mkdir).

use crate::create_common::{
    cleanup_temp, create_temporary_archive, member_path_for_tar, open_source_file,
    progress_percentage, publish_temp_archive, CancellableRead, ProgressGate,
};
use crate::extraction::normalize_entry_name;
use crate::format_detect::{detect_format, ArchiveFormat};
use crate::models::{CommandError, CompressionPreset, EditOptions, EditSummary, OperationProgress};
use crate::security::{is_link_or_reparse_point, validate_entry_path};
use bzip2::read::BzDecoder;
use bzip2::write::BzEncoder;
use bzip2::Compression as BzCompression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression as GzCompression;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tar::{Archive, Builder, EntryType, Header};
use xz2::read::XzDecoder;
use xz2::write::XzEncoder;

use crate::io_perf::IO_BUFFER_SIZE as BUFFER_SIZE;

#[derive(Clone)]
enum RebuildMember {
    /// Copy existing member (by source index) under `out_path`.
    Copy {
        index: usize,
        out_path: String,
        is_dir: bool,
    },
    NewDirectory {
        path: String,
    },
    NewFile {
        path: String,
        source: PathBuf,
    },
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
    keep_map: &HashMap<String, (String, bool)>,
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

    let mut keep_map: HashMap<String, (String, bool)> = HashMap::new();
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
