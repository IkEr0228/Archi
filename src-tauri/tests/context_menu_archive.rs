use archi_backend_lib::cli_handler::{
    archive_stem, execute_cli_extraction, parse_cli_action, CliAction, CliExtractTarget,
};
use archi_backend_lib::context_menu::{
    get_context_menu_status, register_archive_context_menu, unregister_archive_context_menu,
};
use archi_backend_lib::models::CreateOptions;
use archi_backend_lib::zipper::create_zip_archive;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_test_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("archi-test-{name}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn create_sample_zip(dir: &Path, filename: &str) -> PathBuf {
    let staging = dir.join("staging");
    fs::create_dir_all(&staging).unwrap();
    let source_file1 = staging.join("hello.txt");
    let source_file2 = staging.join("sub.dat");
    fs::write(&source_file1, "Hello world from archive!").unwrap();
    fs::write(&source_file2, "Binary test content 12345").unwrap();

    let zip_path = dir.join(filename);
    let sources = vec![
        source_file1.to_string_lossy().into_owned(),
        source_file2.to_string_lossy().into_owned(),
    ];

    let cancelled = AtomicBool::new(false);
    create_zip_archive(
        &sources,
        &zip_path,
        "test-op",
        &cancelled,
        &CreateOptions::default_zip(),
        |_| {},
    )
    .unwrap();

    let _ = fs::remove_dir_all(&staging);
    zip_path
}

#[test]
fn test_cli_extract_here() {
    let dir = temp_test_dir("extract-here");
    let zip_path = create_sample_zip(&dir, "sample.zip");

    let dest = execute_cli_extraction(&zip_path, CliExtractTarget::Here).unwrap();
    assert_eq!(dest, dir);
    assert!(dir.join("hello.txt").exists());
    assert!(dir.join("sub.dat").exists());
    assert_eq!(
        fs::read_to_string(dir.join("hello.txt")).unwrap(),
        "Hello world from archive!"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_cli_extract_to_subfolder() {
    let dir = temp_test_dir("extract-to");
    let zip_path = create_sample_zip(&dir, "my_package.zip");

    let dest = execute_cli_extraction(&zip_path, CliExtractTarget::ToSubfolder).unwrap();
    let expected_subfolder = dir.join("my_package");
    assert_eq!(dest, expected_subfolder);
    assert!(expected_subfolder.exists());
    assert!(expected_subfolder.join("hello.txt").exists());
    assert!(expected_subfolder.join("sub.dat").exists());
    assert_eq!(
        fs::read_to_string(expected_subfolder.join("hello.txt")).unwrap(),
        "Hello world from archive!"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_cli_parse_actions() {
    let cwd = PathBuf::from(r"C:\test");
    let args_here = vec![
        "archi.exe".into(),
        "--extract-here".into(),
        r"C:\test\sample.zip".into(),
    ];
    assert_eq!(
        parse_cli_action(&args_here, &cwd),
        CliAction::ExtractHere(PathBuf::from(r"C:\test\sample.zip"))
    );

    let args_to = vec![
        "archi.exe".into(),
        "--extract-to".into(),
        r"C:\test\sample.zip".into(),
    ];
    assert_eq!(
        parse_cli_action(&args_to, &cwd),
        CliAction::ExtractTo(PathBuf::from(r"C:\test\sample.zip"))
    );
}

#[test]
fn test_archive_stem_stripping() {
    assert_eq!(archive_stem(Path::new("archive.zip")), "archive");
    assert_eq!(archive_stem(Path::new("project.tar.gz")), "project");
    assert_eq!(archive_stem(Path::new("system.tar.bz2")), "system");
    assert_eq!(archive_stem(Path::new("data.tar.xz")), "data");
    assert_eq!(archive_stem(Path::new("backup.7z")), "backup");
    assert_eq!(archive_stem(Path::new("release.rar")), "release");
}

#[cfg(windows)]
#[test]
fn test_context_menu_registration_roundtrip() {
    let status = register_archive_context_menu().unwrap();
    assert!(status.supported);
    assert!(status.archive_menu_enabled);

    let current = get_context_menu_status();
    assert!(current.archive_menu_enabled);

    let unreg = unregister_archive_context_menu().unwrap();
    assert!(!unreg.archive_menu_enabled);

    let after = get_context_menu_status();
    assert!(!after.archive_menu_enabled);
}
