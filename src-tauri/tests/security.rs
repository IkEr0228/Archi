use archi_backend_lib::security::{
    assess_archive, safe_destination_path, validate_entry_path, ArchiveRiskInput,
};

#[test]
fn rejects_unsafe_windows_archive_paths() {
    for path in [
        "../evil",
        "/rooted",
        "C:/evil",
        r"C:\evil",
        r"\\server\share\evil",
        "safe/../../evil",
        "safe:stream",
    ] {
        assert!(validate_entry_path(path).is_err(), "accepted {path}");
    }
    assert_eq!(
        validate_entry_path("folder/РїСЂРёРјРµСЂ.txt")
            .unwrap()
            .to_string_lossy(),
        "folder\\РїСЂРёРјРµСЂ.txt"
    );
}

#[test]
fn rejects_windows_device_names_and_trailing_forms() {
    for path in [
        "CON",
        "con.txt",
        "PRN.log",
        "AUX ",
        "NUL.",
        "CLOCK$.bak",
        "COM1",
        "com9.txt",
        "COM\u{00B9}",
        "com\u{00B2}.txt",
        "COM\u{00B3}",
        "LPT1",
        "lpt9.log",
        "LPT\u{00B9}",
        "LPT\u{00B2}",
        "LPT\u{00B3}",
        "lpt\u{00B9}.log",
        "COM1 .txt",
        "folder /file.txt",
        "folder/file. ",
    ] {
        assert!(validate_entry_path(path).is_err(), "accepted {path}");
    }
}

#[test]
fn resolves_valid_entry_within_destination() {
    let root = std::env::temp_dir().join(format!("archi-security-test-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();

    let resolved = safe_destination_path(&root, "folder/file.txt").unwrap();

    assert_eq!(
        resolved,
        root.canonicalize().unwrap().join("folder/file.txt")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn rejects_dangling_symbolic_link_in_destination() {
    use std::os::windows::fs::symlink_dir;

    let root =
        std::env::temp_dir().join(format!("archi-dangling-link-test-{}", std::process::id()));
    let link = root.join("dangling");
    std::fs::create_dir_all(&root).unwrap();

    match symlink_dir(root.join("missing-target"), &link) {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(1314) => {
            std::fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(error) => panic!("failed to create test symlink: {error}"),
    }

    let result = safe_destination_path(&root, "dangling/file.txt");
    std::fs::remove_dir_all(root).unwrap();

    assert!(result.is_err(), "accepted a dangling symbolic link");
}

#[test]
fn warns_without_rejecting_suspicious_metadata() {
    let warnings = assess_archive(ArchiveRiskInput {
        entry_count: 1_000_001,
        total_uncompressed: 1_100_000_000,
        total_compressed: 1_000_000,
        largest_entry: 500_000_000,
        deepest_path: 140,
    });
    assert!(warnings.iter().any(|w| w.code == "entry_count"));
    assert!(warnings.iter().any(|w| w.code == "expansion_ratio"));
    assert!(warnings.iter().any(|w| w.code == "path_depth"));
}

#[test]
fn warns_only_when_expansion_ratio_exceeds_1000_to_1() {
    const ONE_GIB: u64 = 1024 * 1024 * 1024;
    const COMPRESSED: u64 = 1_073_742;

    for total_uncompressed in [ONE_GIB - 1, ONE_GIB] {
        let warnings = assess_archive(ArchiveRiskInput {
            entry_count: 0,
            total_uncompressed,
            total_compressed: 1,
            largest_entry: 0,
            deepest_path: 0,
        });
        assert!(!warnings
            .iter()
            .any(|warning| warning.code == "expansion_ratio"));
    }

    let exact_ratio = assess_archive(ArchiveRiskInput {
        entry_count: 0,
        total_uncompressed: COMPRESSED * 1_000,
        total_compressed: COMPRESSED,
        largest_entry: 0,
        deepest_path: 0,
    });
    assert!(!exact_ratio
        .iter()
        .any(|warning| warning.code == "expansion_ratio"));

    let above_ratio = assess_archive(ArchiveRiskInput {
        entry_count: 0,
        total_uncompressed: COMPRESSED * 1_000 + 1,
        total_compressed: COMPRESSED,
        largest_entry: 0,
        deepest_path: 0,
    });
    assert!(above_ratio
        .iter()
        .any(|warning| warning.code == "expansion_ratio"));

    let large_values = assess_archive(ArchiveRiskInput {
        entry_count: 0,
        total_uncompressed: u64::MAX,
        total_compressed: u64::MAX / 1_000,
        largest_entry: 0,
        deepest_path: 0,
    });
    assert!(large_values
        .iter()
        .any(|warning| warning.code == "expansion_ratio"));
}
