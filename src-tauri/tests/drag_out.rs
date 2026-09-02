mod common;

use archi_backend_lib::drag_out::{
    cleanup_old_drag_temp_dirs, register_temp_dir_for_cleanup, top_level_paths,
};
#[cfg(windows)]
use archi_backend_lib::drag_out::{create_hdrop_buffer, DROPFILES};
use std::fs;
use std::path::PathBuf;

#[test]
fn test_top_level_paths_selection() {
    let selected = vec![
        "folder".to_string(),
        "folder/a.txt".to_string(),
        "folder/b/c.txt".to_string(),
        "standalone.txt".to_string(),
        "other/file.png".to_string(),
    ];
    let top = top_level_paths(&selected);
    assert_eq!(
        top,
        vec![
            "folder".to_string(),
            "standalone.txt".to_string(),
            "other/file.png".to_string(),
        ]
    );
}

#[test]
fn test_top_level_paths_with_slashes() {
    let selected = vec![
        "/dir/".to_string(),
        "dir/sub/file.txt".to_string(),
        "dir\\sub2\\other.txt".to_string(),
    ];
    let top = top_level_paths(&selected);
    assert_eq!(top, vec!["dir".to_string()]);
}

#[cfg(windows)]
#[test]
fn test_hdrop_buffer_construction() {
    let p1 = PathBuf::from(r"C:\sample\one.txt");
    let p2 = PathBuf::from(r"C:\sample\two.txt");
    let hglobal = create_hdrop_buffer(&[p1.clone(), p2.clone()]).expect("valid hdrop");
    assert!(!hglobal.is_null());

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalLock(hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        fn GlobalUnlock(hMem: *mut std::ffi::c_void) -> i32;
        fn GlobalFree(hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    unsafe {
        let ptr = GlobalLock(hglobal);
        assert!(!ptr.is_null());

        let dropfiles = ptr as *const DROPFILES;
        assert_eq!((*dropfiles).pFiles, std::mem::size_of::<DROPFILES>() as u32);
        assert_eq!((*dropfiles).fWide, 1);
        assert_eq!((*dropfiles).fNC, 0);

        GlobalUnlock(hglobal);
        GlobalFree(hglobal);
    }
}

#[test]
fn test_temp_cleanup_routines() {
    let temp_root = std::env::temp_dir();
    let dummy_drag_dir = temp_root.join("archi-dnd-test-dummy-12345");
    fs::create_dir_all(&dummy_drag_dir).unwrap();
    fs::write(dummy_drag_dir.join("temp.txt"), b"temp content").unwrap();
    assert!(dummy_drag_dir.exists());

    // Register for delayed cleanup
    register_temp_dir_for_cleanup(dummy_drag_dir.clone());

    // Test cleanup old directories
    cleanup_old_drag_temp_dirs();
    assert!(!dummy_drag_dir.exists());
}
