//! ZIP central-directory helpers for logical delete (CD rewrite without touching local data).
//!
//! Implements APPNOTE EOCD / Zip64 EOCD+locator / central directory file headers from the public
//! format (does not use private `zip::spec`).

use crate::extraction::normalize_entry_name;
use crate::models::CommandError;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};

const EOCD_SIGNATURE: u32 = 0x0605_4b50;
const ZIP64_EOCD_SIGNATURE: u32 = 0x0606_4b50;
const ZIP64_EOCD_LOCATOR_SIGNATURE: u32 = 0x0706_4b50;
const CENTRAL_DIRECTORY_HEADER_SIGNATURE: u32 = 0x0201_4b50;

const EOCD_MIN_SIZE: u64 = 22;
const ZIP64_EOCD_LOCATOR_SIZE: u64 = 20;
/// Fixed-size Zip64 EOCD (signature + size + fields through CD offset), no extensible data.
const ZIP64_EOCD_FIXED_SIZE: u64 = 56;
const CD_HEADER_FIXED_SIZE: usize = 46;

fn cd_error(code: &str, message: impl Into<String>) -> CommandError {
    CommandError::new(code, message)
}

fn cancelled_error() -> CommandError {
    cd_error("cancelled", "Archive edit was cancelled.")
}

fn read_exact_at(file: &mut File, pos: u64, buf: &mut [u8]) -> io::Result<()> {
    file.seek(SeekFrom::Start(pos))?;
    file.read_exact(buf)
}

fn read_u16_le(buf: &[u8]) -> u16 {
    u16::from_le_bytes([buf[0], buf[1]])
}

fn read_u32_le(buf: &[u8]) -> u32 {
    u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])
}

fn read_u64_le(buf: &[u8]) -> u64 {
    u64::from_le_bytes([
        buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7],
    ])
}

fn write_u16_le(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn write_u32_le(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn write_u64_le(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

#[derive(Debug, Clone)]
struct Eocd {
    disk_number: u16,
    disk_with_central_directory: u16,
    number_of_files_on_this_disk: u16,
    number_of_files: u16,
    central_directory_size: u32,
    central_directory_offset: u32,
    comment: Vec<u8>,
    /// File offset where the EOCD signature starts.
    start_pos: u64,
}

impl Eocd {
    fn record_too_small(&self) -> bool {
        self.disk_number == 0xFFFF
            || self.disk_with_central_directory == 0xFFFF
            || self.number_of_files_on_this_disk == 0xFFFF
            || self.number_of_files == 0xFFFF
            || self.central_directory_size == 0xFFFF_FFFF
            || self.central_directory_offset == 0xFFFF_FFFF
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(EOCD_MIN_SIZE as usize + self.comment.len());
        write_u32_le(&mut out, EOCD_SIGNATURE);
        write_u16_le(&mut out, self.disk_number);
        write_u16_le(&mut out, self.disk_with_central_directory);
        write_u16_le(&mut out, self.number_of_files_on_this_disk);
        write_u16_le(&mut out, self.number_of_files);
        write_u32_le(&mut out, self.central_directory_size);
        write_u32_le(&mut out, self.central_directory_offset);
        write_u16_le(&mut out, self.comment.len() as u16);
        out.extend_from_slice(&self.comment);
        out
    }
}

#[derive(Debug, Clone)]
struct Zip64Eocd {
    version_made_by: u16,
    version_needed_to_extract: u16,
    disk_number: u32,
    disk_with_central_directory: u32,
    number_of_files_on_this_disk: u64,
    number_of_files: u64,
    central_directory_size: u64,
    central_directory_offset: u64,
}

impl Zip64Eocd {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(ZIP64_EOCD_FIXED_SIZE as usize);
        write_u32_le(&mut out, ZIP64_EOCD_SIGNATURE);
        // Size of the rest of the record excluding signature and this size field.
        write_u64_le(&mut out, 44);
        write_u16_le(&mut out, self.version_made_by);
        write_u16_le(&mut out, self.version_needed_to_extract);
        write_u32_le(&mut out, self.disk_number);
        write_u32_le(&mut out, self.disk_with_central_directory);
        write_u64_le(&mut out, self.number_of_files_on_this_disk);
        write_u64_le(&mut out, self.number_of_files);
        write_u64_le(&mut out, self.central_directory_size);
        write_u64_le(&mut out, self.central_directory_offset);
        out
    }
}

fn encode_zip64_locator(zip64_eocd_offset: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(ZIP64_EOCD_LOCATOR_SIZE as usize);
    write_u32_le(&mut out, ZIP64_EOCD_LOCATOR_SIGNATURE);
    write_u32_le(&mut out, 0); // disk with Zip64 EOCD
    write_u64_le(&mut out, zip64_eocd_offset);
    write_u32_le(&mut out, 1); // total number of disks
    out
}

/// Locate and parse the standard EOCD (comment preserved).
fn find_eocd(file: &mut File) -> Result<Eocd, CommandError> {
    let file_len = file.seek(SeekFrom::End(0)).map_err(|error| {
        cd_error(
            "invalid_archive",
            format!("Cannot determine ZIP size: {error}"),
        )
    })?;
    if file_len < EOCD_MIN_SIZE {
        return Err(cd_error(
            "invalid_archive",
            "ZIP is too small to contain an end-of-central-directory record.",
        ));
    }

    let max_back = EOCD_MIN_SIZE + u16::MAX as u64;
    let search_start = file_len.saturating_sub(max_back);
    let search_len = (file_len - search_start) as usize;
    let mut tail = vec![0_u8; search_len];
    read_exact_at(file, search_start, &mut tail).map_err(|error| {
        cd_error(
            "invalid_archive",
            format!("Cannot read ZIP tail for EOCD: {error}"),
        )
    })?;

    // Scan from the end for EOCD signature; require comment length to land at EOF.
    let mut i = search_len.saturating_sub(EOCD_MIN_SIZE as usize);
    loop {
        if tail[i] == 0x50
            && tail[i + 1] == 0x4b
            && tail[i + 2] == 0x05
            && tail[i + 3] == 0x06
        {
            let comment_len = read_u16_le(&tail[i + 20..i + 22]) as usize;
            let record_end = i + EOCD_MIN_SIZE as usize + comment_len;
            if record_end == search_len {
                let start_pos = search_start + i as u64;
                return Ok(Eocd {
                    disk_number: read_u16_le(&tail[i + 4..i + 6]),
                    disk_with_central_directory: read_u16_le(&tail[i + 6..i + 8]),
                    number_of_files_on_this_disk: read_u16_le(&tail[i + 8..i + 10]),
                    number_of_files: read_u16_le(&tail[i + 10..i + 12]),
                    central_directory_size: read_u32_le(&tail[i + 12..i + 16]),
                    central_directory_offset: read_u32_le(&tail[i + 16..i + 20]),
                    comment: tail[i + 22..record_end].to_vec(),
                    start_pos,
                });
            }
        }
        if i == 0 {
            break;
        }
        i -= 1;
    }

    Err(cd_error(
        "invalid_archive",
        "Could not find ZIP end-of-central-directory record.",
    ))
}

/// Directory counts and absolute start of the central directory in the file.
struct DirectoryInfo {
    directory_start: u64,
    number_of_files: u64,
    /// Relative CD offset as stored in EOCD/Zip64 (without archive_offset).
    central_directory_offset: u64,
    /// Zip64 version fields to preserve when rewriting Zip64 EOCD.
    zip64_versions: Option<(u16, u16)>,
}

fn directory_info(file: &mut File, eocd: &Eocd) -> Result<DirectoryInfo, CommandError> {
    if !eocd.record_too_small() && eocd.disk_number != eocd.disk_with_central_directory {
        return Err(cd_error(
            "append_unsupported",
            "Multi-disk ZIP archives are not supported for logical delete.",
        ));
    }

    // Zip64 locator sits immediately before the standard EOCD when present.
    let locator_pos = eocd.start_pos.checked_sub(ZIP64_EOCD_LOCATOR_SIZE);

    let zip64_locator = if let Some(pos) = locator_pos {
        let mut loc_buf = [0_u8; ZIP64_EOCD_LOCATOR_SIZE as usize];
        match read_exact_at(file, pos, &mut loc_buf) {
            Ok(()) if read_u32_le(&loc_buf[0..4]) == ZIP64_EOCD_LOCATOR_SIGNATURE => {
                let disk_with_cd = read_u32_le(&loc_buf[4..8]);
                let zip64_eocd_offset = read_u64_le(&loc_buf[8..16]);
                let number_of_disks = read_u32_le(&loc_buf[16..20]);
                if number_of_disks > 1 || disk_with_cd != 0 {
                    return Err(cd_error(
                        "append_unsupported",
                        "Multi-disk ZIP archives are not supported for logical delete.",
                    ));
                }
                Some(zip64_eocd_offset)
            }
            _ => None,
        }
    } else {
        None
    };

    match zip64_locator {
        None => {
            let cd_size = eocd.central_directory_size as u64;
            let cd_offset = eocd.central_directory_offset as u64;
            let archive_offset = eocd
                .start_pos
                .checked_sub(cd_size)
                .and_then(|x| x.checked_sub(cd_offset))
                .ok_or_else(|| {
                    cd_error(
                        "invalid_archive",
                        "Invalid central directory size or offset.",
                    )
                })?;
            Ok(DirectoryInfo {
                directory_start: cd_offset + archive_offset,
                number_of_files: eocd.number_of_files_on_this_disk as u64,
                central_directory_offset: cd_offset,
                zip64_versions: None,
            })
        }
        Some(nominal_zip64_offset) => {
            // Search forward from nominal offset for Zip64 EOCD (handles prepended junk).
            let search_upper = eocd.start_pos.saturating_sub(ZIP64_EOCD_LOCATOR_SIZE);
            let mut pos = nominal_zip64_offset;
            let mut found: Option<(Zip64Eocd, u64)> = None;
            while pos <= search_upper.saturating_sub(ZIP64_EOCD_FIXED_SIZE) {
                let mut sig = [0_u8; 4];
                if read_exact_at(file, pos, &mut sig).is_err() {
                    break;
                }
                if read_u32_le(&sig) == ZIP64_EOCD_SIGNATURE {
                    let mut rest = [0_u8; (ZIP64_EOCD_FIXED_SIZE - 4) as usize];
                    read_exact_at(file, pos + 4, &mut rest).map_err(|error| {
                        cd_error(
                            "invalid_archive",
                            format!("Cannot read Zip64 EOCD: {error}"),
                        )
                    })?;
                    // rest: size(8) + ver_made(2) + ver_need(2) + disk(4) + disk_cd(4)
                    // + n_this(8) + n_total(8) + cd_size(8) + cd_offset(8)
                    let z = Zip64Eocd {
                        version_made_by: read_u16_le(&rest[8..10]),
                        version_needed_to_extract: read_u16_le(&rest[10..12]),
                        disk_number: read_u32_le(&rest[12..16]),
                        disk_with_central_directory: read_u32_le(&rest[16..20]),
                        number_of_files_on_this_disk: read_u64_le(&rest[20..28]),
                        number_of_files: read_u64_le(&rest[28..36]),
                        central_directory_size: read_u64_le(&rest[36..44]),
                        central_directory_offset: read_u64_le(&rest[44..52]),
                    };
                    let archive_offset = pos.saturating_sub(nominal_zip64_offset);
                    found = Some((z, archive_offset));
                    break;
                }
                pos += 1;
            }
            let (z, archive_offset) = found.ok_or_else(|| {
                cd_error(
                    "invalid_archive",
                    "Could not find Zip64 end-of-central-directory record.",
                )
            })?;
            if z.disk_number != z.disk_with_central_directory {
                return Err(cd_error(
                    "append_unsupported",
                    "Multi-disk ZIP archives are not supported for logical delete.",
                ));
            }
            let directory_start = z
                .central_directory_offset
                .checked_add(archive_offset)
                .ok_or_else(|| {
                    cd_error(
                        "invalid_archive",
                        "Invalid Zip64 central directory offset.",
                    )
                })?;
            Ok(DirectoryInfo {
                directory_start,
                number_of_files: z.number_of_files,
                central_directory_offset: z.central_directory_offset,
                zip64_versions: Some((z.version_made_by, z.version_needed_to_extract)),
            })
        }
    }
}

/// True when normalized `entry_path` is exactly `selected` or under `selected/`.
fn selection_matches(entry_path: &str, selected: &str) -> bool {
    entry_path == selected || entry_path.starts_with(&(selected.to_owned() + "/"))
}

/// Decode a central-directory file name for path matching.
fn decode_cd_file_name(raw: &[u8], general_purpose_flag: u16) -> String {
    let is_utf8 = general_purpose_flag & (1 << 11) != 0;
    let name = if is_utf8 {
        String::from_utf8_lossy(raw).into_owned()
    } else {
        // Prefer UTF-8 when valid (common for modern writers); otherwise lossy.
        match std::str::from_utf8(raw) {
            Ok(s) => s.to_string(),
            Err(_) => String::from_utf8_lossy(raw).into_owned(),
        }
    };
    normalize_entry_name(&name)
}

/// Walk central directory records; return raw bytes of records that should be kept.
fn filter_central_directory(
    file: &mut File,
    directory_start: u64,
    number_of_files: u64,
    selected: &[String],
    cancelled: &AtomicBool,
) -> Result<(Vec<u8>, u64, u64), CommandError> {
    file.seek(SeekFrom::Start(directory_start)).map_err(|error| {
        cd_error(
            "invalid_archive",
            format!("Cannot seek to central directory: {error}"),
        )
    })?;

    let mut kept = Vec::new();
    let mut kept_count = 0_u64;
    let mut deleted_count = 0_u64;

    for index in 0..number_of_files {
        if index % 64 == 0 && cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        let mut fixed = [0_u8; CD_HEADER_FIXED_SIZE];
        file.read_exact(&mut fixed).map_err(|error| {
            cd_error(
                "invalid_archive",
                format!("Cannot read central directory header: {error}"),
            )
        })?;
        if read_u32_le(&fixed[0..4]) != CENTRAL_DIRECTORY_HEADER_SIGNATURE {
            return Err(cd_error(
                "invalid_archive",
                "Invalid central directory file header signature.",
            ));
        }

        let general_purpose_flag = read_u16_le(&fixed[8..10]);
        let file_name_len = read_u16_le(&fixed[28..30]) as usize;
        let extra_len = read_u16_le(&fixed[30..32]) as usize;
        let comment_len = read_u16_le(&fixed[32..34]) as usize;
        let variable_len = file_name_len + extra_len + comment_len;

        let mut variable = vec![0_u8; variable_len];
        file.read_exact(&mut variable).map_err(|error| {
            cd_error(
                "invalid_archive",
                format!("Cannot read central directory entry payload: {error}"),
            )
        })?;

        let name_raw = &variable[..file_name_len];
        let path = decode_cd_file_name(name_raw, general_purpose_flag);
        let delete = selected.iter().any(|sel| selection_matches(&path, sel));

        if delete {
            deleted_count += 1;
        } else {
            kept.extend_from_slice(&fixed);
            kept.extend_from_slice(&variable);
            kept_count += 1;
        }
    }

    if deleted_count == 0 {
        return Err(cd_error(
            "not_found",
            "Delete selection matched no archive entries.",
        ));
    }

    Ok((kept, kept_count, deleted_count))
}

fn needs_zip64(entry_count: u64, cd_size: u64, cd_offset: u64) -> bool {
    entry_count > u16::MAX as u64 || cd_size > u32::MAX as u64 || cd_offset > u32::MAX as u64
}

fn eocd_u16_count(count: u64) -> u16 {
    if count > u16::MAX as u64 {
        0xFFFF
    } else {
        count as u16
    }
}

fn eocd_u32_field(value: u64) -> u32 {
    if value > u32::MAX as u64 {
        0xFFFF_FFFF
    } else {
        value as u32
    }
}

/// Rewrite central directory + EOCD on an open read/write temp ZIP copy.
///
/// Local file data for removed entries is left in place (orphaned). Preserves the ZIP comment.
/// Multi-disk archives are rejected. Caller must validate with `ZipArchive::new` before publish.
///
/// Returns the number of central-directory entries kept.
pub fn logical_delete_on_file(
    file: &mut File,
    selected_normalized: &[String],
    cancelled: &AtomicBool,
) -> Result<u64, CommandError> {
    if selected_normalized.is_empty() {
        return Err(cd_error(
            "invalid_selection",
            "No archive paths specified for delete.",
        ));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let eocd = find_eocd(file)?;
    let info = directory_info(file, &eocd)?;

    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let (kept_cd, kept_count, _deleted_count) = filter_central_directory(
        file,
        info.directory_start,
        info.number_of_files,
        selected_normalized,
        cancelled,
    )?;

    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let cd_size = kept_cd.len() as u64;
    let cd_offset_relative = info.central_directory_offset;
    let cd_write_pos = info.directory_start;
    let comment = eocd.comment;

    let use_zip64 = needs_zip64(kept_count, cd_size, cd_offset_relative);
    let (version_made_by, version_needed) = info.zip64_versions.unwrap_or((45, 45));

    let mut trailer = Vec::new();
    if use_zip64 {
        let zip64 = Zip64Eocd {
            version_made_by,
            version_needed_to_extract: version_needed.max(45),
            disk_number: 0,
            disk_with_central_directory: 0,
            number_of_files_on_this_disk: kept_count,
            number_of_files: kept_count,
            central_directory_size: cd_size,
            central_directory_offset: cd_offset_relative,
        };
        let zip64_eocd_offset = cd_offset_relative.checked_add(cd_size).ok_or_else(|| {
            cd_error("write_failed", "ZIP offset overflow writing Zip64 EOCD.")
        })?;
        trailer.extend_from_slice(&zip64.encode());
        trailer.extend_from_slice(&encode_zip64_locator(zip64_eocd_offset));
    }

    let eocd_final = Eocd {
        disk_number: 0,
        disk_with_central_directory: 0,
        number_of_files_on_this_disk: eocd_u16_count(kept_count),
        number_of_files: eocd_u16_count(kept_count),
        central_directory_size: eocd_u32_field(cd_size),
        central_directory_offset: eocd_u32_field(cd_offset_relative),
        comment,
        start_pos: 0,
    };
    trailer.extend_from_slice(&eocd_final.encode());

    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    file.seek(SeekFrom::Start(cd_write_pos)).map_err(|error| {
        cd_error(
            "write_failed",
            format!("Cannot seek to rewrite central directory: {error}"),
        )
    })?;
    file.write_all(&kept_cd).map_err(|error| {
        cd_error(
            "write_failed",
            format!("Cannot write central directory: {error}"),
        )
    })?;
    file.write_all(&trailer).map_err(|error| {
        cd_error(
            "write_failed",
            format!("Cannot write ZIP end records: {error}"),
        )
    })?;

    let new_len = cd_write_pos + kept_cd.len() as u64 + trailer.len() as u64;
    file.set_len(new_len).map_err(|error| {
        cd_error(
            "write_failed",
            format!("Cannot truncate ZIP after logical delete: {error}"),
        )
    })?;
    file.sync_all().map_err(|error| {
        cd_error(
            "write_failed",
            format!("Cannot sync ZIP after logical delete: {error}"),
        )
    })?;

    Ok(kept_count)
}
