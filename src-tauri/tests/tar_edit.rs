mod common;

use archi_backend_lib::archive::open_archive;
use archi_backend_lib::models::{CompressionPreset, CreateFormat, CreateOptions, EditOptions};
use archi_backend_lib::tar_create::create_tar_archive;
use archi_backend_lib::tar_edit::{
    add_paths, create_folder, delete_entries, move_entries, rename_entry, replace_file,
};
use std::cell::Cell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

fn default_edit_options() -> EditOptions {
    EditOptions::default()
}

fn create_options(format: CreateFormat) -> CreateOptions {
    CreateOptions {
        format,
        compression: CompressionPreset::Fast,
        include_root: false,
        overwrite: false,
    }
}

fn create_sample_tar(root: &Path, format: CreateFormat, ext: &str) -> PathBuf {
    let src = root.join("pack");
    fs::create_dir_all(src.join("nested")).unwrap();
    fs::write(src.join("keep.txt"), b"keep-data").unwrap();
    fs::write(src.join("drop.txt"), b"drop-data").unwrap();
    fs::write(src.join("nested").join("item.txt"), b"nested").unwrap();
    fs::write(src.join("other.txt"), b"other").unwrap();

    let out = root.join(format!("sample.{ext}"));
    create_tar_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "tar-edit-create",
        &AtomicBool::new(false),
        &create_options(format),
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

// ── Plain TAR ───────────────────────────────────────────────────────────────

#[test]
fn tar_delete_file_removes_entry_stream_rebuild() {
    let root = common::temp_dir("tar-edit-delete");
    let archive = create_sample_tar(&root, CreateFormat::Tar, "tar");

    let summary = delete_entries(
        &archive,
        &["drop.txt".into()],
        "tar-edit-delete-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.operation_id, "tar-edit-delete-1");
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
fn tar_rename_file_changes_path_and_preserves_content() {
    let root = common::temp_dir("tar-edit-rename");
    let archive = create_sample_tar(&root, CreateFormat::Tar, "tar");

    let summary = rename_entry(
        &archive,
        "other.txt",
        "renamed/new-name.txt",
        "tar-edit-rename-1",
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
fn tar_move_entries_relocates_leaf() {
    let root = common::temp_dir("tar-edit-move");
    let archive = create_sample_tar(&root, CreateFormat::Tar, "tar");

    let summary = move_entries(
        &archive,
        &["keep.txt".into()],
        "nested",
        "tar-edit-move-1",
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
fn tar_create_folder_adds_directory() {
    let root = common::temp_dir("tar-edit-mkdir");
    let archive = create_sample_tar(&root, CreateFormat::Tar, "tar");

    let summary = create_folder(
        &archive,
        "brand-new",
        "tar-edit-mkdir-1",
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
fn tar_add_file_into_parent() {
    let root = common::temp_dir("tar-edit-add");
    let archive = create_sample_tar(&root, CreateFormat::Tar, "tar");
    let source = root.join("added.txt");
    fs::write(&source, b"added-bytes").unwrap();

    let summary = add_paths(
        &archive,
        &[source.to_string_lossy().into_owned()],
        "nested",
        "tar-edit-add-1",
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
fn tar_replace_file_updates_content() {
    let root = common::temp_dir("tar-edit-replace");
    let archive = create_sample_tar(&root, CreateFormat::Tar, "tar");
    let source = root.join("replacement.txt");
    fs::write(&source, b"replaced-payload").unwrap();

    let summary = replace_file(
        &archive,
        "keep.txt",
        &source,
        "tar-edit-replace-1",
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
fn tar_cancel_delete_preserves_original_and_removes_temp() {
    let root = common::temp_dir("tar-edit-cancel");
    // Larger payload so cancel can fire mid-stream rebuild (not only on final progress).
    let src = root.join("pack");
    fs::create_dir_all(src.join("nested")).unwrap();
    let big = vec![b'x'; 256 * 1024];
    fs::write(src.join("keep.txt"), &big).unwrap();
    fs::write(src.join("drop.txt"), &big).unwrap();
    fs::write(src.join("nested").join("item.txt"), &big).unwrap();
    fs::write(src.join("other.txt"), &big).unwrap();
    fs::write(src.join("extra1.txt"), &big).unwrap();
    fs::write(src.join("extra2.txt"), &big).unwrap();
    let archive = root.join("sample.tar");
    create_tar_archive(
        &[src.to_string_lossy().into_owned()],
        &archive,
        "tar-edit-cancel-create",
        &AtomicBool::new(false),
        &create_options(CreateFormat::Tar),
        |_| {},
    )
    .unwrap();

    let original_bytes = fs::read(&archive).unwrap();
    let cancelled = AtomicBool::new(false);
    let progress_calls = Cell::new(0);

    let error = delete_entries(
        &archive,
        &["drop.txt".into(), "other.txt".into()],
        "tar-edit-cancel-1",
        &cancelled,
        |_| {
            let call = progress_calls.get() + 1;
            progress_calls.set(call);
            if call == 1 {
                assert!(!temporary_edit_archives(&root).is_empty());
            }
            // Cancel as soon as rebuild progress starts so the original stays untouched.
            cancelled.store(true, Ordering::Relaxed);
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
fn tar_delete_directory_prefix_removes_children() {
    let root = common::temp_dir("tar-edit-delete-dir");
    let archive = create_sample_tar(&root, CreateFormat::Tar, "tar");

    delete_entries(
        &archive,
        &["nested".into()],
        "tar-edit-delete-dir",
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

// ── TarGz ───────────────────────────────────────────────────────────────────

#[test]
fn tgz_delete_file_removes_entry_stream_rebuild() {
    let root = common::temp_dir("tgz-edit-delete");
    let archive = create_sample_tar(&root, CreateFormat::TarGz, "tar.gz");

    let summary = delete_entries(
        &archive,
        &["drop.txt".into()],
        "tgz-edit-delete-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("stream_rebuild"));
    let names = entry_names(&archive);
    assert!(!names.iter().any(|n| n == "drop.txt"));
    assert!(names.iter().any(|n| n == "keep.txt"));
    assert_eq!(entry_bytes_via_extract(&archive, "keep.txt"), b"keep-data");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tgz_rename_file_changes_path_and_preserves_content() {
    let root = common::temp_dir("tgz-edit-rename");
    let archive = create_sample_tar(&root, CreateFormat::TarGz, "tar.gz");

    let summary = rename_entry(
        &archive,
        "other.txt",
        "renamed/new-name.txt",
        "tgz-edit-rename-1",
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

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tgz_move_entries_relocates_leaf() {
    let root = common::temp_dir("tgz-edit-move");
    let archive = create_sample_tar(&root, CreateFormat::TarGz, "tar.gz");

    let summary = move_entries(
        &archive,
        &["keep.txt".into()],
        "nested",
        "tgz-edit-move-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("stream_rebuild"));
    assert!(entry_names(&archive).iter().any(|n| n == "nested/keep.txt"));
    assert_eq!(
        entry_bytes_via_extract(&archive, "nested/keep.txt"),
        b"keep-data"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tgz_create_folder_adds_directory() {
    let root = common::temp_dir("tgz-edit-mkdir");
    let archive = create_sample_tar(&root, CreateFormat::TarGz, "tar.gz");

    let summary = create_folder(
        &archive,
        "brand-new",
        "tgz-edit-mkdir-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("stream_rebuild"));
    assert!(entry_names(&archive).iter().any(|n| n == "brand-new"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tgz_add_file_into_parent() {
    let root = common::temp_dir("tgz-edit-add");
    let archive = create_sample_tar(&root, CreateFormat::TarGz, "tar.gz");
    let source = root.join("added.txt");
    fs::write(&source, b"added-bytes").unwrap();

    let summary = add_paths(
        &archive,
        &[source.to_string_lossy().into_owned()],
        "nested",
        "tgz-edit-add-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("stream_rebuild"));
    assert_eq!(
        entry_bytes_via_extract(&archive, "nested/added.txt"),
        b"added-bytes"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tgz_replace_file_updates_content() {
    let root = common::temp_dir("tgz-edit-replace");
    let archive = create_sample_tar(&root, CreateFormat::TarGz, "tar.gz");
    let source = root.join("replacement.txt");
    fs::write(&source, b"replaced-payload").unwrap();

    let summary = replace_file(
        &archive,
        "keep.txt",
        &source,
        "tgz-edit-replace-1",
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

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn tgz_cancel_delete_preserves_original_and_removes_temp() {
    let root = common::temp_dir("tgz-edit-cancel");
    let src = root.join("pack");
    fs::create_dir_all(src.join("nested")).unwrap();
    let big = vec![b'y'; 256 * 1024];
    fs::write(src.join("keep.txt"), &big).unwrap();
    fs::write(src.join("drop.txt"), &big).unwrap();
    fs::write(src.join("nested").join("item.txt"), &big).unwrap();
    fs::write(src.join("other.txt"), &big).unwrap();
    fs::write(src.join("extra1.txt"), &big).unwrap();
    fs::write(src.join("extra2.txt"), &big).unwrap();
    let archive = root.join("sample.tar.gz");
    create_tar_archive(
        &[src.to_string_lossy().into_owned()],
        &archive,
        "tgz-edit-cancel-create",
        &AtomicBool::new(false),
        &create_options(CreateFormat::TarGz),
        |_| {},
    )
    .unwrap();

    let original_bytes = fs::read(&archive).unwrap();
    let cancelled = AtomicBool::new(false);
    let progress_calls = Cell::new(0);

    let error = delete_entries(
        &archive,
        &["drop.txt".into(), "other.txt".into()],
        "tgz-edit-cancel-1",
        &cancelled,
        |_| {
            let call = progress_calls.get() + 1;
            progress_calls.set(call);
            if call == 1 {
                assert!(!temporary_edit_archives(&root).is_empty());
            }
            cancelled.store(true, Ordering::Relaxed);
        },
        &default_edit_options(),
    )
    .unwrap_err();

    assert_eq!(error.code, "cancelled");
    assert_eq!(fs::read(&archive).unwrap(), original_bytes);
    assert!(temporary_edit_archives(&root).is_empty());

    fs::remove_dir_all(root).unwrap();
}
