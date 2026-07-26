mod common;

use archi_backend_lib::archive::open_archive;
use archi_backend_lib::extraction::{extract_any, FailOnConflict};
use archi_backend_lib::models::{CompressionPreset, CreateFormat, CreateOptions};
use archi_backend_lib::testing::test_archive;
use archi_backend_lib::zipper::create_zip_archive;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn encrypted_options(password: &str) -> CreateOptions {
    CreateOptions {
        format: CreateFormat::Zip,
        compression: CompressionPreset::Normal,
        include_root: false,
        overwrite: true,
        password: Some(password.to_string()),
    }
}

fn create_encrypted_zip(root: &Path, password: &str) -> PathBuf {
    let src = root.join("pack");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("secret.txt"), b"super secret").unwrap();
    fs::write(src.join("plain.txt"), b"plain data").unwrap();
    let out = root.join("encrypted.zip");
    create_zip_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "zip-aes-create",
        &AtomicBool::new(false),
        &encrypted_options(password),
        |_| {},
    )
    .unwrap();
    out
}

#[test]
fn create_zip_with_password_marks_entries_encrypted() {
    let root = common::temp_dir("zip-aes-mark");
    let out = create_encrypted_zip(&root, "hunter2");

    let info = open_archive(&out, Some("hunter2".into())).unwrap();
    assert_eq!(info.format, "zip");
    let methods: Vec<_> = info
        .entries
        .iter()
        .filter(|e| !e.is_directory)
        .map(|e| (e.name.clone(), e.method.clone()))
        .collect();
    assert!(
        info.entries
            .iter()
            .any(|e| e.name == "secret.txt" && e.method.as_deref() == Some("AES-256")),
        "methods: {methods:?}"
    );
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn open_zip_listing_without_password_warns_encrypted() {
    let root = common::temp_dir("zip-aes-list");
    let out = create_encrypted_zip(&root, "hunter2");

    // ZIP central directory is plaintext: listing works, warning flags encryption.
    let info = open_archive(&out, None).unwrap();
    assert!(info.warnings.iter().any(|w| w.code == "encrypted"));
    assert!(info.entries.iter().any(|e| e.name == "secret.txt"));
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn extract_encrypted_zip_without_password_requires_password() {
    let root = common::temp_dir("zip-aes-nopass");
    let out = create_encrypted_zip(&root, "hunter2");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();

    let err = extract_any(
        &out,
        &dest,
        "zip-aes-ex-nopass",
        &AtomicBool::new(false),
        None,
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code, "password_required");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn extract_encrypted_zip_with_wrong_password_fails() {
    let root = common::temp_dir("zip-aes-wrong");
    let out = create_encrypted_zip(&root, "hunter2");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();

    let err = extract_any(
        &out,
        &dest,
        "zip-aes-ex-wrong",
        &AtomicBool::new(false),
        None,
        Some("wrong-password".into()),
        &FailOnConflict,
        |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code, "password_required");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn extract_encrypted_zip_with_password_round_trips() {
    let root = common::temp_dir("zip-aes-roundtrip");
    let out = create_encrypted_zip(&root, "hunter2");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();

    extract_any(
        &out,
        &dest,
        "zip-aes-ex-ok",
        &AtomicBool::new(false),
        None,
        Some("hunter2".into()),
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("secret.txt")).unwrap(), b"super secret");
    assert_eq!(fs::read(dest.join("plain.txt")).unwrap(), b"plain data");
    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn test_encrypted_zip_password_flow() {
    let root = common::temp_dir("zip-aes-test");
    let out = create_encrypted_zip(&root, "hunter2");

    let err = test_archive(&out, "zip-aes-test-nopass", &AtomicBool::new(false), None, |_| {})
        .unwrap_err();
    assert_eq!(err.code, "password_required");

    let summary = test_archive(
        &out,
        "zip-aes-test-ok",
        &AtomicBool::new(false),
        Some("hunter2".into()),
        |_| {},
    )
    .unwrap();
    assert_eq!(summary.tested_failed, 0);
    assert!(summary.tested_ok >= 2);
    fs::remove_dir_all(&root).unwrap();
}
