mod common;

use archi_backend_lib::extraction::{
    extract_archive, normalize_entry_name, selection_includes_entry, validate_selection,
    FailOnConflict, ScriptedConflictResolver,
};
use archi_backend_lib::models::ConflictDecision;
use archi_backend_lib::operations::OperationRegistry;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn cancellation_flag_is_shared_and_removed() {
    let registry = OperationRegistry::default();
    let state = registry.start("extract-1").unwrap();

    assert!(registry.cancel("extract-1"));
    assert!(state.cancelled.load(Ordering::Relaxed));

    registry.finish("extract-1");
    assert!(!registry.cancel("extract-1"));
}

#[test]
fn cancelled_extraction_removes_partial_file() {
    let root = common::temp_dir("cancel-extract");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    common::write_zip(&zip_path, &[("large.bin", &[7; 1024])]);

    let cancelled = AtomicBool::new(true);
    let result = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        None,
        &FailOnConflict,
        |_| {},
    );

    assert_eq!(result.unwrap_err().code, "cancelled");
    assert!(!destination.join("large.bin").exists());
    assert!(fs::read_dir(&destination).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains("archi-part")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn conflicting_destination_starts_no_partial_extraction() {
    let root = common::temp_dir("conflict-extract");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("existing.txt"), b"keep").unwrap();
    common::write_zip(
        &zip_path,
        &[("existing.txt", b"replace"), ("new.txt", b"new")],
    );

    let cancelled = AtomicBool::new(false);
    let result = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        None,
        &FailOnConflict,
        |_| {},
    );

    assert_eq!(result.unwrap_err().code, "conflict");
    assert_eq!(fs::read(destination.join("existing.txt")).unwrap(), b"keep");
    assert!(!destination.join("new.txt").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn non_directory_parent_fails_before_extraction() {
    let root = common::temp_dir("parent-conflict-extract");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("parent"), b"keep").unwrap();
    common::write_zip(&zip_path, &[("parent/child.txt", b"new")]);

    let cancelled = AtomicBool::new(false);
    let result = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        None,
        &FailOnConflict,
        |_| {},
    );

    assert_eq!(result.unwrap_err().code, "invalid_entry");
    assert_eq!(fs::read(destination.join("parent")).unwrap(), b"keep");
    assert!(!destination.join("parent").join("child.txt").exists());

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_when_zip_lists_directory_then_nested_files() {
    let root = common::temp_dir("dir-then-files");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    // Game-asset style: explicit "assets/" + files under it (and a nested dir record).
    common::write_zip_with_dirs(
        &zip_path,
        &["assets/", "assets/tex/"],
        &[
            ("assets/mesh.obj", b"o mesh\n"),
            ("assets/tex/diff.png", b"png"),
        ],
    );

    let cancelled = AtomicBool::new(false);
    let summary = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();

    assert!(summary.extracted_files >= 2);
    assert_eq!(
        fs::read(destination.join("assets").join("mesh.obj")).unwrap(),
        b"o mesh\n"
    );
    assert_eq!(
        fs::read(destination.join("assets").join("tex").join("diff.png")).unwrap(),
        b"png"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_when_nested_files_precede_directory_records() {
    let root = common::temp_dir("files-then-dir");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    // Reverse order: files first, then directory record (still common in packs).
    let mut zip = zip::ZipWriter::new(fs::File::create(&zip_path).unwrap());
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    use std::io::Write;
    zip.start_file("models/hero.fbx", options).unwrap();
    zip.write_all(b"fbx").unwrap();
    zip.add_directory("models/", options).unwrap();
    zip.finish().unwrap();

    let cancelled = AtomicBool::new(false);
    let summary = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();

    assert_eq!(summary.extracted_files, 1);
    assert_eq!(
        fs::read(destination.join("models").join("hero.fbx")).unwrap(),
        b"fbx"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_a_file_with_the_secure_write_path() {
    let root = common::temp_dir("secure-extract");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    common::write_zip(&zip_path, &[("folder/file.txt", b"content")]);

    let cancelled = AtomicBool::new(false);
    let summary = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();

    assert_eq!(summary.extracted_files, 1);
    assert_eq!(
        fs::read(destination.join("folder").join("file.txt")).unwrap(),
        b"content"
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_only_selected_file() {
    let root = common::temp_dir("selected-file");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    common::write_zip(&zip_path, &[("keep.txt", b"keep"), ("skip.txt", b"skip")]);
    let selected = vec!["keep.txt".to_string()];
    let cancelled = AtomicBool::new(false);
    let summary = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        Some(&selected),
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(summary.extracted_files, 1);
    assert_eq!(fs::read(destination.join("keep.txt")).unwrap(), b"keep");
    assert!(!destination.join("skip.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extracts_selected_directory_recursively() {
    let root = common::temp_dir("select-dir");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    common::write_zip(
        &zip_path,
        &[
            ("docs/a.txt", b"a"),
            ("docs/sub/b.txt", b"b"),
            ("other/c.txt", b"c"),
        ],
    );
    let selected = vec!["docs".to_string()];
    let cancelled = AtomicBool::new(false);
    let summary = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        Some(&selected),
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(summary.extracted_files, 2);
    assert_eq!(
        fs::read(destination.join("docs").join("a.txt")).unwrap(),
        b"a"
    );
    assert_eq!(
        fs::read(destination.join("docs").join("sub").join("b.txt")).unwrap(),
        b"b"
    );
    assert!(!destination.join("other").join("c.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_selection_vector_errors() {
    let root = common::temp_dir("empty-sel");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    common::write_zip(&zip_path, &[("a.txt", b"a")]);
    let selected: Vec<String> = vec![];
    let cancelled = AtomicBool::new(false);
    let err = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        Some(&selected),
        &FailOnConflict,
        |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code, "empty_selection");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn unknown_selection_errors() {
    let root = common::temp_dir("bad-sel");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    common::write_zip(&zip_path, &[("a.txt", b"a")]);
    let selected = vec!["nope.txt".to_string()];
    let cancelled = AtomicBool::new(false);
    let err = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        Some(&selected),
        &FailOnConflict,
        |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code, "invalid_selection");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn overwrite_replaces_existing_file() {
    let root = common::temp_dir("overwrite-extract");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("file.txt"), b"keep").unwrap();
    common::write_zip(&zip_path, &[("file.txt", b"replace")]);

    let resolver = ScriptedConflictResolver::new([ConflictDecision::Overwrite]);
    let cancelled = AtomicBool::new(false);
    let summary = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        None,
        &resolver,
        |_| {},
    )
    .unwrap();

    assert_eq!(summary.extracted_files, 1);
    assert_eq!(summary.skipped_files, 0);
    assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"replace");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn skip_keeps_existing_and_extracts_other() {
    let root = common::temp_dir("skip-extract");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("existing.txt"), b"keep").unwrap();
    common::write_zip(
        &zip_path,
        &[("existing.txt", b"replace"), ("new.txt", b"new")],
    );

    let resolver = ScriptedConflictResolver::new([ConflictDecision::Skip]);
    let cancelled = AtomicBool::new(false);
    let summary = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        None,
        &resolver,
        |_| {},
    )
    .unwrap();

    assert_eq!(summary.extracted_files, 1);
    assert_eq!(summary.skipped_files, 1);
    assert_eq!(fs::read(destination.join("existing.txt")).unwrap(), b"keep");
    assert_eq!(fs::read(destination.join("new.txt")).unwrap(), b"new");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rename_writes_numbered_file() {
    let root = common::temp_dir("rename-extract");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("file.txt"), b"keep").unwrap();
    common::write_zip(&zip_path, &[("file.txt", b"new")]);

    let resolver = ScriptedConflictResolver::new([ConflictDecision::Rename]);
    let cancelled = AtomicBool::new(false);
    let summary = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        None,
        &resolver,
        |_| {},
    )
    .unwrap();

    assert_eq!(summary.extracted_files, 1);
    assert_eq!(summary.skipped_files, 0);
    assert_eq!(fs::read(destination.join("file.txt")).unwrap(), b"keep");
    assert_eq!(fs::read(destination.join("file (1).txt")).unwrap(), b"new");

    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn symlink_destination_hard_fails_without_overwrite() {
    use std::os::windows::fs::symlink_file;

    let root = common::temp_dir("symlink-dest-extract");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    let target = destination.join("target.txt");
    let link = destination.join("file.txt");
    fs::write(&target, b"target").unwrap();
    match symlink_file(&target, &link) {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(1314) => {
            // Privilege not held — skip when symlinks cannot be created.
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(error) => panic!("cannot create test symlink: {error}"),
    }
    common::write_zip(&zip_path, &[("file.txt", b"replace")]);

    let cancelled = AtomicBool::new(false);
    // Even Overwrite must not replace a reparse destination; fail closed first.
    let resolver = ScriptedConflictResolver::new([ConflictDecision::Overwrite]);
    let err = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        None,
        &resolver,
        |_| {},
    )
    .unwrap_err();

    assert_eq!(err.code, "unsafe_destination");
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(fs::read(&target).unwrap(), b"target");
    assert!(!destination.read_dir().unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .contains("archi-part")));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn apply_policy_simulated_by_two_skips_in_script() {
    let root = common::temp_dir("two-skips-extract");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("a.txt"), b"keep-a").unwrap();
    fs::write(destination.join("b.txt"), b"keep-b").unwrap();
    common::write_zip(
        &zip_path,
        &[
            ("a.txt", b"new-a"),
            ("b.txt", b"new-b"),
            ("c.txt", b"new-c"),
        ],
    );

    let resolver = ScriptedConflictResolver::new([ConflictDecision::Skip, ConflictDecision::Skip]);
    let cancelled = AtomicBool::new(false);
    let summary = extract_archive(
        &zip_path,
        &destination,
        "extract-1",
        &cancelled,
        None,
        &resolver,
        |_| {},
    )
    .unwrap();

    assert_eq!(summary.extracted_files, 1);
    assert_eq!(summary.skipped_files, 2);
    assert_eq!(fs::read(destination.join("a.txt")).unwrap(), b"keep-a");
    assert_eq!(fs::read(destination.join("b.txt")).unwrap(), b"keep-b");
    assert_eq!(fs::read(destination.join("c.txt")).unwrap(), b"new-c");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancel_from_conflict_cleans_partials() {
    // First entry extracts cleanly; second hits a conflict and Cancel is chosen.
    // Cleanup must remove created files and leave no .archi-part leftovers.
    let root = common::temp_dir("cancel-from-conflict");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("second.txt"), b"existing").unwrap();
    common::write_zip(
        &zip_path,
        &[("first.txt", b"new-first"), ("second.txt", b"new-second")],
    );

    let resolver = ScriptedConflictResolver::new([ConflictDecision::Cancel]);
    let cancelled = AtomicBool::new(false);
    let err = extract_archive(
        &zip_path,
        &destination,
        "extract-cancel-conflict",
        &cancelled,
        None,
        &resolver,
        |_| {},
    )
    .unwrap_err();

    assert_eq!(err.code, "cancelled");
    // Pre-existing destination must remain; extraction-created content must go.
    assert_eq!(
        fs::read(destination.join("second.txt")).unwrap(),
        b"existing"
    );
    assert!(!destination.join("first.txt").exists());
    assert!(fs::read_dir(&destination).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("archi-part")
    }));
    // Error message should not report cleanup failures.
    assert!(
        !err.message.to_lowercase().contains("cleanup failed"),
        "unexpected cleanup failure in message: {}",
        err.message
    );

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn apply_to_all_skip_via_registry_policy() {
    // Simulate production: first conflict resolved with Skip+apply_to_all stores policy;
    // subsequent conflicts read the policy without another waiter round-trip.
    use archi_backend_lib::extraction::ConflictResolver;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    struct RegistryPolicyResolver {
        registry: OperationRegistry,
    }

    impl ConflictResolver for RegistryPolicyResolver {
        fn resolve_file_exists(
            &self,
            operation_id: &str,
            _entry_path: &str,
            _dest_path: &std::path::Path,
        ) -> Result<ConflictDecision, archi_backend_lib::models::CommandError> {
            if let Some(policy) = self.registry.peek_apply_policy(operation_id) {
                return Ok(policy);
            }
            let conflict_id = format!("{operation_id}-conflict");
            self.registry
                .install_conflict_waiter(operation_id, &conflict_id)
                .map_err(|m| archi_backend_lib::models::CommandError::new("operation_failed", m))?;
            self.registry
                .recv_conflict_decision(operation_id, &conflict_id)
                .map_err(|m| archi_backend_lib::models::CommandError::new("operation_failed", m))
        }
    }

    let root = common::temp_dir("apply-all-registry");
    let zip_path = root.join("input.zip");
    let destination = root.join("output");
    fs::create_dir_all(&destination).unwrap();
    fs::write(destination.join("a.txt"), b"keep-a").unwrap();
    fs::write(destination.join("b.txt"), b"keep-b").unwrap();
    common::write_zip(
        &zip_path,
        &[
            ("a.txt", b"new-a"),
            ("b.txt", b"new-b"),
            ("c.txt", b"new-c"),
        ],
    );

    let registry = Arc::new(OperationRegistry::default());
    let op_id = "extract-apply-all".to_string();
    let state = registry.start(&op_id).unwrap();
    let cancelled = state.cancelled.clone();

    let reg_resolve = registry.clone();
    let op_resolve = op_id.clone();
    let resolver_thread = thread::spawn(move || {
        // Wait until extract installs a waiter, then resolve Skip+apply_to_all.
        for _ in 0..200 {
            if reg_resolve
                .resolve_conflict(
                    &op_resolve,
                    &format!("{op_resolve}-conflict"),
                    ConflictDecision::Skip,
                    true,
                )
                .is_ok()
            {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting to resolve first conflict");
    });

    let resolver = RegistryPolicyResolver {
        registry: (*registry).clone(),
    };
    let summary = extract_archive(
        &zip_path,
        &destination,
        &op_id,
        &cancelled,
        None,
        &resolver,
        |_| {},
    )
    .unwrap();
    resolver_thread.join().unwrap();
    registry.finish(&op_id);

    assert_eq!(summary.extracted_files, 1);
    assert_eq!(summary.skipped_files, 2);
    assert_eq!(fs::read(destination.join("a.txt")).unwrap(), b"keep-a");
    assert_eq!(fs::read(destination.join("b.txt")).unwrap(), b"keep-b");
    assert_eq!(fs::read(destination.join("c.txt")).unwrap(), b"new-c");

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn normalize_entry_name_uses_forward_slashes() {
    assert_eq!(normalize_entry_name(r"folder\file.txt"), "folder/file.txt");
    assert_eq!(normalize_entry_name("folder/file.txt"), "folder/file.txt");
    assert_eq!(normalize_entry_name("folder/"), "folder");
}

#[test]
fn selection_includes_exact_and_prefix() {
    let selected = vec!["docs".into(), "readme.txt".into()];
    assert!(selection_includes_entry("readme.txt", &selected));
    assert!(selection_includes_entry("docs/a.txt", &selected));
    assert!(selection_includes_entry("docs/sub/b.txt", &selected));
    assert!(!selection_includes_entry("other/x.txt", &selected));
    assert!(!selection_includes_entry("docs-extra/x.txt", &selected));
}

#[test]
fn validate_selection_rejects_unknown_roots() {
    let names = vec!["docs/a.txt".into(), "readme.txt".into()];
    assert!(validate_selection(&["readme.txt".into()], &names).is_ok());
    assert!(validate_selection(&["docs".into()], &names).is_ok());
    let err = validate_selection(&["missing".into()], &names).unwrap_err();
    assert_eq!(err.code, "invalid_selection");
}
