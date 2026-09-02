mod common;

use archi_backend_lib::archive::open_archive;
use archi_backend_lib::extraction::{extract_any, FailOnConflict};
use archi_backend_lib::format_detect::{detect_format, ArchiveFormat};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

#[test]
fn test_detect_format_rar() {
    let version_rar = fixture_path("version.rar");
    assert_eq!(detect_format(&version_rar).unwrap(), ArchiveFormat::Rar);

    let crypted_rar = fixture_path("crypted.rar");
    assert_eq!(detect_format(&crypted_rar).unwrap(), ArchiveFormat::Rar);

    let solid_rar = fixture_path("solid.rar");
    assert_eq!(detect_format(&solid_rar).unwrap(), ArchiveFormat::Rar);
}

#[test]
fn test_open_rar_listing() {
    let path = fixture_path("version.rar");
    let info = open_archive(&path, None).expect("failed to open version.rar");

    assert_eq!(info.format, "rar");
    assert!(info.capabilities.extract);
    assert!(!info.capabilities.edit);
    assert!(!info.capabilities.create);
    assert_eq!(info.entries.len(), 1);
    assert_eq!(info.entries[0].name, "VERSION");
    assert_eq!(info.entries[0].path, "VERSION");
    assert_eq!(info.entries[0].parent_path, "/");
    assert!(!info.entries[0].is_directory);
    assert_eq!(info.entries[0].uncompressed_size, 11);
    assert_eq!(info.entries[0].method.as_deref(), Some("RAR"));
}

#[test]
fn test_open_rar_encrypted_listing() {
    let path = fixture_path("crypted.rar");
    let info = open_archive(&path, None).expect("open crypted.rar without password for listing");

    assert_eq!(info.format, "rar");
    assert_eq!(info.entries.len(), 1);
    assert_eq!(info.entries[0].name, ".gitignore");
}

#[test]
fn test_extract_rar_full() {
    let archive_path = fixture_path("version.rar");
    let root = common::temp_dir("rar-extract");
    let dest = root.join("extracted");
    fs::create_dir(&dest).unwrap();

    let summary = extract_any(
        &archive_path,
        &dest,
        "op-rar-1",
        &AtomicBool::new(false),
        None,
        None,
        &FailOnConflict,
        |_| {},
    )
    .expect("failed to extract version.rar");

    assert_eq!(summary.extracted_files, 1);
    let extracted_file = dest.join("VERSION");
    assert!(extracted_file.exists());
    let content = fs::read_to_string(&extracted_file).unwrap();
    assert_eq!(content, "unrar-0.4.0");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_extract_rar_with_password() {
    let archive_path = fixture_path("crypted.rar");
    let root = common::temp_dir("rar-pw-extract");
    let dest = root.join("extracted");
    fs::create_dir(&dest).unwrap();

    let summary = extract_any(
        &archive_path,
        &dest,
        "op-rar-pw",
        &AtomicBool::new(false),
        None,
        Some("unrar".to_string()),
        &FailOnConflict,
        |_| {},
    )
    .expect("failed to extract password protected rar");

    assert_eq!(summary.extracted_files, 1);
    let extracted_file = dest.join(".gitignore");
    assert!(extracted_file.exists());
    let content = fs::read_to_string(&extracted_file).unwrap();
    assert_eq!(content, "target\nCargo.lock\n");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_extract_rar_missing_password() {
    let archive_path = fixture_path("crypted.rar");
    let root = common::temp_dir("rar-missing-pw");
    let dest = root.join("extracted");
    fs::create_dir(&dest).unwrap();

    let err = extract_any(
        &archive_path,
        &dest,
        "op-rar-missing-pw",
        &AtomicBool::new(false),
        None,
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap_err();

    assert!(
        err.code == "password_required" || err.code == "wrong_password",
        "expected password error, got: {:?}",
        err
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_extract_rar_cancellation() {
    let archive_path = fixture_path("version.rar");
    let root = common::temp_dir("rar-cancel");
    let dest = root.join("extracted");
    fs::create_dir(&dest).unwrap();

    let cancelled = AtomicBool::new(true);
    let err = extract_any(
        &archive_path,
        &dest,
        "op-rar-cancel",
        &cancelled,
        None,
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap_err();

    assert_eq!(err.code, "cancelled");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn test_extract_rar_selective() {
    let archive_path = fixture_path("version.rar");
    let root = common::temp_dir("rar-sel");
    let dest = root.join("extracted");
    fs::create_dir(&dest).unwrap();

    let selected = vec!["VERSION".to_string()];
    let summary = extract_any(
        &archive_path,
        &dest,
        "op-rar-sel",
        &AtomicBool::new(false),
        Some(&selected),
        None,
        &FailOnConflict,
        |_| {},
    )
    .expect("selective extraction failed");

    assert_eq!(summary.extracted_files, 1);
    assert!(dest.join("VERSION").exists());

    let _ = fs::remove_dir_all(root);
}
