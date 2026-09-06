use archi_backend_lib::archive::open_archive;
use archi_backend_lib::cli_handler::{
    execute_cli_add_7z, execute_cli_add_zip, parse_cli_action, unique_archive_path, CliAction,
};
use archi_backend_lib::context_menu::{
    get_context_menu_status, register_files_context_menu, unregister_files_context_menu,
};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_test_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("archi-test-files-{name}-{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn test_cli_add_zip_file() {
    let dir = temp_test_dir("zip-file");
    let test_file = dir.join("document.txt");
    fs::write(&test_file, "Sample text content for 1-click zip").unwrap();

    let out_zip = execute_cli_add_zip(&test_file).unwrap();
    assert_eq!(out_zip, dir.join("document.zip"));
    assert!(out_zip.is_file());

    let info = open_archive(&out_zip, None).unwrap();
    assert_eq!(info.format, "zip");
    assert_eq!(info.entries.len(), 1);
    assert_eq!(info.entries[0].name, "document.txt");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_cli_add_zip_directory() {
    let dir = temp_test_dir("zip-dir");
    let sub = dir.join("project_folder");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("main.rs"), "fn main() {}").unwrap();
    fs::write(sub.join("data.json"), "{}").unwrap();

    let out_zip = execute_cli_add_zip(&sub).unwrap();
    assert_eq!(out_zip, dir.join("project_folder.zip"));
    assert!(out_zip.is_file());

    let info = open_archive(&out_zip, None).unwrap();
    assert_eq!(info.format, "zip");
    assert!(info.entries.len() >= 2);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_cli_add_7z_file() {
    let dir = temp_test_dir("7z-file");
    let test_file = dir.join("notes.txt");
    fs::write(&test_file, "Important notes for 1-click 7z archive").unwrap();

    let out_7z = execute_cli_add_7z(&test_file).unwrap();
    assert_eq!(out_7z, dir.join("notes.7z"));
    assert!(out_7z.is_file());

    let info = open_archive(&out_7z, None).unwrap();
    assert_eq!(info.format, "7z");
    assert_eq!(info.entries.len(), 1);
    assert_eq!(info.entries[0].name, "notes.txt");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_cli_add_7z_directory() {
    let dir = temp_test_dir("7z-dir");
    let sub = dir.join("asset_dir");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("texture.png"), "fake png data").unwrap();

    let out_7z = execute_cli_add_7z(&sub).unwrap();
    assert_eq!(out_7z, dir.join("asset_dir.7z"));
    assert!(out_7z.is_file());

    let info = open_archive(&out_7z, None).unwrap();
    assert_eq!(info.format, "7z");
    assert!(info.entries.iter().any(|e| e.name.contains("texture.png")));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_unique_archive_path_avoids_collision() {
    let dir = temp_test_dir("collision");
    let base_zip = dir.join("data.zip");
    fs::write(&base_zip, "existing").unwrap();

    let unique = unique_archive_path(&base_zip);
    assert_eq!(unique, dir.join("data (1).zip"));

    fs::write(&unique, "existing 1").unwrap();
    let unique2 = unique_archive_path(&base_zip);
    assert_eq!(unique2, dir.join("data (2).zip"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_cli_parse_file_actions() {
    let cwd = PathBuf::from(r"C:\work");

    let create_args = vec![
        "archi.exe".into(),
        "--create".into(),
        r"C:\work\photo.jpg".into(),
    ];
    assert_eq!(
        parse_cli_action(&create_args, &cwd),
        CliAction::Create(vec![PathBuf::from(r"C:\work\photo.jpg")])
    );

    let zip_args = vec![
        "archi.exe".into(),
        "--add-zip".into(),
        r"C:\work\photo.jpg".into(),
    ];
    assert_eq!(
        parse_cli_action(&zip_args, &cwd),
        CliAction::AddZip(PathBuf::from(r"C:\work\photo.jpg"))
    );

    let sevenz_args = vec![
        "archi.exe".into(),
        "--add-7z".into(),
        r"C:\work\photo.jpg".into(),
    ];
    assert_eq!(
        parse_cli_action(&sevenz_args, &cwd),
        CliAction::Add7z(PathBuf::from(r"C:\work\photo.jpg"))
    );
}

#[cfg(windows)]
#[test]
fn test_files_context_menu_registration_roundtrip() {
    let status = register_files_context_menu().unwrap();
    assert!(status.supported);
    assert!(status.files_menu_enabled);

    let current = get_context_menu_status();
    assert!(current.files_menu_enabled);

    let unreg = unregister_files_context_menu().unwrap();
    assert!(!unreg.files_menu_enabled);

    let after = get_context_menu_status();
    assert!(!after.files_menu_enabled);
}
