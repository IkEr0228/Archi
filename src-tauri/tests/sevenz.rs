mod common;

use archi_backend_lib::archive::open_archive;
use archi_backend_lib::extraction::{extract_any, FailOnConflict};
use archi_backend_lib::format_detect::{detect_format, ArchiveFormat};
use archi_backend_lib::models::{CompressionPreset, CreateFormat, CreateOptions};
use archi_backend_lib::sevenz_format::create_sevenz_archive;
use std::fs;
use std::sync::atomic::AtomicBool;

#[test]
fn create_open_extract_sevenz_round_trip() {
    let root = common::temp_dir("7z-rt");
    let src = root.join("pack");
    fs::create_dir_all(src.join("nested")).unwrap();
    fs::write(src.join("a.txt"), b"hello-sevenz").unwrap();
    fs::write(src.join("nested").join("b.bin"), b"nested-7z").unwrap();
    let out = root.join("out.7z");
    let dest = root.join("extract");
    fs::create_dir(&dest).unwrap();

    create_sevenz_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "7z-1",
        &AtomicBool::new(false),
        &CreateOptions {
            format: CreateFormat::SevenZ,
            compression: CompressionPreset::Max,
            include_root: true,
            overwrite: false,
        },
        |_| {},
    )
    .unwrap();

    assert_eq!(detect_format(&out).unwrap(), ArchiveFormat::SevenZ);
    let info = open_archive(&out).unwrap();
    assert_eq!(info.format, "7z");
    assert!(info.capabilities.extract);
    assert!(info.capabilities.edit);
    assert!(info.capabilities.test);

    extract_any(
        &out,
        &dest,
        "ex-7z",
        &AtomicBool::new(false),
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(
        fs::read(dest.join("pack").join("a.txt")).unwrap(),
        b"hello-sevenz"
    );
    assert_eq!(
        fs::read(dest.join("pack").join("nested").join("b.bin")).unwrap(),
        b"nested-7z"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_sevenz_materializes_empty_directory() {
    let root = common::temp_dir("7z-empty-dir");
    let src = root.join("pack");
    fs::create_dir_all(src.join("empty_folder")).unwrap();
    fs::write(src.join("file.txt"), b"keep").unwrap();
    let out = root.join("out.7z");
    let dest = root.join("extract");
    fs::create_dir(&dest).unwrap();

    create_sevenz_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "7z-empty",
        &AtomicBool::new(false),
        &CreateOptions {
            format: CreateFormat::SevenZ,
            compression: CompressionPreset::Fast,
            include_root: true,
            overwrite: false,
        },
        |_| {},
    )
    .unwrap();

    extract_any(
        &out,
        &dest,
        "ex-7z-empty",
        &AtomicBool::new(false),
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert!(dest.join("pack").join("empty_folder").is_dir());
    assert_eq!(
        fs::read(dest.join("pack").join("file.txt")).unwrap(),
        b"keep"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_sevenz_selected_file_only() {
    let root = common::temp_dir("7z-sel-file");
    let src = root.join("pack");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("keep.txt"), b"keep").unwrap();
    fs::write(src.join("skip.txt"), b"skip").unwrap();
    let out = root.join("out.7z");
    let dest = root.join("extract");
    fs::create_dir(&dest).unwrap();

    create_sevenz_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "7z-sel-f",
        &AtomicBool::new(false),
        &CreateOptions {
            format: CreateFormat::SevenZ,
            compression: CompressionPreset::Fast,
            include_root: true,
            overwrite: false,
        },
        |_| {},
    )
    .unwrap();

    let selected = vec!["pack/keep.txt".to_string()];
    let summary = extract_any(
        &out,
        &dest,
        "ex-7z-sel-file",
        &AtomicBool::new(false),
        Some(&selected),
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(summary.extracted_files, 1);
    assert_eq!(
        fs::read(dest.join("pack").join("keep.txt")).unwrap(),
        b"keep"
    );
    assert!(!dest.join("pack").join("skip.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_sevenz_selected_directory_recursively() {
    let root = common::temp_dir("7z-sel-dir");
    let src = root.join("pack");
    fs::create_dir_all(src.join("docs").join("sub")).unwrap();
    fs::create_dir_all(src.join("other")).unwrap();
    fs::write(src.join("docs").join("a.txt"), b"a").unwrap();
    fs::write(src.join("docs").join("sub").join("b.txt"), b"b").unwrap();
    fs::write(src.join("other").join("c.txt"), b"c").unwrap();
    let out = root.join("out.7z");
    let dest = root.join("extract");
    fs::create_dir(&dest).unwrap();

    create_sevenz_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "7z-sel-d",
        &AtomicBool::new(false),
        &CreateOptions {
            format: CreateFormat::SevenZ,
            compression: CompressionPreset::Fast,
            include_root: true,
            overwrite: false,
        },
        |_| {},
    )
    .unwrap();

    let selected = vec!["pack/docs".to_string()];
    let summary = extract_any(
        &out,
        &dest,
        "ex-7z-sel-dir",
        &AtomicBool::new(false),
        Some(&selected),
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    // Directory entries may count toward extracted_files depending on 7z listing;
    // assert content inclusion and exclusion instead of a brittle total.
    assert!(summary.extracted_files >= 2);
    assert_eq!(
        fs::read(dest.join("pack").join("docs").join("a.txt")).unwrap(),
        b"a"
    );
    assert_eq!(
        fs::read(dest.join("pack").join("docs").join("sub").join("b.txt")).unwrap(),
        b"b"
    );
    assert!(!dest.join("pack").join("other").join("c.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sevenz_max_shrinks_text() {
    let root = common::temp_dir("7z-max");
    let src = root.join("data");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("big.txt"), "SEVENZ".repeat(200_000).as_bytes()).unwrap();
    let raw = fs::metadata(src.join("big.txt")).unwrap().len();
    let out = root.join("max.7z");
    create_sevenz_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "7z-max",
        &AtomicBool::new(false),
        &CreateOptions {
            format: CreateFormat::SevenZ,
            compression: CompressionPreset::Max,
            include_root: true,
            overwrite: true,
        },
        |_| {},
    )
    .unwrap();
    let packed = fs::metadata(&out).unwrap().len();
    assert!(
        packed < raw / 10,
        "expected strong LZMA2 shrink: packed {packed} vs raw {raw}"
    );
    let info = open_archive(&out).unwrap();
    assert_eq!(info.stats.total_compressed, packed);
    assert!(info.stats.total_uncompressed >= raw);
    assert!(info.stats.total_compressed < info.stats.total_uncompressed);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn sevenz_rejects_output_inside_source() {
    let root = common::temp_dir("7z-contain");
    let src = root.join("pack");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.txt"), b"x").unwrap();
    let out = src.join("evil.7z");
    let err = create_sevenz_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "bad",
        &AtomicBool::new(false),
        &CreateOptions {
            format: CreateFormat::SevenZ,
            compression: CompressionPreset::Fast,
            include_root: true,
            overwrite: false,
        },
        |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code, "output_inside_source");
    fs::remove_dir_all(root).unwrap();
}
