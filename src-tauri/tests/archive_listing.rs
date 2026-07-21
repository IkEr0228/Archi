mod common;

use archi_backend_lib::archive::open_archive;
use std::fs::File;
use std::io::Write;
use zip::write::FileOptions;
use zip::{CompressionMethod, DateTime, ZipWriter};

#[test]
fn lists_zip_metadata_once() {
    let root = common::temp_dir("listing");
    let archive_path = root.join("listing.zip");
    common::write_zip(&archive_path, &[("folder/file.txt", b"content")]);

    let info = open_archive(&archive_path).unwrap();

    assert_eq!(info.format, "zip");
    assert!(info.capabilities.test);
    assert_eq!(info.stats.file_count, 1);
    assert_eq!(info.stats.folder_count, 1);
    assert!(info.stats.total_uncompressed >= 7);
    assert!(info.stats.methods.iter().any(|m| m.contains("Deflated")));
    assert_eq!(
        info.entries
            .iter()
            .find(|entry| entry.path == "folder/file.txt")
            .unwrap()
            .method
            .as_deref(),
        Some("Deflated")
    );
    assert_eq!(
        info.entries
            .iter()
            .filter(|entry| entry.path == "folder")
            .count(),
        1
    );
    assert_eq!(
        info.entries
            .iter()
            .find(|entry| entry.path == "folder")
            .unwrap()
            .modified_at,
        None
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_directory_metadata_replaces_synthesized_metadata() {
    let root = common::temp_dir("explicit-directory");
    let archive_path = root.join("listing.zip");
    let mut zip = ZipWriter::new(File::create(&archive_path).unwrap());
    let file_options = FileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::from_date_and_time(2023, 1, 2, 3, 4, 6).unwrap());
    let directory_options = FileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(DateTime::from_date_and_time(2024, 5, 6, 7, 8, 10).unwrap());
    zip.start_file("explicit/file.txt", file_options).unwrap();
    zip.write_all(b"content").unwrap();
    zip.add_directory("explicit/", directory_options).unwrap();
    zip.finish().unwrap();

    let info = open_archive(&archive_path).unwrap();

    assert_eq!(
        info.entries
            .iter()
            .find(|entry| entry.path == "explicit")
            .unwrap()
            .modified_at
            .as_deref(),
        Some("2024-05-06 07:08:10")
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_invalid_entry_paths() {
    let root = common::temp_dir("invalid-entry");
    let archive_path = root.join("invalid.zip");
    common::write_zip(&archive_path, &[("../evil.txt", b"content")]);

    let error = open_archive(&archive_path).unwrap_err();

    assert_eq!(error.code, "invalid_entry");
    std::fs::remove_dir_all(root).unwrap();
}
