use crate::models::CommandError;
use crate::security::is_link_or_reparse_point;
use std::path::{Path, PathBuf};

pub fn split_file_name(file_name: &str) -> (String, String) {
    if let Some(idx) = file_name.rfind('.') {
        if idx > 0 {
            return (file_name[..idx].to_string(), file_name[idx..].to_string());
        }
    }
    (file_name.to_string(), String::new())
}

pub fn candidate_renamed_name(file_name: &str, n: u32) -> String {
    let (stem, ext) = split_file_name(file_name);
    format!("{stem} ({n}){ext}")
}

pub fn unique_renamed_path(parent: &Path, file_name: &str) -> Result<PathBuf, CommandError> {
    for n in 1..=10_000 {
        let name = candidate_renamed_name(file_name, n);
        let candidate = parent.join(&name);
        match std::fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(candidate);
            }
            Ok(metadata) if is_link_or_reparse_point(&metadata) => {
                // Treat reparse candidates as occupied; never rename onto a link.
                continue;
            }
            Ok(_) => continue,
            Err(error) => {
                return Err(CommandError::new(
                    "write_failed",
                    format!("Cannot inspect renamed destination candidate: {error}"),
                ));
            }
        }
    }
    Err(CommandError::new(
        "conflict",
        "Could not find a free renamed path for the conflicting file.",
    ))
}
