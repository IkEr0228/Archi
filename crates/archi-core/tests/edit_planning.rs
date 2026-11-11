use archi_core::{
    archive, archive_edit, extraction,
    models::{CreateOptions, EditOptions},
    sevenz_format, tar_create, zipper,
};
use std::{fs, path::PathBuf, sync::atomic::AtomicBool};

struct Lab(PathBuf);
impl Lab {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("archi-edit-plan-{}-{nonce}", std::process::id()));
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
fn sevenz_unsafe_raw_names_are_not_sanitized_by_listing_extraction_or_edit() {
    let lab = Lab::new();
    let cancelled = AtomicBool::new(false);
    for name in [
        "/absolute",
        "\\absolute",
        "C:/drive",
        "../escape",
        "nested/../../escape",
        "./relative",
    ] {
        let path = lab.0.join("unsafe.7z");
        let mut writer =
            sevenz_rust2::ArchiveWriter::new(fs::File::create(&path).unwrap()).unwrap();
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file(name),
                Some(std::io::Cursor::new(b"unsafe")),
            )
            .unwrap();
        writer.finish().unwrap();
        let original = fs::read(&path).unwrap();
        assert_eq!(
            archive::open_archive(&path, None).unwrap_err().code,
            "invalid_entry",
            "{name}"
        );
        let out = lab.0.join("out");
        fs::create_dir(&out).unwrap();
        assert!(
            extraction::extract_any(
                &path,
                &out,
                "unsafe",
                &cancelled,
                None,
                None,
                &extraction::FailOnConflict,
                |_| {}
            )
            .is_err(),
            "{name}"
        );
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
        assert_eq!(
            archive_edit::create_folder(
                &path,
                "new",
                "unsafe",
                &cancelled,
                |_| {},
                None,
                &EditOptions::default()
            )
            .unwrap_err()
            .code,
            "invalid_entry"
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::remove_dir(out).unwrap();
    }
}

#[test]
fn shared_add_planner_preserves_empty_dirs_and_rejects_collisions_and_unsafe_parents() {
    let lab = Lab::new();
    let source = lab.0.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("kept"), b"original").unwrap();
    let additions = lab.0.join("additions");
    fs::create_dir_all(additions.join("empty")).unwrap();
    fs::write(additions.join("new"), b"added").unwrap();
    let paths = vec![additions.to_string_lossy().into_owned()];
    let cancelled = AtomicBool::new(false);
    for format in ["zip", "tar", "7z"] {
        let path = lab.0.join(format!("archive.{format}"));
        let sources = vec![source.to_string_lossy().into_owned()];
        let options = CreateOptions {
            include_root: false,
            ..if format == "7z" {
                CreateOptions::default_7z()
            } else {
                CreateOptions::default_zip()
            }
        };
        match format {
            "zip" => {
                zipper::create_zip_archive(&sources, &path, "create", &cancelled, &options, |_| {})
                    .unwrap();
            }
            "tar" => {
                let options = CreateOptions {
                    format: archi_core::models::CreateFormat::Tar,
                    ..options
                };
                tar_create::create_tar_archive(
                    &sources,
                    &path,
                    "create",
                    &cancelled,
                    &options,
                    |_| {},
                )
                .unwrap();
            }
            _ => {
                sevenz_format::create_sevenz_archive(
                    &sources,
                    &path,
                    "create",
                    &cancelled,
                    sevenz_rust2::Password::empty(),
                    &options,
                    |_| {},
                )
                .unwrap();
            }
        }
        let edit = EditOptions::default();
        let before = fs::read(&path).unwrap();
        for parent in ["../escape", "/absolute", "C:/drive", "kept/sub"] {
            assert!(
                archive_edit::add_paths(
                    &path,
                    &paths,
                    parent,
                    "add",
                    &cancelled,
                    |_| {},
                    None,
                    &edit
                )
                .is_err(),
                "{format}: {parent}"
            );
            assert_eq!(fs::read(&path).unwrap(), before);
        }
        let duplicate = vec![paths[0].clone(), paths[0].clone()];
        assert_eq!(
            archive_edit::add_paths(
                &path,
                &duplicate,
                "",
                "add",
                &cancelled,
                |_| {},
                None,
                &edit
            )
            .unwrap_err()
            .code,
            "entry_exists"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        archive_edit::add_paths(
            &path,
            &paths,
            "folder",
            "add",
            &cancelled,
            |_| {},
            None,
            &edit,
        )
        .unwrap();
        let info = archive::open_archive(&path, None).unwrap();
        assert!(info
            .entries
            .iter()
            .any(|e| e.path == "folder/additions/empty" && e.is_directory));
        let output = lab.0.join(format!("out-{format}"));
        fs::create_dir(&output).unwrap();
        extraction::extract_any(
            &path,
            &output,
            "extract",
            &cancelled,
            None,
            None,
            &extraction::FailOnConflict,
            |_| {},
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join("folder/additions/new")).unwrap(),
            b"added"
        );
        let before = fs::read(&path).unwrap();
        assert_eq!(
            archive_edit::add_paths(
                &path,
                &paths,
                "folder",
                "add",
                &cancelled,
                |_| {},
                None,
                &edit
            )
            .unwrap_err()
            .code,
            "entry_exists"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(!fs::read_dir(&lab.0).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".archi-part-")));
    }
}
