mod common;

use archi_backend_lib::commands::ensure_directory_path;
use std::fs;

#[test]
fn creates_missing_leaf_under_existing_parent() {
    let root = common::temp_dir("ensure-dir");
    let leaf = root.join("archive-name");
    assert!(!leaf.exists());
    let created = ensure_directory_path(&leaf).unwrap();
    assert!(fs::metadata(&created).unwrap().is_dir());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_when_parent_missing() {
    let root = common::temp_dir("ensure-missing-parent");
    let leaf = root.join("no-parent").join("child");
    let err = ensure_directory_path(&leaf).unwrap_err();
    assert_eq!(err.code, "invalid_destination");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_existing_file_as_leaf() {
    let root = common::temp_dir("ensure-file-leaf");
    let leaf = root.join("archive-name");
    fs::write(&leaf, b"not a directory").unwrap();
    let err = ensure_directory_path(&leaf).unwrap_err();
    assert_eq!(err.code, "invalid_destination");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn accepts_existing_directory_leaf() {
    let root = common::temp_dir("ensure-existing-dir");
    let leaf = root.join("archive-name");
    fs::create_dir(&leaf).unwrap();
    let created = ensure_directory_path(&leaf).unwrap();
    assert!(fs::metadata(&created).unwrap().is_dir());
    fs::remove_dir_all(root).unwrap();
}
