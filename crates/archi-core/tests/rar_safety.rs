#![cfg(windows)]
mod common {
    pub mod rar_fixture;
}
use archi_core::{
    archive,
    extraction::{extract_any, FailOnConflict, ScriptedConflictResolver},
    models::ConflictDecision,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

struct Lab(PathBuf);
impl Lab {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("archi-rar-safety-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn nested_rar_crc_failure_preserves_old_file_and_cleans_staging() {
    for corrupt in [false, true] {
        let lab = Lab::new();
        let path = lab.0.join("nested.rar");
        let payload = vec![b'x'; 4 * 1024 * 1024];
        common::rar_fixture::stored(&path, "nested/data", &payload, corrupt);
        let out = lab.0.join("out");
        fs::create_dir_all(out.join("nested")).unwrap();
        fs::write(out.join("nested/data"), b"original").unwrap();
        let result = extract_any(
            &path,
            &out,
            "rar",
            &AtomicBool::new(false),
            None,
            None,
            &ScriptedConflictResolver::new(vec![ConflictDecision::Overwrite]),
            |_| {},
        );
        if corrupt {
            assert_eq!(result.unwrap_err().code, "corrupted_archive");
        } else {
            assert_eq!(result.unwrap().extracted_files, 1);
        }
        assert_eq!(
            fs::read(out.join("nested/data")).unwrap(),
            if corrupt {
                b"original".as_slice()
            } else {
                payload.as_slice()
            }
        );
        assert_eq!(fs::read_dir(out.join("nested")).unwrap().count(), 1);
    }
}

#[test]
fn solid_rar_selective_decode_matches_full_decode() {
    let lab = Lab::new();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/solid.rar");
    let info = archive::open_archive(&path, None).unwrap();
    let files: Vec<_> = info.entries.iter().filter(|e| !e.is_directory).collect();
    // Bundled solid fixture has one member; exercises solid decode, not skipped blocks.
    assert!(!files.is_empty());
    let full = lab.0.join("full");
    let selected = lab.0.join("selected");
    fs::create_dir(&full).unwrap();
    fs::create_dir(&selected).unwrap();
    extract_any(
        &path,
        &full,
        "full",
        &AtomicBool::new(false),
        None,
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    let last = &files.last().unwrap().path;
    let summary = extract_any(
        &path,
        &selected,
        "selected",
        &AtomicBool::new(false),
        Some(&[last.clone()]),
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(summary.extracted_files, 1);
    assert_eq!(
        fs::read(selected.join(last)).unwrap(),
        fs::read(full.join(last)).unwrap()
    );
}

#[test]
fn rar_traversal_names_are_rejected_before_writing() {
    let lab = Lab::new();
    let path = lab.0.join("unsafe.rar");
    let out = lab.0.join("out");
    fs::create_dir(&out).unwrap();
    for name in ["../escape", "/absolute", "nested/../../escape"] {
        common::rar_fixture::stored(&path, name, b"unsafe", false);
        assert!(
            extract_any(
                &path,
                &out,
                "unsafe",
                &AtomicBool::new(false),
                None,
                None,
                &FailOnConflict,
                |_| {}
            )
            .is_err(),
            "{name}"
        );
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
        assert!(!lab.0.join("escape").exists());
    }
}
