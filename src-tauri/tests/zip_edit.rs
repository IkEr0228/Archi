mod common;

use archi_backend_lib::archive::open_archive;
use archi_backend_lib::models::{EditOptions, EditStrategyPref};
use archi_backend_lib::zip_edit::{
    add_paths, create_folder, delete_entries, rename_entry, replace_file,
};
use std::cell::Cell;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use zip::ZipArchive;

fn default_edit_options() -> EditOptions {
    EditOptions::default()
}

fn compact_edit_options() -> EditOptions {
    EditOptions {
        strategy: Some(EditStrategyPref::PreferCompact),
        ..Default::default()
    }
}

fn fast_edit_options() -> EditOptions {
    EditOptions {
        strategy: Some(EditStrategyPref::PreferFast),
        ..Default::default()
    }
}

fn auto_edit_options() -> EditOptions {
    EditOptions {
        strategy: Some(EditStrategyPref::Auto),
        ..Default::default()
    }
}

fn temporary_edit_archives(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".archi-edit-")
        })
        .collect()
}

fn entry_names(zip_path: &Path) -> Vec<String> {
    let file = File::open(zip_path).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();
    let mut names: Vec<String> = (0..archive.len())
        .map(|index| {
            archive
                .by_index(index)
                .unwrap()
                .name()
                .trim_matches('/')
                .replace('\\', "/")
                .to_string()
        })
        .collect();
    names.sort();
    names
}

fn entry_bytes(zip_path: &Path, name: &str) -> Vec<u8> {
    let file = File::open(zip_path).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();
    let mut entry = archive.by_name(name).unwrap();
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn delete_file_removes_entry_and_keeps_valid_zip() {
    let root = common::temp_dir("zip-edit-delete");
    let zip_path = root.join("sample.zip");
    common::write_zip(
        &zip_path,
        &[
            ("keep.txt", b"keep-data"),
            ("drop.txt", b"drop-data"),
            ("nested/item.txt", b"nested"),
        ],
    );
    let original = zip_path.clone();

    let summary = delete_entries(
        &zip_path,
        &["drop.txt".into()],
        "edit-delete-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.operation_id, "edit-delete-1");
    assert_eq!(summary.destination, zip_path.to_string_lossy());
    assert!(summary.members_written >= 2);
    assert!(original.is_file());
    assert!(ZipArchive::new(File::open(&zip_path).unwrap()).is_ok());

    let names = entry_names(&zip_path);
    assert!(!names.iter().any(|n| n == "drop.txt"));
    assert!(names.iter().any(|n| n == "keep.txt"));
    assert!(names.iter().any(|n| n == "nested/item.txt"));
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"keep-data");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rename_file_changes_path_and_preserves_content() {
    let root = common::temp_dir("zip-edit-rename");
    let zip_path = root.join("sample.zip");
    common::write_zip(
        &zip_path,
        &[("old-name.txt", b"payload-bytes"), ("other.txt", b"other")],
    );

    let summary = rename_entry(
        &zip_path,
        "old-name.txt",
        "renamed/new-name.txt",
        "edit-rename-1",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();

    assert_eq!(summary.operation_id, "edit-rename-1");
    assert_eq!(summary.destination, zip_path.to_string_lossy());
    assert!(ZipArchive::new(File::open(&zip_path).unwrap()).is_ok());

    let names = entry_names(&zip_path);
    assert!(!names.iter().any(|n| n == "old-name.txt"));
    assert!(names.iter().any(|n| n == "renamed/new-name.txt"));
    assert_eq!(
        entry_bytes(&zip_path, "renamed/new-name.txt"),
        b"payload-bytes"
    );
    assert_eq!(entry_bytes(&zip_path, "other.txt"), b"other");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancel_delete_preserves_original_and_removes_temp() {
    let root = common::temp_dir("zip-edit-cancel");
    let zip_path = root.join("sample.zip");
    common::write_zip(
        &zip_path,
        &[
            ("a.txt", b"aaa"),
            ("b.txt", b"bbb"),
            ("c.txt", b"ccc"),
            ("d.txt", b"ddd"),
        ],
    );
    let original_bytes = fs::read(&zip_path).unwrap();
    let cancelled = AtomicBool::new(false);
    let progress_calls = Cell::new(0);

    // PreferCompact forces rebuild so multi-member progress can cancel mid-edit.
    let error = delete_entries(
        &zip_path,
        &["a.txt".into(), "b.txt".into()],
        "edit-cancel-1",
        &cancelled,
        |_| {
            let call = progress_calls.get() + 1;
            progress_calls.set(call);
            if call == 1 {
                assert!(!temporary_edit_archives(&root).is_empty());
                // Exceed PROGRESS_INTERVAL so a later rebuild member still emits progress.
                std::thread::sleep(std::time::Duration::from_millis(120));
            } else if call >= 2 {
                cancelled.store(true, Ordering::Relaxed);
            }
        },
        &compact_edit_options(),
    )
    .unwrap_err();

    assert_eq!(error.code, "cancelled");
    assert_eq!(fs::read(&zip_path).unwrap(), original_bytes);
    assert!(ZipArchive::new(File::open(&zip_path).unwrap()).is_ok());
    assert!(temporary_edit_archives(&root).is_empty());
    assert_eq!(entry_names(&zip_path).len(), 4);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn zip_open_reports_edit_capability() {
    let root = common::temp_dir("zip-edit-cap");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("a.txt", b"a")]);

    let info = open_archive(&zip_path).unwrap();
    assert!(info.capabilities.edit);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_directory_prefix_removes_children() {
    let root = common::temp_dir("zip-edit-delete-dir");
    let zip_path = root.join("sample.zip");
    common::write_zip_with_dirs(
        &zip_path,
        &["folder/"],
        &[
            ("folder/a.txt", b"a"),
            ("folder/sub/b.txt", b"b"),
            ("keep.txt", b"k"),
        ],
    );

    delete_entries(
        &zip_path,
        &["folder".into()],
        "edit-delete-dir",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    let names = entry_names(&zip_path);
    assert!(!names
        .iter()
        .any(|n| n == "folder" || n.starts_with("folder/")));
    assert!(names.iter().any(|n| n == "keep.txt"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rename_directory_rewrites_prefix() {
    let root = common::temp_dir("zip-edit-rename-dir");
    let zip_path = root.join("sample.zip");
    common::write_zip_with_dirs(
        &zip_path,
        &["old/"],
        &[
            ("old/a.txt", b"a"),
            ("old/sub/b.txt", b"b"),
            ("root.txt", b"r"),
        ],
    );

    rename_entry(
        &zip_path,
        "old",
        "new",
        "edit-rename-dir",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();

    let names = entry_names(&zip_path);
    assert!(!names.iter().any(|n| n == "old" || n.starts_with("old/")));
    assert!(names.iter().any(|n| n == "new" || n == "new/a.txt"));
    assert!(names.iter().any(|n| n == "new/sub/b.txt"));
    assert_eq!(entry_bytes(&zip_path, "new/a.txt"), b"a");
    assert_eq!(entry_bytes(&zip_path, "root.txt"), b"r");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rename_rejects_target_collision() {
    let root = common::temp_dir("zip-edit-rename-collision");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("from.txt", b"from"), ("to.txt", b"to")]);

    let error = rename_entry(
        &zip_path,
        "from.txt",
        "to.txt",
        "edit-rename-collision",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();

    assert_eq!(error.code, "entry_exists");
    assert_eq!(entry_bytes(&zip_path, "from.txt"), b"from");
    assert_eq!(entry_bytes(&zip_path, "to.txt"), b"to");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn create_folder_adds_directory_entry() {
    let root = common::temp_dir("zip-edit-create-folder");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("keep.txt", b"keep")]);

    let summary = create_folder(
        &zip_path,
        "new-folder",
        "edit-create-folder-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.operation_id, "edit-create-folder-1");
    assert_eq!(summary.destination, zip_path.to_string_lossy());
    assert!(summary.members_written >= 1);
    assert_eq!(summary.strategy_used.as_deref(), Some("append"));
    assert!(ZipArchive::new(File::open(&zip_path).unwrap()).is_ok());

    let names = entry_names(&zip_path);
    assert!(names.iter().any(|n| n == "new-folder"));
    assert!(names.iter().any(|n| n == "keep.txt"));
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"keep");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn create_folder_rejects_existing_path() {
    let root = common::temp_dir("zip-edit-create-folder-exists");
    let zip_path = root.join("sample.zip");
    common::write_zip_with_dirs(
        &zip_path,
        &["existing/"],
        &[("existing/a.txt", b"a"), ("keep.txt", b"k")],
    );

    let error = create_folder(
        &zip_path,
        "existing",
        "edit-create-folder-exists",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap_err();

    assert_eq!(error.code, "entry_exists");
    assert!(entry_names(&zip_path).iter().any(|n| n == "keep.txt"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn add_file_into_nested_parent() {
    let root = common::temp_dir("zip-edit-add-file");
    let zip_path = root.join("sample.zip");
    common::write_zip_with_dirs(
        &zip_path,
        &["nested/"],
        &[("nested/keep.txt", b"keep"), ("root.txt", b"root")],
    );

    let source = root.join("added.txt");
    fs::write(&source, b"added-bytes").unwrap();

    let summary = add_paths(
        &zip_path,
        &[source.to_string_lossy().into_owned()],
        "nested",
        "edit-add-1",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.operation_id, "edit-add-1");
    assert_eq!(summary.strategy_used.as_deref(), Some("append"));
    assert!(ZipArchive::new(File::open(&zip_path).unwrap()).is_ok());

    let names = entry_names(&zip_path);
    assert!(names.iter().any(|n| n == "nested/added.txt"));
    assert_eq!(entry_bytes(&zip_path, "nested/added.txt"), b"added-bytes");
    assert_eq!(entry_bytes(&zip_path, "nested/keep.txt"), b"keep");
    assert_eq!(entry_bytes(&zip_path, "root.txt"), b"root");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn add_directory_includes_root_and_children() {
    let root = common::temp_dir("zip-edit-add-dir");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("keep.txt", b"k")]);

    let dir = root.join("payload");
    fs::create_dir_all(dir.join("sub")).unwrap();
    fs::write(dir.join("a.txt"), b"aaa").unwrap();
    fs::write(dir.join("sub").join("b.txt"), b"bbb").unwrap();

    let summary = add_paths(
        &zip_path,
        &[dir.to_string_lossy().into_owned()],
        "",
        "edit-add-dir",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap();
    assert_eq!(summary.strategy_used.as_deref(), Some("append"));

    let names = entry_names(&zip_path);
    assert!(names.iter().any(|n| n == "payload/a.txt"));
    assert!(names.iter().any(|n| n == "payload/sub/b.txt"));
    assert_eq!(entry_bytes(&zip_path, "payload/a.txt"), b"aaa");
    assert_eq!(entry_bytes(&zip_path, "payload/sub/b.txt"), b"bbb");
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"k");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn add_rejects_existing_target() {
    let root = common::temp_dir("zip-edit-add-exists");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("dup.txt", b"old")]);

    let source = root.join("dup.txt");
    fs::write(&source, b"new").unwrap();

    let error = add_paths(
        &zip_path,
        &[source.to_string_lossy().into_owned()],
        "",
        "edit-add-exists",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap_err();

    assert_eq!(error.code, "entry_exists");
    assert_eq!(entry_bytes(&zip_path, "dup.txt"), b"old");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn add_rejects_parent_directory_containing_open_zip() {
    let root = common::temp_dir("zip-edit-add-containment");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("keep.txt", b"keep-data")]);
    let original_bytes = fs::read(&zip_path).unwrap();

    // Adding the parent of the open archive would pull the zip (and sibling temp) into itself.
    let error = add_paths(
        &zip_path,
        &[root.to_string_lossy().into_owned()],
        "",
        "edit-add-containment",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap_err();

    assert_eq!(error.code, "output_inside_source");
    assert_eq!(fs::read(&zip_path).unwrap(), original_bytes);
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"keep-data");
    assert!(temporary_edit_archives(&root).is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn add_rejects_file_equal_to_virtual_folder_prefix() {
    // Archive has docs/a.txt (virtual folder "docs"); adding a file named "docs" collides.
    let root = common::temp_dir("zip-edit-add-folder-prefix");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("docs/a.txt", b"child"), ("keep.txt", b"k")]);

    let source = root.join("docs");
    fs::write(&source, b"file-as-folder").unwrap();

    let error = add_paths(
        &zip_path,
        &[source.to_string_lossy().into_owned()],
        "",
        "edit-add-folder-prefix",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap_err();

    assert_eq!(error.code, "entry_exists");
    assert_eq!(entry_bytes(&zip_path, "docs/a.txt"), b"child");
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"k");
    assert!(temporary_edit_archives(&root).is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn add_rejects_under_existing_file_as_parent() {
    // Archive has file "docs"; adding docs/x.txt is file-as-parent collision.
    let root = common::temp_dir("zip-edit-add-file-parent");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("docs", b"file-body"), ("keep.txt", b"k")]);

    let source = root.join("x.txt");
    fs::write(&source, b"nested").unwrap();

    let error = add_paths(
        &zip_path,
        &[source.to_string_lossy().into_owned()],
        "docs",
        "edit-add-file-parent",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap_err();

    assert_eq!(error.code, "entry_exists");
    assert_eq!(entry_bytes(&zip_path, "docs"), b"file-body");
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"k");
    assert!(temporary_edit_archives(&root).is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn create_folder_prefer_compact_uses_rebuild() {
    let root = common::temp_dir("zip-edit-create-folder-compact");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("keep.txt", b"keep")]);

    let mut phases = Vec::new();
    let summary = create_folder(
        &zip_path,
        "compact-folder",
        "edit-create-folder-compact",
        &AtomicBool::new(false),
        |p| {
            if let Some(phase) = p.phase {
                phases.push(phase);
            }
        },
        &compact_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("rebuild"));
    assert!(phases.iter().any(|p| p == "rebuild"));
    assert!(!phases.iter().any(|p| p == "append"));
    assert!(entry_names(&zip_path).iter().any(|n| n == "compact-folder"));
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"keep");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn add_file_emits_append_phase() {
    let root = common::temp_dir("zip-edit-add-phase");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("keep.txt", b"k")]);
    let source = root.join("new.txt");
    fs::write(&source, b"new").unwrap();

    let mut phases = Vec::new();
    let summary = add_paths(
        &zip_path,
        &[source.to_string_lossy().into_owned()],
        "",
        "edit-add-phase",
        &AtomicBool::new(false),
        |p| {
            if let Some(phase) = p.phase {
                phases.push(phase);
            }
        },
        &default_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("append"));
    assert!(phases.iter().any(|p| p == "append"));
    assert_eq!(entry_bytes(&zip_path, "new.txt"), b"new");
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"k");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rename_rejects_destination_virtual_folder_prefix() {
    let root = common::temp_dir("zip-edit-rename-folder-prefix");
    let zip_path = root.join("sample.zip");
    common::write_zip(
        &zip_path,
        &[
            ("from.txt", b"from"),
            ("docs/a.txt", b"child"),
            ("keep.txt", b"k"),
        ],
    );

    let error = rename_entry(
        &zip_path,
        "from.txt",
        "docs",
        "edit-rename-folder-prefix",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();

    assert_eq!(error.code, "entry_exists");
    assert_eq!(entry_bytes(&zip_path, "from.txt"), b"from");
    assert_eq!(entry_bytes(&zip_path, "docs/a.txt"), b"child");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rename_rejects_destination_under_file_as_parent() {
    let root = common::temp_dir("zip-edit-rename-file-parent");
    let zip_path = root.join("sample.zip");
    common::write_zip(
        &zip_path,
        &[
            ("from.txt", b"from"),
            ("docs", b"file-body"),
            ("keep.txt", b"k"),
        ],
    );

    let error = rename_entry(
        &zip_path,
        "from.txt",
        "docs/x.txt",
        "edit-rename-file-parent",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();

    assert_eq!(error.code, "entry_exists");
    assert_eq!(entry_bytes(&zip_path, "from.txt"), b"from");
    assert_eq!(entry_bytes(&zip_path, "docs"), b"file-body");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_rejects_selection_matching_nothing() {
    let root = common::temp_dir("zip-edit-delete-noop");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("keep.txt", b"keep")]);
    let original_bytes = fs::read(&zip_path).unwrap();

    let error = delete_entries(
        &zip_path,
        &["missing.txt".into()],
        "edit-delete-noop",
        &AtomicBool::new(false),
        |_| {},
        &default_edit_options(),
    )
    .unwrap_err();

    assert_eq!(error.code, "not_found");
    assert_eq!(fs::read(&zip_path).unwrap(), original_bytes);
    assert!(temporary_edit_archives(&root).is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replace_file_updates_content() {
    let root = common::temp_dir("zip-edit-replace");
    let zip_path = root.join("sample.zip");
    common::write_zip(
        &zip_path,
        &[("target.txt", b"old-content"), ("other.txt", b"other")],
    );

    let source = root.join("replacement.bin");
    fs::write(&source, b"new-content-bytes").unwrap();

    let summary = replace_file(
        &zip_path,
        "target.txt",
        &source,
        "edit-replace-1",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();

    assert_eq!(summary.operation_id, "edit-replace-1");
    assert!(ZipArchive::new(File::open(&zip_path).unwrap()).is_ok());
    assert_eq!(entry_bytes(&zip_path, "target.txt"), b"new-content-bytes");
    assert_eq!(entry_bytes(&zip_path, "other.txt"), b"other");

    let names = entry_names(&zip_path);
    assert_eq!(names.len(), 2);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replace_rejects_missing_or_directory_entry() {
    let root = common::temp_dir("zip-edit-replace-bad");
    let zip_path = root.join("sample.zip");
    common::write_zip_with_dirs(&zip_path, &["folder/"], &[("folder/a.txt", b"a")]);
    let source = root.join("data.bin");
    fs::write(&source, b"data").unwrap();

    let missing = replace_file(
        &zip_path,
        "nope.txt",
        &source,
        "edit-replace-missing",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(missing.code, "not_found");

    let is_dir = replace_file(
        &zip_path,
        "folder",
        &source,
        "edit-replace-dir",
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(is_dir.code, "invalid_entry");

    assert_eq!(entry_bytes(&zip_path, "folder/a.txt"), b"a");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_prefer_fast_uses_logical_delete() {
    let root = common::temp_dir("zip-edit-delete-fast");
    let zip_path = root.join("sample.zip");
    common::write_zip(
        &zip_path,
        &[
            ("keep.txt", b"keep"),
            ("drop.txt", b"drop"),
            ("other.txt", b"other"),
        ],
    );

    let mut phases = Vec::new();
    let summary = delete_entries(
        &zip_path,
        &["drop.txt".into()],
        "edit-delete-fast",
        &AtomicBool::new(false),
        |p| {
            if let Some(phase) = p.phase {
                phases.push(phase);
            }
        },
        &fast_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("logical_delete"));
    assert!(phases.iter().any(|p| p == "logical_delete"));
    assert!(!phases.iter().any(|p| p == "rebuild"));
    assert!(!entry_names(&zip_path).iter().any(|n| n == "drop.txt"));
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"keep");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_prefer_compact_uses_rebuild() {
    let root = common::temp_dir("zip-edit-delete-compact");
    let zip_path = root.join("sample.zip");
    common::write_zip(&zip_path, &[("keep.txt", b"keep"), ("drop.txt", b"drop")]);

    let mut phases = Vec::new();
    let summary = delete_entries(
        &zip_path,
        &["drop.txt".into()],
        "edit-delete-compact",
        &AtomicBool::new(false),
        |p| {
            if let Some(phase) = p.phase {
                phases.push(phase);
            }
        },
        &compact_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("rebuild"));
    assert!(phases.iter().any(|p| p == "rebuild"));
    assert!(!phases.iter().any(|p| p == "logical_delete"));
    assert!(!entry_names(&zip_path).iter().any(|n| n == "drop.txt"));
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"keep");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_auto_small_fraction_uses_logical() {
    let root = common::temp_dir("zip-edit-delete-auto-small");
    let zip_path = root.join("sample.zip");
    // 1 of 8 = 12.5% ≤ 25% → logical
    let entries: Vec<(&str, &[u8])> = vec![
        ("a0.txt", b"0"),
        ("a1.txt", b"1"),
        ("a2.txt", b"2"),
        ("a3.txt", b"3"),
        ("a4.txt", b"4"),
        ("a5.txt", b"5"),
        ("a6.txt", b"6"),
        ("a7.txt", b"7"),
    ];
    common::write_zip(&zip_path, &entries);

    let summary = delete_entries(
        &zip_path,
        &["a0.txt".into()],
        "edit-delete-auto-small",
        &AtomicBool::new(false),
        |_| {},
        &auto_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("logical_delete"));
    assert_eq!(entry_names(&zip_path).len(), 7);
    assert!(!entry_names(&zip_path).iter().any(|n| n == "a0.txt"));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_auto_large_fraction_uses_rebuild() {
    let root = common::temp_dir("zip-edit-delete-auto-large");
    let zip_path = root.join("sample.zip");
    // Auto: logical if fraction ≤ 0.25 OR deleted ≤ 64. A 7-of-8 case hits the
    // absolute gate (deleted=7≤64); use 70 of 100 so both gates fail → rebuild.
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..100 {
        entries.push((format!("f{i:03}.txt"), vec![b'x'; 32]));
    }
    let entries_ref: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    common::write_zip(&zip_path, &entries_ref);

    let to_delete: Vec<String> = (0..70).map(|i| format!("f{i:03}.txt")).collect();
    let mut phases = Vec::new();
    let summary = delete_entries(
        &zip_path,
        &to_delete,
        "edit-delete-auto-large",
        &AtomicBool::new(false),
        |p| {
            if let Some(phase) = p.phase {
                phases.push(phase);
            }
        },
        &auto_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("rebuild"));
    assert!(phases.iter().any(|p| p == "rebuild"));
    assert_eq!(entry_names(&zip_path).len(), 30);

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_logical_leaves_orphan_local_data() {
    let root = common::temp_dir("zip-edit-delete-orphan");
    let zip_path = root.join("sample.zip");
    // Large payload so orphaned local data keeps file size from shrinking much.
    let big = vec![b'Z'; 64 * 1024];
    common::write_zip(
        &zip_path,
        &[
            ("keep.txt", b"keep-small"),
            ("big-drop.bin", big.as_slice()),
            ("other.txt", b"other"),
        ],
    );
    let size_before = fs::metadata(&zip_path).unwrap().len();

    let summary = delete_entries(
        &zip_path,
        &["big-drop.bin".into()],
        "edit-delete-orphan",
        &AtomicBool::new(false),
        |_| {},
        &fast_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("logical_delete"));
    assert!(!entry_names(&zip_path).iter().any(|n| n == "big-drop.bin"));
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"keep-small");

    let size_after = fs::metadata(&zip_path).unwrap().len();
    // Logical delete only rewrites CD/EOCD; local data for big-drop remains.
    // File must not shrink by nearly the full payload size.
    assert!(
        size_after + 8 * 1024 > size_before,
        "expected orphan local data to keep size near original ({size_before} -> {size_after})"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_all_entries_logical_yields_empty_zip() {
    let root = common::temp_dir("zip-edit-delete-all");
    let zip_path = root.join("sample.zip");
    common::write_zip(
        &zip_path,
        &[("a.txt", b"a"), ("b.txt", b"b"), ("c.txt", b"c")],
    );

    let summary = delete_entries(
        &zip_path,
        &["a.txt".into(), "b.txt".into(), "c.txt".into()],
        "edit-delete-all",
        &AtomicBool::new(false),
        |_| {},
        &fast_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("logical_delete"));
    assert_eq!(summary.members_written, 0);
    assert!(ZipArchive::new(File::open(&zip_path).unwrap()).is_ok());
    assert!(entry_names(&zip_path).is_empty());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_directory_prefix_logical() {
    let root = common::temp_dir("zip-edit-delete-dir-logical");
    let zip_path = root.join("sample.zip");
    common::write_zip_with_dirs(
        &zip_path,
        &["folder/"],
        &[
            ("folder/a.txt", b"a"),
            ("folder/sub/b.txt", b"b"),
            ("keep.txt", b"k"),
        ],
    );

    let summary = delete_entries(
        &zip_path,
        &["folder".into()],
        "edit-delete-dir-logical",
        &AtomicBool::new(false),
        |_| {},
        &fast_edit_options(),
    )
    .unwrap();

    assert_eq!(summary.strategy_used.as_deref(), Some("logical_delete"));
    let names = entry_names(&zip_path);
    assert!(!names
        .iter()
        .any(|n| n == "folder" || n.starts_with("folder/")));
    assert!(names.iter().any(|n| n == "keep.txt"));
    assert_eq!(entry_bytes(&zip_path, "keep.txt"), b"k");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancel_logical_delete_preserves_original() {
    let root = common::temp_dir("zip-edit-cancel-logical");
    let zip_path = root.join("sample.zip");
    common::write_zip(
        &zip_path,
        &[
            ("a.txt", b"aaa"),
            ("b.txt", b"bbb"),
            ("c.txt", b"ccc"),
            ("d.txt", b"ddd"),
        ],
    );
    let original_bytes = fs::read(&zip_path).unwrap();
    let cancelled = AtomicBool::new(true); // cancel before work starts

    let error = delete_entries(
        &zip_path,
        &["a.txt".into()],
        "edit-cancel-logical",
        &cancelled,
        |_| {},
        &fast_edit_options(),
    )
    .unwrap_err();

    assert_eq!(error.code, "cancelled");
    assert_eq!(fs::read(&zip_path).unwrap(), original_bytes);
    assert!(ZipArchive::new(File::open(&zip_path).unwrap()).is_ok());
    assert!(temporary_edit_archives(&root).is_empty());
    assert_eq!(entry_names(&zip_path).len(), 4);

    fs::remove_dir_all(root).unwrap();
}
