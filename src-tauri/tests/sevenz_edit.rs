mod common;

use archi_backend_lib::archive::open_archive;
use archi_backend_lib::models::{
    CompressionPreset, CreateFormat, CreateOptions, EditOptions,
};
use archi_backend_lib::sevenz_edit::{
    add_paths, create_folder, delete_entries, move_entries, rename_entry, replace_file,
};
use archi_backend_lib::sevenz_format::create_sevenz_archive;
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

fn default_edit_options() -> EditOptions {
    EditOptions::default()
}

fn fast_create_options() -> CreateOptions {
    CreateOptions {
        format: CreateFormat::SevenZ,
        compression: CompressionPreset::Fast,
        include_root: false,
        overwrite: false,
    }
}

fn create_sample_7z(root: &Path) -> PathBuf {
    let src = root.join("pack");
    fs::create_dir_all(src.join("nested")).unwrap();
    fs::write(src.join("keep.txt"), b"keep-data").unwrap();
    fs::write(src.join("drop.txt"), b"drop-data").unwrap();
    fs::write(src.join("nested").join("item.txt"), b"nested").unwrap();
    fs::write(src.join("other.txt"), b"other").unwrap();

    let out = root.join("sample.7z");
    create_sevenz_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "7z-edit-create",
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

fn temporary_edit_archives(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            let name = path.file_name().unwrap().to_string_lossy();
            name.contains(".archi-part-") || name.contains(".archi-edit-")
        })
        .collect()
}

#[test]
fn delete_file_removes_entry_stream_rebuild() {
    let root = common::temp_dir("7z-edit-delete");
    let archive = create_sample_7z(&root);

    let summary = delete_entries(
        &archive,
        &["drop.txt".into()],
        "7z-edit-delete-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.operation_id, "7z-edit-delete-1");
    assert_eq!(summary.destination, archive.to_string_lossy());
    assert_eq!(summary.strategy_used.as_deref(), Some("stream_rebuild"));
    assert!(summary.members_written >= 2);

    let names = entry_names(&archive);
    assert!(!names.iter().any(|n| n == "drop.txt"));
    assert!(names.iter().any(|n| n == "keep.txt"));
    assert!(names.iter().any(|n| n == "nested/item.txt"));
    assert_eq!(entry_bytes_via_extract(&archive, "keep.txt"), b"keep-data");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rename_file_changes_path_and_preserves_content() {
    let root = common::temp_dir("7z-edit-rename");
    let archive = create_sample_7z(&root);

    let summary = rename_entry(
        &archive,
        "other.txt",
        "renamed/new-name.txt",
        "7z-edit-rename-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("stream_rebuild"));
    let names = entry_names(&archive);
    assert!(!names.iter().any(|n| n == "other.txt"));
    assert!(names.iter().any(|n| n == "renamed/new-name.txt"));
    assert_eq!(
        entry_bytes_via_extract(&archive, "renamed/new-name.txt"),
        b"other"
    );
    assert_eq!(entry_bytes_via_extract(&archive, "keep.txt"), b"keep-data");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn move_entries_relocates_leaf() {
    let root = common::temp_dir("7z-edit-move");
    let archive = create_sample_7z(&root);

    let summary = move_entries(
        &archive,
        &["keep.txt".into()],
        "nested",
        "7z-edit-move-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("stream_rebuild"));
    let names = entry_names(&archive);
    assert!(!names.iter().any(|n| n == "keep.txt"));
    assert!(names.iter().any(|n| n == "nested/keep.txt"));
    assert_eq!(
        entry_bytes_via_extract(&archive, "nested/keep.txt"),
        b"keep-data"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn create_folder_adds_directory() {
    let root = common::temp_dir("7z-edit-mkdir");
    let archive = create_sample_7z(&root);

    let summary = create_folder(
        &archive,
        "brand-new",
        "7z-edit-mkdir-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("stream_rebuild"));
    let names = entry_names(&archive);
    assert!(names.iter().any(|n| n == "brand-new"));
    assert!(names.iter().any(|n| n == "keep.txt"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn add_file_into_parent() {
    let root = common::temp_dir("7z-edit-add");
    let archive = create_sample_7z(&root);
    let source = root.join("added.txt");
    fs::write(&source, b"added-bytes").unwrap();

    let summary = add_paths(
        &archive,
        &[source.to_string_lossy().into_owned()],
        "nested",
        "7z-edit-add-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("stream_rebuild"));
    let names = entry_names(&archive);
    assert!(names.iter().any(|n| n == "nested/added.txt"));
    assert_eq!(
        entry_bytes_via_extract(&archive, "nested/added.txt"),
        b"added-bytes"
    );
    assert_eq!(
        entry_bytes_via_extract(&archive, "nested/item.txt"),
        b"nested"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replace_file_updates_content() {
    let root = common::temp_dir("7z-edit-replace");
    let archive = create_sample_7z(&root);
    let source = root.join("replacement.txt");
    fs::write(&source, b"replaced-payload").unwrap();

    let summary = replace_file(
        &archive,
        "keep.txt",
        &source,
        "7z-edit-replace-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("stream_rebuild"));
    assert_eq!(
        entry_bytes_via_extract(&archive, "keep.txt"),
        b"replaced-payload"
    );
    assert_eq!(entry_bytes_via_extract(&archive, "drop.txt"), b"drop-data");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancel_delete_preserves_original_and_removes_temp() {
    let root = common::temp_dir("7z-edit-cancel");
    let archive = create_sample_7z(&root);
    let original_bytes = fs::read(&archive).unwrap();
    let cancelled = AtomicBool::new(false);
    let progress_calls = Cell::new(0);

    let error = delete_entries(
        &archive,
        &["drop.txt".into(), "other.txt".into()],
        "7z-edit-cancel-1",
        &cancelled,
        |_| {
            let call = progress_calls.get() + 1;
            progress_calls.set(call);
            if call == 1 {
                assert!(!temporary_edit_archives(&root).is_empty());
                std::thread::sleep(std::time::Duration::from_millis(120));
            } else if call >= 2 {
                cancelled.store(true, Ordering::Relaxed);
            }
        },
        &default_edit_options(),
    )
    .unwrap_err();

    assert_eq!(error.code, "cancelled");
    assert_eq!(fs::read(&archive).unwrap(), original_bytes);
    assert!(temporary_edit_archives(&root).is_empty());
    let names = entry_names(&archive);
    assert!(names.iter().any(|n| n == "drop.txt"));
    assert!(names.iter().any(|n| n == "other.txt"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_directory_prefix_removes_children() {
    let root = common::temp_dir("7z-edit-delete-dir");
    let archive = create_sample_7z(&root);

    delete_entries(
        &archive,
        &["nested".into()],
        "7z-edit-delete-dir",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    let names = entry_names(&archive);
    assert!(!names
        .iter()
        .any(|n| n == "nested" || n.starts_with("nested/")));
    assert!(names.iter().any(|n| n == "keep.txt"));

    fs::remove_dir_all(root).unwrap();
}
