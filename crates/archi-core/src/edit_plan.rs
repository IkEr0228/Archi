//! Shared archive edit paths and indexed hierarchy collision checks.
use crate::edit_sources::{
    enumerate_add_sources, reject_add_sources_contain_archive, source_metadata,
};
use crate::extraction::normalize_entry_name;
use crate::models::CommandError;
use crate::security::validate_entry_path;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn copy_members(members: &[SourceMember]) -> Vec<RebuildMember> {
    members
        .iter()
        .map(|member| RebuildMember::Copy {
            index: member.index,
            out_path: member.path.clone(),
            is_dir: member.is_dir,
        })
        .collect()
}

pub(crate) fn plan_create_folder(
    members: &[SourceMember],
    folder_path: &str,
) -> Result<Vec<RebuildMember>, CommandError> {
    let folder_path = normalize_and_validate(folder_path)?;
    let index = PathIndex::new(members.iter().map(|m| (m.path.as_str(), m.is_dir)));
    if index.collides(&folder_path) {
        return Err(CommandError {
            code: "entry_exists".into(),
            message: format!("Folder path already exists in archive: {folder_path}"),
            path: Some(folder_path),
        });
    }
    let mut planned = copy_members(members);
    planned.push(RebuildMember::NewDirectory { path: folder_path });
    Ok(planned)
}

pub(crate) fn plan_add_paths(
    members: &[SourceMember],
    sources: &[String],
    parent: &str,
    archive: &Path,
    cancelled: &AtomicBool,
) -> Result<Vec<RebuildMember>, CommandError> {
    let entries = enumerate_add_sources(sources, parent, cancelled)?;
    reject_add_sources_contain_archive(sources, archive)?;
    let existing = PathIndex::new(members.iter().map(|m| (m.path.as_str(), m.is_dir)));
    let mut targets = BTreeMap::new();
    for entry in &entries {
        let path = &entry.archive_path;
        if targets.insert(path.as_str(), entry.is_directory).is_some() {
            return Err(CommandError {
                code: "entry_exists".into(),
                message: format!("Duplicate add target: {path}"),
                path: Some(path.clone()),
            });
        }
        if existing.collides(path) {
            return Err(CommandError {
                code: "entry_exists".into(),
                message: format!("Add target already exists: {path}"),
                path: Some(path.clone()),
            });
        }
    }
    // Different disk roots can introduce a file-as-parent collision within the add set.
    for path in targets.keys() {
        if path
            .match_indices('/')
            .any(|(offset, _)| targets.get(&path[..offset]) == Some(&false))
        {
            return Err(CommandError {
                code: "entry_exists".into(),
                message: format!("Add target has a file as parent: {path}"),
                path: Some((*path).into()),
            });
        }
    }
    let mut planned = copy_members(members);
    planned.extend(entries.into_iter().map(|entry| {
        if entry.is_directory {
            RebuildMember::NewDirectory {
                path: entry.archive_path,
            }
        } else {
            RebuildMember::NewFile {
                path: entry.archive_path,
                source: entry.path,
            }
        }
    }));
    Ok(planned)
}

pub(crate) fn plan_replace_file(
    members: &[SourceMember],
    entry_path: &str,
    source: &Path,
) -> Result<Vec<RebuildMember>, CommandError> {
    let entry_path = normalize_and_validate(entry_path)?;
    let existing = members
        .iter()
        .find(|m| m.path == entry_path)
        .ok_or_else(|| CommandError {
            code: "not_found".into(),
            message: format!("Replace target is not in the archive: {entry_path}"),
            path: Some(entry_path.clone()),
        })?;
    if existing.is_dir {
        return Err(CommandError {
            code: "invalid_entry".into(),
            message: format!("Replace target is a directory: {entry_path}"),
            path: Some(entry_path),
        });
    }
    if !source_metadata(source)?.is_file() {
        return Err(CommandError::new(
            "invalid_source",
            format!("Replace source is not a regular file: {}", source.display()),
        ));
    }
    Ok(members
        .iter()
        .map(|m| {
            if m.path == entry_path {
                RebuildMember::NewFile {
                    path: entry_path.clone(),
                    source: source.to_path_buf(),
                }
            } else {
                RebuildMember::Copy {
                    index: m.index,
                    out_path: m.path.clone(),
                    is_dir: m.is_dir,
                }
            }
        })
        .collect())
}

pub(crate) struct SourceMember {
    pub index: usize,
    pub path: String,
    pub is_dir: bool,
}

pub(crate) fn plan_rename(
    members: &[SourceMember],
    from: &str,
    to: &str,
) -> Result<Vec<RebuildMember>, CommandError> {
    let from = normalize_and_validate(from)?;
    let to = normalize_and_validate(to)?;
    if from == to {
        return Err(CommandError::new(
            "invalid_entry",
            "Rename source and destination are the same.",
        ));
    }
    if !members.iter().any(|m| selection_matches(&m.path, &from)) {
        return Err(CommandError {
            code: "not_found".into(),
            message: format!("Rename source is not in the archive: {from}"),
            path: Some(from),
        });
    }
    let planned: Vec<_> = members
        .iter()
        .map(|m| {
            let out_path = if selection_matches(&m.path, &from) {
                format!("{to}{}", &m.path[from.len()..])
            } else {
                m.path.clone()
            };
            RebuildMember::Copy {
                index: m.index,
                out_path,
                is_dir: m.is_dir,
            }
        })
        .collect();
    validate_rewrite(members.iter().map(|m| m.path.as_str()), &planned, "Rename")?;
    Ok(planned)
}

pub(crate) fn selection_matches(entry: &str, selected: &str) -> bool {
    entry == selected
        || entry
            .strip_prefix(selected)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

pub(crate) fn plan_delete(
    members: &[SourceMember],
    paths: &[String],
) -> Result<Vec<RebuildMember>, CommandError> {
    if paths.is_empty() {
        return Err(CommandError::new(
            "invalid_selection",
            "No archive paths specified for delete.",
        ));
    }
    let paths: Vec<_> = paths
        .iter()
        .map(|p| normalize_and_validate(p))
        .collect::<Result<_, _>>()?;
    let selection = crate::extraction::SelectionIndex::from_selected(&paths)?;
    let planned: Vec<_> = members
        .iter()
        .filter(|m| !selection.includes_normalized(&m.path))
        .map(|m| RebuildMember::Copy {
            index: m.index,
            out_path: m.path.clone(),
            is_dir: m.is_dir,
        })
        .collect();
    if planned.len() == members.len() {
        return Err(CommandError::new(
            "not_found",
            "Delete selection matched no archive entries.",
        ));
    }
    Ok(planned)
}

/// Sources are matched against original paths, never rewritten paths (no cascading moves).
pub(crate) fn plan_move(
    members: &[SourceMember],
    sources: &[String],
    destination: &str,
) -> Result<Vec<RebuildMember>, CommandError> {
    if sources.is_empty() {
        return Err(CommandError::new(
            "invalid_selection",
            "No archive paths specified for move.",
        ));
    }
    let destination = if destination.is_empty() || destination == "/" {
        String::new()
    } else {
        normalize_and_validate(destination)?
    };
    let paths: HashSet<String> = sources
        .iter()
        .map(|p| normalize_and_validate(p))
        .collect::<Result<_, _>>()?;
    let mut tops: Vec<_> = paths
        .iter()
        .filter(|path| {
            !path
                .match_indices('/')
                .any(|(offset, _)| paths.contains(&path[..offset]))
        })
        .collect();
    tops.sort();
    let index = PathIndex::new(members.iter().map(|m| (m.path.as_str(), m.is_dir)));
    let mut moves = BTreeMap::new();
    for from in tops {
        let leaf = from.rsplit('/').next().unwrap();
        let to = if destination.is_empty() {
            leaf.to_string()
        } else {
            format!("{destination}/{leaf}")
        };
        if *from == to {
            continue;
        }
        if selection_matches(&destination, from) {
            return Err(CommandError::new(
                "invalid_entry",
                format!("Cannot move '{from}' into itself or a subfolder."),
            ));
        }
        if !index.contains_tree(from) {
            return Err(CommandError {
                code: "not_found".into(),
                message: format!("Move source is not in the archive: {from}"),
                path: Some(from.clone()),
            });
        }
        moves.insert(from.as_str(), to);
    }
    if moves.is_empty() {
        return Err(CommandError::new(
            "invalid_entry",
            "Nothing to move (sources already at destination).",
        ));
    }
    let planned: Vec<_> = members
        .iter()
        .map(|member| {
            let source = moves.get_key_value(member.path.as_str()).or_else(|| {
                member
                    .path
                    .match_indices('/')
                    .find_map(|(offset, _)| moves.get_key_value(&member.path[..offset]))
            });
            let out_path = match source {
                Some((from, to)) => format!("{to}{}", &member.path[from.len()..]),
                None => member.path.clone(),
            };
            RebuildMember::Copy {
                index: member.index,
                out_path,
                is_dir: member.is_dir,
            }
        })
        .collect();
    validate_rewrite(members.iter().map(|m| m.path.as_str()), &planned, "Move")?;
    Ok(planned)
}

#[derive(Clone)]
pub(crate) enum RebuildMember {
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
    pub(crate) fn out_path(&self) -> &str {
        match self {
            Self::Copy { out_path, .. } => out_path,
            Self::NewDirectory { path } | Self::NewFile { path, .. } => path,
        }
    }
}

pub(crate) fn normalize_and_validate(path: &str) -> Result<String, CommandError> {
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

/// Exact/descendant queries O(log N); file-as-parent queries O(depth log N).
pub(crate) struct PathIndex<'a>(BTreeMap<&'a str, bool>);
impl<'a> PathIndex<'a> {
    pub(crate) fn new(paths: impl IntoIterator<Item = (&'a str, bool)>) -> Self {
        let mut index = BTreeMap::new();
        for (path, is_dir) in paths {
            // Duplicate file+directory names must retain the stricter file-as-parent rule.
            index
                .entry(path)
                .and_modify(|value| *value &= is_dir)
                .or_insert(is_dir);
        }
        Self(index)
    }

    pub(crate) fn collides(&self, path: &str) -> bool {
        if self.contains_tree(path) {
            return true;
        }
        path.match_indices('/')
            .any(|(offset, _)| self.0.get(&path[..offset]) == Some(&false))
    }

    fn contains_tree(&self, path: &str) -> bool {
        if self.0.contains_key(path) {
            return true;
        }
        let prefix = format!("{path}/");
        self.0
            .range(prefix.as_str()..)
            .next()
            .is_some_and(|(key, _)| key.starts_with(&prefix))
    }
}

/// Rename/move preserve member order and source index. No index lookup per output member.
pub(crate) fn validate_rewrite<'a>(
    originals: impl IntoIterator<Item = &'a str>,
    planned: &[RebuildMember],
    verb: &str,
) -> Result<(), CommandError> {
    let originals: Vec<_> = originals.into_iter().collect();
    assert_eq!(
        originals.len(),
        planned.len(),
        "Rewrite must preserve member order and count"
    );
    let kept = PathIndex::new(originals.iter().copied().zip(planned).filter_map(
        |(original, member)| match member {
            RebuildMember::Copy {
                out_path, is_dir, ..
            } if out_path == original => Some((original, *is_dir)),
            _ => None,
        },
    ));
    let mut seen = HashSet::with_capacity(planned.len());
    for (original, member) in originals.into_iter().zip(planned) {
        let path = member.out_path();
        // Unchanged members already exist in kept; query only rewritten paths.
        let unchanged = path == original;
        if (!unchanged && kept.collides(path)) || !seen.insert(path) {
            return Err(CommandError {
                code: "entry_exists".into(),
                message: format!("{verb} destination already exists: {path}"),
                path: Some(path.into()),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_matches_linear_hierarchy_rules() {
        let entries = [
            ("a", true),
            ("a/file", false),
            ("a-extra/child", false),
            ("z", false),
            ("unicode/я", false),
            ("a!", false),
        ];
        let index = PathIndex::new(entries);
        for path in [
            "a",
            "a/file",
            "a/file/child",
            "a/new",
            "a-extra",
            "a-ex",
            "z/child",
            "unicode",
            "unicode/я/child",
            "free",
        ] {
            let expected = entries.iter().any(|(kept, dir)| {
                *kept == path
                    || kept
                        .strip_prefix(path)
                        .is_some_and(|suffix| suffix.starts_with('/'))
                    || (!dir
                        && path
                            .strip_prefix(kept)
                            .is_some_and(|suffix| suffix.starts_with('/')))
            });
            assert_eq!(index.collides(path), expected, "{path}");
        }
    }

    fn members(paths: &[(&str, bool)]) -> Vec<SourceMember> {
        paths
            .iter()
            .enumerate()
            .map(|(index, (path, is_dir))| SourceMember {
                index,
                path: (*path).into(),
                is_dir: *is_dir,
            })
            .collect()
    }

    #[test]
    fn move_matches_original_paths_without_cascade() {
        let source = members(&[("a/file", false), ("destination/a/file2", false)]);
        let planned = plan_move(
            &source,
            &["a".into(), "destination/a".into()],
            "destination",
        )
        .unwrap();
        assert_eq!(
            planned
                .iter()
                .map(RebuildMember::out_path)
                .collect::<Vec<_>>(),
            ["destination/a/file", "destination/a/file2"]
        );
    }

    #[test]
    fn nested_move_selection_and_hierarchy_collisions() {
        let source = members(&[("a/file", false), ("ab/file", false), ("block", false)]);
        let moved = plan_move(&source, &["a".into(), "a/file".into()], "new").unwrap();
        assert_eq!(moved[0].out_path(), "new/a/file");
        assert_eq!(moved[1].out_path(), "ab/file");
        assert_eq!(
            plan_move(&source, &["a".into()], "a/sub")
                .err()
                .unwrap()
                .code,
            "invalid_entry"
        );
        assert_eq!(
            plan_rename(&source, "a", "block/sub").err().unwrap().code,
            "entry_exists"
        );
        assert_eq!(
            plan_rename(&source, "a/file", "ab").err().unwrap().code,
            "entry_exists"
        );
        let deleted = plan_delete(&source, &["a".into()]).unwrap();
        assert_eq!(deleted.len(), 2);
        assert_eq!(deleted[0].out_path(), "ab/file");
    }
}
