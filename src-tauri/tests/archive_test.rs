mod common;

use archi_backend_lib::testing::test_archive;
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn tests_valid_zip_entries() {
    let root = common::temp_dir("test-ok");
    let zip_path = root.join("ok.zip");
    common::write_zip(
        &zip_path,
        &[("a.txt", b"hello"), ("folder/b.txt", b"world")],
    );

    let cancelled = AtomicBool::new(false);
    let summary = test_archive(&zip_path, "test-1", &cancelled, |_| {}).unwrap();

    assert_eq!(summary.total_entries, 2);
    assert_eq!(summary.tested_ok, 2);
    assert_eq!(summary.tested_failed, 0);
    assert!(summary.failures.is_empty());

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancel_mid_test_returns_cancelled() {
    let root = common::temp_dir("test-cancel");
    let zip_path = root.join("ok.zip");
    // Several small files so cancel can be observed.
    common::write_zip(
        &zip_path,
        &[
            ("a.txt", b"1"),
            ("b.txt", b"2"),
            ("c.txt", b"3"),
            ("d.txt", b"4"),
        ],
    );

    let cancelled = AtomicBool::new(true);
    let error = test_archive(&zip_path, "test-2", &cancelled, |_| {}).unwrap_err();
    assert_eq!(error.code, "cancelled");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancel_flag_can_be_set_during_progress() {
    let root = common::temp_dir("test-cancel-progress");
    let zip_path = root.join("ok.zip");
    let big = vec![7_u8; 256 * 1024];
    common::write_zip(
        &zip_path,
        &[("big1.bin", &big), ("big2.bin", &big), ("big3.bin", &big)],
    );

    let cancelled = AtomicBool::new(false);
    let flag = &cancelled;
    let error = test_archive(&zip_path, "test-3", &cancelled, move |_| {
        flag.store(true, Ordering::Relaxed);
    })
    .unwrap_err();
    assert_eq!(error.code, "cancelled");

    std::fs::remove_dir_all(root).unwrap();
}
