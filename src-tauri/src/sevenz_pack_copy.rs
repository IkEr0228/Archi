//! SA-C1 research spike: byte-copy unchanged non-solid 7z pack streams.
//!
//! **Not wired** into `sevenz_edit` product paths. Call only from tests / explicit
//! spike entry points. Solid and multi-substream archives are always rejected.

use crate::create_common::{
    cleanup_temp, create_temporary_archive, member_path_for_tar, progress_percentage,
    publish_temp_archive, ProgressGate,
};
use crate::extraction::normalize_entry_name;
use crate::models::{CommandError, EditSummary, OperationProgress};
use crate::security::validate_entry_path;
use sevenz_rust2::{
    Archive, ArchiveEntry as SzEntry, ArchiveReader, ArchiveWriter, EncoderMethod, Password,
    SIGNATURE_HEADER_SIZE,
};
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Counters returned by a successful pack-copy (for tests / S2 evidence).
#[derive(Debug, Clone, Default)]
pub struct PackCopyStats {
    /// Number of pack streams byte-copied without re-encoding.
    pub packs_copied: u64,
    /// Total compressed bytes copied from source packs.
    pub pack_bytes_copied: u64,
    /// Number of members re-encoded (new/dirty files). Zero for pure delete of one file.
    pub members_reencoded: u64,
    /// Number of empty directories written (no pack stream).
    pub directories_written: u64,
}

static LAST_PACK_COPY_STATS: AtomicU64 = AtomicU64::new(0);
static LAST_PACK_BYTES: AtomicU64 = AtomicU64::new(0);
static LAST_REENCODED: AtomicU64 = AtomicU64::new(0);

/// Last successful pack-copy stats (process-local; for integration tests).
pub fn last_pack_copy_stats() -> PackCopyStats {
    PackCopyStats {
        packs_copied: LAST_PACK_COPY_STATS.load(Ordering::SeqCst),
        pack_bytes_copied: LAST_PACK_BYTES.load(Ordering::SeqCst),
        members_reencoded: LAST_REENCODED.load(Ordering::SeqCst),
        directories_written: 0,
    }
}

fn record_stats(stats: &PackCopyStats) {
    LAST_PACK_COPY_STATS.store(stats.packs_copied, Ordering::SeqCst);
    LAST_PACK_BYTES.store(stats.pack_bytes_copied, Ordering::SeqCst);
    LAST_REENCODED.store(stats.members_reencoded, Ordering::SeqCst);
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

fn selection_matches(entry_path: &str, selected: &str) -> bool {
    entry_path == selected || entry_path.starts_with(&(selected.to_owned() + "/"))
}

/// Why pack-copy is not eligible for this archive / plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackCopyIneligible {
    Solid,
    MultiSubstream,
    MultiPackStreams,
    MultiCoder,
    UnsupportedCoder,
    EncryptedOrPassword,
    ComplexBind,
    EmptySelection,
    NothingKept,
    Other(String),
}

impl PackCopyIneligible {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Solid => "solid",
            Self::MultiSubstream => "multi_substream",
            Self::MultiPackStreams => "multi_pack_streams",
            Self::MultiCoder => "multi_coder",
            Self::UnsupportedCoder => "unsupported_coder",
            Self::EncryptedOrPassword => "encrypted",
            Self::ComplexBind => "complex_bind",
            Self::EmptySelection => "empty_selection",
            Self::NothingKept => "nothing_kept",
            Self::Other(s) => s.as_str(),
        }
    }
}

/// Per-file pack location for a 1:1 file↔block non-solid archive.
#[derive(Debug, Clone)]
struct PackSlot {
    file_index: usize,
    /// Absolute file offset of the pack stream.
    absolute_offset: u64,
    pack_size: u64,
    /// Ordered coders: (method, property bytes).
    coders: Vec<(EncoderMethod, Vec<u8>)>,
    /// Folder unpack sizes (per coder).
    unpack_sizes: Vec<u64>,
    entry: SzEntry,
}

fn method_from_coder(coder: &sevenz_rust2::Coder) -> Result<EncoderMethod, PackCopyIneligible> {
    EncoderMethod::by_id(coder.encoder_method_id()).ok_or(PackCopyIneligible::UnsupportedCoder)
}

/// Inspect archive structure for pack-copy eligibility (S6).
pub fn assess_pack_copy_eligibility(archive: &Archive) -> Result<(), PackCopyIneligible> {
    if archive.is_solid {
        return Err(PackCopyIneligible::Solid);
    }
    for block in &archive.blocks {
        if block.num_unpack_sub_streams() != 1 {
            return Err(PackCopyIneligible::MultiSubstream);
        }
        if block.packed_streams_count() != 1 {
            return Err(PackCopyIneligible::MultiPackStreams);
        }
        if block.coders.len() != 1 {
            // Spike scope: single-coder folders only (Archi LZMA2 1:1).
            return Err(PackCopyIneligible::MultiCoder);
        }
        let coder = &block.coders[0];
        if coder.num_in_streams() != 1 || coder.num_out_streams() != 1 {
            return Err(PackCopyIneligible::ComplexBind);
        }
        let method = method_from_coder(coder)?;
        // Refuse encryption in pack-copy path.
        if method.id() == EncoderMethod::ID_AES256_SHA256 {
            return Err(PackCopyIneligible::EncryptedOrPassword);
        }
        // Spike: only LZMA2 / COPY (Store) single-coder folders.
        if method.id() != EncoderMethod::ID_LZMA2 && method.id() != EncoderMethod::ID_COPY {
            return Err(PackCopyIneligible::UnsupportedCoder);
        }
    }
    Ok(())
}

fn build_pack_slots(archive: &Archive) -> Result<Vec<PackSlot>, CommandError> {
    assess_pack_copy_eligibility(archive).map_err(|e| {
        edit_error(
            "pack_copy_ineligible",
            format!("Pack-copy not eligible: {}", e.as_str()),
        )
    })?;

    let pack_base = SIGNATURE_HEADER_SIZE
        .checked_add(archive.pack_pos())
        .ok_or_else(|| edit_error("invalid_archive", "pack_pos overflow"))?;
    let pack_sizes = archive.pack_sizes();
    let stream_map = &archive.stream_map;
    let pack_offsets = stream_map.pack_stream_offsets();
    let block_first_pack = stream_map.block_first_pack_stream_index();

    let mut slots = Vec::new();
    for (file_index, file) in archive.files.iter().enumerate() {
        if file.is_anti_item {
            continue;
        }
        if !file.has_stream {
            // Directory / empty — no pack slot.
            continue;
        }
        let block_index = stream_map.file_block_index[file_index].ok_or_else(|| {
            edit_error(
                "invalid_archive",
                format!("Streamed file {file_index} has no block mapping"),
            )
        })?;
        let block = archive.blocks.get(block_index).ok_or_else(|| {
            edit_error("invalid_archive", format!("Missing block {block_index}"))
        })?;
        let pack_stream_index = *block_first_pack.get(block_index).ok_or_else(|| {
            edit_error(
                "invalid_archive",
                format!("Missing pack stream index for block {block_index}"),
            )
        })?;
        let pack_size = *pack_sizes.get(pack_stream_index).ok_or_else(|| {
            edit_error(
                "invalid_archive",
                format!("Missing pack size for stream {pack_stream_index}"),
            )
        })?;
        let relative = *pack_offsets.get(pack_stream_index).ok_or_else(|| {
            edit_error(
                "invalid_archive",
                format!("Missing pack offset for stream {pack_stream_index}"),
            )
        })?;
        let absolute_offset = pack_base
            .checked_add(relative)
            .ok_or_else(|| edit_error("invalid_archive", "pack absolute offset overflow"))?;

        let mut coders = Vec::with_capacity(block.coders.len());
        for coder in &block.coders {
            let method = method_from_coder(coder).map_err(|e| {
                edit_error(
                    "pack_copy_ineligible",
                    format!("Pack-copy not eligible: {}", e.as_str()),
                )
            })?;
            coders.push((method, coder.properties().to_vec()));
        }
        let unpack_sizes = block.unpack_sizes().to_vec();
        if unpack_sizes.len() != coders.len() {
            return Err(edit_error(
                "invalid_archive",
                "Coder count does not match unpack_sizes length",
            ));
        }

        let _ = (block_index, pack_stream_index); // validated above via offsets
        slots.push(PackSlot {
            file_index,
            absolute_offset,
            pack_size,
            coders,
            unpack_sizes,
            entry: file.clone(),
        });
    }
    Ok(slots)
}

/// Spike entry: delete selected paths using pack-stream copy for kept non-solid members.
///
/// Does **not** re-LZMA kept packs. Only re-encodes nothing for pure deletes of whole
/// 1:1 blocks. New/dirty content is not supported on this spike path (use stream_rebuild).
pub fn delete_entries_pack_copy(
    archive_path: &Path,
    paths: &[String],
    operation_id: &str,
    cancelled: &AtomicBool,
    mut emit: impl FnMut(OperationProgress),
) -> Result<EditSummary, CommandError> {
    if operation_id.is_empty() {
        return Err(edit_error("invalid_operation", "Operation ID is empty."));
    }
    if paths.is_empty() {
        return Err(edit_error(
            "invalid_selection",
            "No archive paths specified for delete.",
        ));
    }
    if !archive_path.is_file() {
        return Err(edit_error(
            "not_found",
            format!("Archive not found: {}", archive_path.display()),
        ));
    }
    if cancelled.load(Ordering::Relaxed) {
        return Err(cancelled_error());
    }

    let mut selected = Vec::with_capacity(paths.len());
    for raw in paths {
        selected.push(normalize_and_validate(raw)?);
    }

    let reader = ArchiveReader::open(archive_path, Password::empty()).map_err(map_sz_error)?;
    let archive = reader.archive().clone();
    drop(reader);

    assess_pack_copy_eligibility(&archive).map_err(|e| {
        edit_error(
            "pack_copy_ineligible",
            format!("Pack-copy not eligible: {}", e.as_str()),
        )
    })?;

    let slots = build_pack_slots(&archive)?;
    let slot_by_file: HashMap<usize, &PackSlot> =
        slots.iter().map(|s| (s.file_index, s)).collect();

    // Plan keep set by source file index.
    let mut keep_file_indices: Vec<usize> = Vec::new();
    let mut matched = false;
    let mut keep_dirs: Vec<SzEntry> = Vec::new();

    for (file_index, file) in archive.files.iter().enumerate() {
        if file.is_anti_item {
            continue;
        }
        let path = normalize_member_name(file.name())?;
        let delete = selected.iter().any(|sel| selection_matches(&path, sel));
        if delete {
            matched = true;
            continue;
        }
        if !file.has_stream {
            let mut entry = file.clone();
            entry.name = member_path_for_tar(&path);
            keep_dirs.push(entry);
            keep_file_indices.push(file_index);
        } else if slot_by_file.contains_key(&file_index) {
            keep_file_indices.push(file_index);
        } else {
            return Err(edit_error(
                "invalid_archive",
                format!("Kept streamed member has no pack slot: {path}"),
            ));
        }
    }

    if !matched {
        return Err(edit_error(
            "not_found",
            "Delete selection matched no archive entries.",
        ));
    }
    if keep_file_indices.is_empty() && keep_dirs.is_empty() {
        // Deleting everything is still valid: write empty archive shell? Prefer explicit.
        // Allow empty output: no members.
    }

    // Preserve original order of kept members.
    keep_file_indices.sort_unstable();
    let mut seen = HashSet::new();
    keep_file_indices.retain(|i| seen.insert(*i));

    let total = (keep_file_indices.len() + keep_dirs.len()) as u64;
    let (temp_path, temp_file) = create_temporary_archive(archive_path)?;

    let result = (|| -> Result<(EditSummary, PackCopyStats), CommandError> {
        let mut writer = ArchiveWriter::new(temp_file).map_err(map_sz_error)?;
        // Header encoding uses content methods; default LZMA is fine.
        writer.set_encrypt_header(false);

        let mut source = File::open(archive_path).map_err(|e| {
            edit_error(
                "invalid_archive",
                format!("Cannot reopen archive for pack copy: {e}"),
            )
        })?;

        let mut stats = PackCopyStats::default();
        let mut processed = 0_u64;
        let mut progress_gate = ProgressGate::new();

        // Write kept members in archive file order.
        for file_index in &keep_file_indices {
            if cancelled.load(Ordering::Relaxed) {
                return Err(cancelled_error());
            }
            let file = &archive.files[*file_index];
            let path = normalize_member_name(file.name())?;
            if progress_gate.should_emit() {
                emit(OperationProgress {
                    operation_id: operation_id.into(),
                    extracted_files: processed,
                    total_files: total,
                    current_file: path.clone(),
                    percentage: progress_percentage(processed, total.max(1)),
                    phase: Some("pack_copy".into()),
                });
            }

            if !file.has_stream {
                let member = member_path_for_tar(&path);
                writer
                    .push_archive_entry(SzEntry::new_directory(&member), None::<File>)
                    .map_err(map_sz_error)?;
                stats.directories_written = stats.directories_written.saturating_add(1);
                processed = processed.saturating_add(1);
                continue;
            }

            let slot = slot_by_file.get(file_index).ok_or_else(|| {
                edit_error("invalid_archive", "Missing pack slot for kept member")
            })?;

            source
                .seek(SeekFrom::Start(slot.absolute_offset))
                .map_err(|e| {
                    edit_error("invalid_archive", format!("Cannot seek pack stream: {e}"))
                })?;
            let mut limited = (&mut source).take(slot.pack_size);

            let mut entry = slot.entry.clone();
            entry.name = member_path_for_tar(&path);
            // Ensure size/crc from source entry are preserved.
            entry.has_stream = true;
            entry.has_crc = slot.entry.has_crc;
            entry.crc = slot.entry.crc;
            entry.size = slot.entry.size;

            let coder_refs: Vec<(EncoderMethod, &[u8])> = slot
                .coders
                .iter()
                .map(|(m, p)| (*m, p.as_slice()))
                .collect();

            writer
                .push_packed_entry(
                    entry,
                    &mut limited,
                    &coder_refs,
                    slot.unpack_sizes.clone(),
                )
                .map_err(map_sz_error)?;

            stats.packs_copied = stats.packs_copied.saturating_add(1);
            stats.pack_bytes_copied = stats.pack_bytes_copied.saturating_add(slot.pack_size);
            processed = processed.saturating_add(1);
        }

        // keep_dirs already written when they appear in keep_file_indices (no stream).
        // If we only collected keep_dirs separately without indices, write leftovers.
        // In this plan, empty dirs are in keep_file_indices too when !has_stream.
        let _ = keep_dirs;

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
        drop(source);

        if cancelled.load(Ordering::Relaxed) {
            return Err(cancelled_error());
        }

        // Snapshot original for failure integrity checks in tests (publish is atomic).
        publish_temp_archive(&temp_path, archive_path, true).map_err(|error| {
            edit_error(
                "finalize_failed",
                format!("Cannot replace archive with pack-copy result: {error}"),
            )
        })?;

        Ok((
            EditSummary {
                operation_id: operation_id.into(),
                destination: archive_path.to_string_lossy().into_owned(),
                members_written: processed,
                strategy_used: Some("pack_copy".into()),
            },
            stats,
        ))
    })();

    match result {
        Ok((summary, stats)) => {
            record_stats(&stats);
            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files: summary.members_written,
                total_files: summary.members_written,
                current_file: "Completed".into(),
                percentage: 100.0,
                phase: Some("pack_copy".into()),
            });
            Ok(summary)
        }
        Err(mut error) => {
            cleanup_temp(&temp_path, &mut error);
            Err(error)
        }
    }
}

/// True if the path is eligible for pack-copy (open + assess). Used by tests (S6).
pub fn is_pack_copy_eligible(path: &Path) -> Result<bool, CommandError> {
    let reader = ArchiveReader::open(path, Password::empty()).map_err(map_sz_error)?;
    match assess_pack_copy_eligibility(reader.archive()) {
        Ok(()) => Ok(true),
        Err(PackCopyIneligible::Solid)
        | Err(PackCopyIneligible::MultiSubstream)
        | Err(PackCopyIneligible::MultiPackStreams)
        | Err(PackCopyIneligible::MultiCoder)
        | Err(PackCopyIneligible::UnsupportedCoder)
        | Err(PackCopyIneligible::EncryptedOrPassword)
        | Err(PackCopyIneligible::ComplexBind) => Ok(false),
        Err(PackCopyIneligible::EmptySelection)
        | Err(PackCopyIneligible::NothingKept)
        | Err(PackCopyIneligible::Other(_)) => Ok(false),
    }
}

/// Failure-path helper for tests: run pack-copy write into a temp, then fail before publish
/// by returning the temp path (caller validates original intact). Spike only.
pub fn pack_copy_delete_to_temp_only(
    archive_path: &Path,
    paths: &[String],
) -> Result<std::path::PathBuf, CommandError> {
    // Minimal: open, plan, write temp, do not publish.
    if paths.is_empty() {
        return Err(edit_error("invalid_selection", "empty"));
    }
    let selected: Result<Vec<_>, _> = paths.iter().map(|p| normalize_and_validate(p)).collect();
    let selected = selected?;
    let reader = ArchiveReader::open(archive_path, Password::empty()).map_err(map_sz_error)?;
    let archive = reader.archive().clone();
    drop(reader);
    assess_pack_copy_eligibility(&archive).map_err(|e| {
        edit_error(
            "pack_copy_ineligible",
            format!("Pack-copy not eligible: {}", e.as_str()),
        )
    })?;
    let slots = build_pack_slots(&archive)?;
    let slot_by_file: HashMap<usize, &PackSlot> =
        slots.iter().map(|s| (s.file_index, s)).collect();

    let mut keep: Vec<usize> = Vec::new();
    let mut matched = false;
    for (file_index, file) in archive.files.iter().enumerate() {
        if file.is_anti_item {
            continue;
        }
        let path = normalize_member_name(file.name())?;
        if selected.iter().any(|sel| selection_matches(&path, sel)) {
            matched = true;
        } else {
            keep.push(file_index);
        }
    }
    if !matched {
        return Err(edit_error("not_found", "no match"));
    }

    let (temp_path, temp_file) = create_temporary_archive(archive_path)?;
    let mut writer = ArchiveWriter::new(temp_file).map_err(map_sz_error)?;
    writer.set_encrypt_header(false);
    let mut source = File::open(archive_path).map_err(|e| {
        edit_error("invalid_archive", format!("open: {e}"))
    })?;

    for file_index in keep {
        let file = &archive.files[file_index];
        let path = normalize_member_name(file.name())?;
        if !file.has_stream {
            writer
                .push_archive_entry(
                    SzEntry::new_directory(&member_path_for_tar(&path)),
                    None::<File>,
                )
                .map_err(map_sz_error)?;
            continue;
        }
        let slot = slot_by_file
            .get(&file_index)
            .ok_or_else(|| edit_error("invalid_archive", "no slot"))?;
        source
            .seek(SeekFrom::Start(slot.absolute_offset))
            .map_err(|e| edit_error("invalid_archive", format!("seek: {e}")))?;
        let mut limited = (&mut source).take(slot.pack_size);
        let mut entry = slot.entry.clone();
        entry.name = member_path_for_tar(&path);
        let coder_refs: Vec<(EncoderMethod, &[u8])> = slot
            .coders
            .iter()
            .map(|(m, p)| (*m, p.as_slice()))
            .collect();
        writer
            .push_packed_entry(entry, &mut limited, &coder_refs, slot.unpack_sizes.clone())
            .map_err(map_sz_error)?;
    }
    let finished = writer
        .finish()
        .map_err(|e| edit_error("write_failed", e.to_string()))?;
    finished
        .sync_all()
        .map_err(|e| edit_error("write_failed", e.to_string()))?;
    drop(finished);
    Ok(temp_path)
}
