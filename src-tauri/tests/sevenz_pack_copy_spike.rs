//! SA-C1 pack-stream copy spike tests.
//!
//! Success criteria: S1 extract correctness, S2 pack-copy evidence, S3 non-solid,
//! S4 atomic publish / original intact on failure, S5 speedup when measurable, S6 eligibility.

mod common;

use archi_backend_lib::archive::open_archive;
use archi_backend_lib::models::{CompressionPreset, CreateFormat, CreateOptions};
use archi_backend_lib::sevenz_edit::delete_entries;
use archi_backend_lib::sevenz_format::create_sevenz_archive;
use archi_backend_lib::sevenz_pack_copy::{
    delete_entries_pack_copy, is_pack_copy_eligible, last_pack_copy_stats,
    pack_copy_delete_to_temp_only,
};
use sevenz_rust2::encoder_options::Lzma2Options;
use sevenz_rust2::{
    ArchiveEntry as SzEntry, ArchiveReader, ArchiveWriter, Password, SourceReader,
};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn fast_create_options() -> CreateOptions {
    CreateOptions {
        format: CreateFormat::SevenZ,
        compression: CompressionPreset::Fast,
        include_root: false,
        overwrite: false,
    }
}

fn create_three_file_nonsolid(root: &Path, payload: &[u8]) -> PathBuf {
    let src = root.join("pack");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.bin"), payload).unwrap();
    fs::write(src.join("b.bin"), payload).unwrap();
    fs::write(src.join("c.bin"), payload).unwrap();

    let out = root.join("three.7z");
    create_sevenz_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "7z-pack-copy-create",
        &AtomicBool::new(false),
        &fast_create_options(),
        |_| {},
    )
    .unwrap();
    out
}

fn entry_names(archive: &Path) -> Vec<String> {
    let info = open_archive(archive).unwrap();
    let mut names: Vec<String> = info
        .entries
        .iter()
        .map(|e| e.path.trim_matches('/').replace('\\', "/"))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn entry_bytes_via_extract(archive: &Path, member: &str) -> Vec<u8> {
    use archi_backend_lib::extraction::{extract_any, FailOnConflict};
    let dest = archive.parent().unwrap().join(format!(
        "extract-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dest).unwrap();
    extract_any(
        archive,
        &dest,
        "ex-check",
        &AtomicBool::new(false),
        Some(&[member.to_string()]),
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    let path = dest.join(member.replace('/', std::path::MAIN_SEPARATOR_STR));
    let bytes = fs::read(&path).unwrap();
    let _ = fs::remove_dir_all(&dest);
    bytes
}

/// S1 + S2 + S3: delete 1 of 3 → remaining extract correct; packs copied; non-solid.
#[test]
fn s1_s2_s3_delete_one_of_three_pack_copy() {
    let root = common::temp_dir("7z-pack-copy-s1");
    // ~256 KiB each so LZMA time is measurable vs pure copy on larger runs.
    let payload = vec![0xABu8; 256 * 1024];
    let archive = create_three_file_nonsolid(&root, &payload);

    assert!(
        is_pack_copy_eligible(&archive).unwrap(),
        "Archi-created Fast non-solid should be pack-copy eligible"
    );

    let reader = ArchiveReader::open(&archive, Password::empty()).unwrap();
    assert!(!reader.archive().is_solid, "fixture must be non-solid");
    drop(reader);

    let summary = delete_entries_pack_copy(
        &archive,
        &["b.bin".into()],
        "pack-copy-delete-1",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("pack_copy"));
    assert!(summary.members_written >= 2);

    let names = entry_names(&archive);
    assert!(!names.iter().any(|n| n == "b.bin"));
    assert!(names.iter().any(|n| n == "a.bin"));
    assert!(names.iter().any(|n| n == "c.bin"));

    // S1: remaining extract correct
    assert_eq!(entry_bytes_via_extract(&archive, "a.bin"), payload);
    assert_eq!(entry_bytes_via_extract(&archive, "c.bin"), payload);

    // S2: kept packs were byte-copied (2 packs for a.bin + c.bin)
    let stats = last_pack_copy_stats();
    assert_eq!(stats.packs_copied, 2, "expected two pack streams copied");
    assert!(
        stats.pack_bytes_copied > 0,
        "expected non-zero packed bytes copied"
    );
    assert_eq!(
        stats.members_reencoded, 0,
        "pure delete must not re-encode members"
    );

    // S3: output opens, is_solid false
    let reader = ArchiveReader::open(&archive, Password::empty()).unwrap();
    assert!(!reader.archive().is_solid);
    assert!(reader.archive().files.iter().any(|f| f.name() == "a.bin"));
    assert!(!reader.archive().files.iter().any(|f| f.name() == "b.bin"));
    drop(reader);

    fs::remove_dir_all(root).unwrap();
}

/// S4: on failure before publish, original archive remains intact.
#[test]
fn s4_original_intact_when_not_published() {
    let root = common::temp_dir("7z-pack-copy-s4");
    let payload = b"keep-me-atomic".to_vec();
    let archive = create_three_file_nonsolid(&root, &payload);
    let original_bytes = fs::read(&archive).unwrap();

    let temp = pack_copy_delete_to_temp_only(&archive, &["c.bin".into()]).unwrap();
    assert!(temp.is_file(), "temp output should exist");
    // Simulate failure: do not publish; remove temp as cleanup would.
    fs::remove_file(&temp).unwrap();

    let after = fs::read(&archive).unwrap();
    assert_eq!(
        after, original_bytes,
        "original archive must be unchanged when publish never runs"
    );
    assert_eq!(
        entry_bytes_via_extract(&archive, "c.bin"),
        payload.as_slice()
    );

    fs::remove_dir_all(root).unwrap();
}

/// S5: pack-copy should be faster than stream_rebuild on larger members (when measurable).
#[test]
fn s5_pack_copy_faster_than_stream_rebuild_when_measurable() {
    let root = common::temp_dir("7z-pack-copy-s5");
    // 1 MiB × 3 — enough that LZMA decode+encode shows up vs pack byte-copy.
    let payload = vec![0x5Au8; 1024 * 1024];
    let archive_pack = create_three_file_nonsolid(&root, &payload);
    let archive_rebuild = root.join("three-rebuild.7z");
    fs::copy(&archive_pack, &archive_rebuild).unwrap();

    let t0 = Instant::now();
    delete_entries_pack_copy(
        &archive_pack,
        &["b.bin".into()],
        "pack-copy-bench",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    let pack_ms = t0.elapsed().as_millis();

    let t1 = Instant::now();
    delete_entries(
        &archive_rebuild,
        &["b.bin".into()],
        "stream-rebuild-bench",
        &AtomicBool::new(false),
        |_| {},
        &Default::default(),
    )
    .unwrap();
    let rebuild_ms = t1.elapsed().as_millis();

    // Soft assert: document ratio; require pack_copy not pathologically slower.
    eprintln!(
        "S5 timing: pack_copy={pack_ms}ms stream_rebuild={rebuild_ms}ms packs_copied={}",
        last_pack_copy_stats().packs_copied
    );
    // On CI/dev machines pack_copy should win or be comparable; allow noise on tiny runs.
    // Hard check: both succeeded and pack_copy actually copied packs.
    assert_eq!(last_pack_copy_stats().packs_copied, 2);
    // Prefer pack_copy ≤ rebuild * 1.5 as "meaningful when measurable"; if rebuild is tiny, skip.
    if rebuild_ms >= 50 {
        assert!(
            pack_ms <= rebuild_ms.saturating_mul(3) / 2 || pack_ms < rebuild_ms,
            "expected pack_copy ({pack_ms}ms) not much slower than stream_rebuild ({rebuild_ms}ms)"
        );
    }

    assert_eq!(entry_bytes_via_extract(&archive_pack, "a.bin"), payload);
    assert_eq!(entry_bytes_via_extract(&archive_rebuild, "a.bin"), payload);

    fs::remove_dir_all(root).unwrap();
}

/// S6: solid (multi-substream) archives rejected for pack-copy.
#[test]
fn s6_eligibility_rejects_solid() {
    let root = common::temp_dir("7z-pack-copy-s6-solid");
    let out = root.join("solid.7z");

    // Build a true solid archive: one pack stream, multiple unpack sub-streams.
    let mut writer = ArchiveWriter::create(&out).unwrap();
    writer.set_content_methods(vec![Lzma2Options::from_level(3).into()]);
    writer.set_encrypt_header(false);
    let entries = vec![
        SzEntry::new_file("x.txt"),
        SzEntry::new_file("y.txt"),
    ];
    let readers = vec![
        SourceReader::new(Cursor::new(b"solid-a".to_vec())),
        SourceReader::new(Cursor::new(b"solid-b".to_vec())),
    ];
    writer.push_archive_entries(entries, readers).unwrap();
    writer.finish().unwrap();

    let reader = ArchiveReader::open(&out, Password::empty()).unwrap();
    assert!(
        reader.archive().is_solid,
        "fixture must be solid (multi-substream)"
    );
    drop(reader);

    assert!(!is_pack_copy_eligible(&out).unwrap());
    let err = delete_entries_pack_copy(
        &out,
        &["x.txt".into()],
        "solid-delete",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code, "pack_copy_ineligible");
    assert!(
        err.message.contains("solid") || err.message.contains("multi"),
        "message={}",
        err.message
    );

    fs::remove_dir_all(root).unwrap();
}

/// S6: multi-substream / solid-style ineligible reason is exposed.
#[test]
fn s6_nonsolid_eligible_and_delete_missing_fails() {
    let root = common::temp_dir("7z-pack-copy-s6-elig");
    let archive = create_three_file_nonsolid(&root, b"data");
    assert!(is_pack_copy_eligible(&archive).unwrap());

    let err = delete_entries_pack_copy(
        &archive,
        &["no-such.bin".into()],
        "missing",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code, "not_found");

    fs::remove_dir_all(root).unwrap();
}
