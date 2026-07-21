use archi_backend_lib::conflict::{candidate_renamed_name, split_file_name, unique_renamed_path};
use std::fs;

mod common;

#[test]
fn split_and_candidate_windows_style() {
    assert_eq!(
        split_file_name("report.txt"),
        ("report".into(), ".txt".into())
    );
    assert_eq!(
        split_file_name("report.backup.zip"),
        ("report.backup".into(), ".zip".into())
    );
    assert_eq!(
        split_file_name(".gitignore"),
        (".gitignore".into(), "".into())
    );
    assert_eq!(candidate_renamed_name("report.txt", 1), "report (1).txt");
    assert_eq!(candidate_renamed_name("report.txt", 2), "report (2).txt");
}

#[test]
fn unique_renamed_path_skips_existing() {
    let root = common::temp_dir("rename-unique");
    fs::write(root.join("file.txt"), b"a").unwrap();
    let path = unique_renamed_path(&root, "file.txt").unwrap();
    assert_eq!(path.file_name().unwrap().to_string_lossy(), "file (1).txt");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(windows)]
#[test]
fn unique_renamed_path_skips_reparse_candidates() {
    use std::os::windows::fs::symlink_file;

    let root = common::temp_dir("rename-reparse");
    fs::write(root.join("file.txt"), b"a").unwrap();
    fs::write(root.join("target.txt"), b"t").unwrap();
    match symlink_file(root.join("target.txt"), root.join("file (1).txt")) {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(1314) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(error) => panic!("cannot create test symlink: {error}"),
    }

    let path = unique_renamed_path(&root, "file.txt").unwrap();
    assert_eq!(path.file_name().unwrap().to_string_lossy(), "file (2).txt");
    fs::remove_dir_all(root).unwrap();
}
