use crate::bzip2_format::open_bzip2;
use crate::format_detect::{detect_format, ArchiveFormat};
use crate::gzip_format::open_gzip;
use crate::models::{ArchiveCapabilities, ArchiveEntry, ArchiveInfo, ArchiveStats, CommandError};
use crate::security::{assess_archive, validate_entry_path, ArchiveRiskInput};
use crate::sevenz_format::open_sevenz;
use crate::tar_format::{open_tar, open_tar_bz2, open_tar_gz, open_tar_xz};
use crate::xz_format::open_xz;
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::path::Path;
use zip::ZipArchive;

/// Opens an archive (zip / tar / compressed tar / single-stream) and returns listing metadata.
pub fn open_archive(path: &Path) -> Result<ArchiveInfo, CommandError> {
    if !path.is_file() {
        return Err(CommandError::new(
            "not_found",
            "File not found or is not a file.",
        ));
    }

    match detect_format(path)? {
        ArchiveFormat::Zip => open_zip_archive(path),
        ArchiveFormat::Tar => open_tar(path),
        ArchiveFormat::TarGz => open_tar_gz(path),
        ArchiveFormat::Gzip => open_gzip(path),
        ArchiveFormat::TarBz2 => open_tar_bz2(path),
        ArchiveFormat::Bzip2 => open_bzip2(path),
        ArchiveFormat::TarXz => open_tar_xz(path),
        ArchiveFormat::Xz => open_xz(path),
        ArchiveFormat::SevenZ => open_sevenz(path),
    }
}

fn zip_method_label(method: zip::CompressionMethod) -> String {
    use zip::CompressionMethod::*;
    // Without zip crate bzip2/zstd/aes features, those methods arrive as Unsupported(n).
    // Keep readable labels for listing; extract/test fail with zip's clear UnsupportedArchive.
    match method {
        Stored => "Stored".into(),
        Deflated => "Deflated".into(),
        #[allow(deprecated)]
        Unsupported(12) => "Bzip2".into(),
        #[allow(deprecated)]
        Unsupported(93) => "Zstd".into(),
        #[allow(deprecated)]
        Unsupported(99) => "AES".into(),
        other => format!("{other:?}"),
    }
}

fn open_zip_archive(path: &Path) -> Result<ArchiveInfo, CommandError> {
    let file = File::open(path).map_err(|error| {
        CommandError::new("invalid_archive", format!("Failed to open file: {error}"))
    })?;
    let mut zip = ZipArchive::new(file).map_err(|error| {
        CommandError::new(
            "invalid_archive",
            format!("Cannot open or read ZIP structure: {error}"),
        )
    })?;

    let zip_len = zip.len();
    // Virtual parents roughly double entry count in deep trees; reserve modestly.
    let reserve = zip_len.saturating_mul(2).max(16);
    let mut entries: Vec<ArchiveEntry> = Vec::with_capacity(reserve);
    let mut entry_indices: HashMap<String, usize> = HashMap::with_capacity(reserve);
    let mut total_uncompressed: u64 = 0;
    let mut total_compressed: u64 = 0;
    let mut largest_entry: u64 = 0;
    let mut deepest_path = 0;

    for i in 0..zip_len {
        let file = zip.by_index(i).map_err(|error| {
            CommandError::new(
                "invalid_archive",
                format!("Failed to read zip entry: {error}"),
            )
        })?;
        let raw_name = file.name();
        validate_entry_path(raw_name).map_err(|message| CommandError {
            code: "invalid_entry".into(),
            message,
            path: Some(raw_name.into()),
        })?;

        // Normalize path: replace backslash, strip trailing slash for logical paths
        let mut normalized = raw_name.replace('\\', "/");
        let is_dir = file.is_dir() || raw_name.ends_with('/') || raw_name.ends_with('\\');
        normalized.truncate(normalized.trim_end_matches('/').len());

        if normalized.is_empty() {
            return Err(CommandError {
                code: "invalid_entry".into(),
                message: "Archive entry path is empty or malformed.".into(),
                path: Some(raw_name.into()),
            });
        }

        let file_size = file.size();
        let file_compressed = file.compressed_size();
        total_uncompressed = total_uncompressed.saturating_add(file_size);
        total_compressed = total_compressed.saturating_add(file_compressed);
        largest_entry = largest_entry.max(file_size);
        deepest_path = deepest_path.max(normalized.bytes().filter(|&b| b == b'/').count() + 1);
        let modified_at = {
            let dt = file.last_modified();
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                dt.year(),
                dt.month(),
                dt.day(),
                dt.hour(),
                dt.minute(),
                dt.second()
            )
        };
        let method_label = if is_dir {
            None
        } else {
            Some(zip_method_label(file.compression()))
        };

        // Generate parent directories (virtual directories)
        let parts: Vec<&str> = normalized.split('/').collect();
        let mut current_prefix = String::with_capacity(normalized.len());

        for j in 0..parts.len() {
            let part = parts[j];
            let parent = if current_prefix.is_empty() {
                "/".to_string()
            } else {
                current_prefix.clone()
            };

            if !current_prefix.is_empty() {
                current_prefix.push('/');
            }
            current_prefix.push_str(part);

            let component_is_dir = j < parts.len() - 1 || is_dir;

            if let Some(&entry_index) = entry_indices.get(&current_prefix) {
                if j == parts.len() - 1 && is_dir {
                    let entry = &mut entries[entry_index];
                    entry.is_directory = true;
                    entry.uncompressed_size = 0;
                    entry.compressed_size = None;
                    entry.modified_at = Some(modified_at.clone());
                    entry.method = None;
                }
            } else {
                let uncompressed_size = if component_is_dir { 0 } else { file_size };
                let compressed_size = if component_is_dir {
                    None
                } else {
                    Some(file_compressed)
                };

                entries.push(ArchiveEntry {
                    path: current_prefix.clone(),
                    name: part.to_string(),
                    parent_path: parent,
                    is_directory: component_is_dir,
                    uncompressed_size,
                    compressed_size,
                    modified_at: (j == parts.len() - 1).then(|| modified_at.clone()),
                    method: if component_is_dir {
                        None
                    } else {
                        method_label.clone()
                    },
                });
                entry_indices.insert(current_prefix.clone(), entries.len() - 1);
            }
        }
    }

    let mut file_count = 0_u64;
    let mut folder_count = 0_u64;
    let mut methods = BTreeSet::new();
    for entry in &entries {
        if entry.is_directory {
            folder_count += 1;
        } else {
            file_count += 1;
            if let Some(method) = &entry.method {
                methods.insert(method.clone());
            }
        }
    }

    Ok(ArchiveInfo {
        archive_path: path.to_string_lossy().into_owned(),
        format: "zip".into(),
        entries,
        capabilities: ArchiveCapabilities {
            open: true,
            list: true,
            extract: true,
            create: true,
            edit: true,
            encrypt: false,
            test: true,
        },
        warnings: assess_archive(ArchiveRiskInput {
            entry_count: zip.len(),
            total_uncompressed,
            total_compressed,
            largest_entry,
            deepest_path,
        }),
        stats: ArchiveStats {
            file_count,
            folder_count,
            total_uncompressed,
            total_compressed,
            methods: methods.into_iter().collect(),
        },
    })
}
