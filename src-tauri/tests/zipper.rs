mod common;

use archi_backend_lib::extraction::{extract_archive, FailOnConflict};
use archi_backend_lib::models::{CompressionPreset, CreateOptions};
use archi_backend_lib::zipper::create_zip_archive;
use std::cell::Cell;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use zip::CompressionMethod;
use zip::ZipArchive;

fn temporary_archives(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".archi-part-")
        })
        .collect()
}

fn options(compression: CompressionPreset, include_root: bool, overwrite: bool) -> CreateOptions {
    CreateOptions {
        format: archi_backend_lib::models::CreateFormat::Zip,
        compression,
        include_root,
        overwrite,
    }
}

#[cfg(windows)]
fn create_junction(target: &Path, junction: &Path) {
    use std::process::{Command, Stdio};

    let status = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(junction)
        .arg(target)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

#[test]
fn rejects_output_inside_source_directory() {
    let source = common::temp_dir("self-output");
    fs::write(source.join("input.txt"), b"content").unwrap();
    let error = create_zip_archive(
        &[source.to_string_lossy().into_owned()],
        &source.join("out.zip"),
        "create-1",
        &AtomicBool::new(false),
        &CreateOptions::default_zip(),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(error.code, "output_inside_source");
    assert!(!source.join("out.zip").exists());
    fs::remove_dir_all(source).unwrap();
}

#[test]
fn cancellation_removes_temporary_archive() {
    let root = common::temp_dir("cancel-create");
    let input_path = root.join("input.txt");
    let second_path = root.join("second.txt");
    let output = root.join("output.zip");
    fs::write(&input_path, b"content").unwrap();
    fs::write(&second_path, b"second").unwrap();
    let inputs = [
        input_path.to_string_lossy().into_owned(),
        second_path.to_string_lossy().into_owned(),
    ];
    let cancelled = AtomicBool::new(false);
    let progress_calls = Cell::new(0);
    let temporary_size = Cell::new(None);
    let error = create_zip_archive(
        &inputs,
        &output,
        "create-2",
        &cancelled,
        &CreateOptions::default_zip(),
        |_| {
            let call = progress_calls.get() + 1;
            progress_calls.set(call);
            if call == 1 {
                assert!(!temporary_archives(&root).is_empty());
                // Exceed PROGRESS_INTERVAL so the next entry still emits progress.
                std::thread::sleep(std::time::Duration::from_millis(120));
            } else if call == 2 {
                let temporary = temporary_archives(&root);
                temporary_size.set(Some(fs::metadata(&temporary[0]).unwrap().len()));
                cancelled.store(true, Ordering::Relaxed);
            }
        },
    )
    .unwrap_err();
    let temporary_size = temporary_size.get();
    let temporary_archives = temporary_archives(&root);
    fs::remove_dir_all(&root).unwrap();

    assert_eq!(error.code, "cancelled");
    assert!(!output.exists());
    assert!(temporary_size.is_some_and(|size| size > 0));
    assert!(temporary_archives.is_empty());
}

#[test]
fn preserves_destination_created_after_temporary_archive() {
    let root = common::temp_dir("publish-race");
    let input_path = root.join("input.txt");
    let output = root.join("output.zip");
    fs::write(&input_path, b"content").unwrap();
    let saw_temporary = Cell::new(false);
    let result = create_zip_archive(
        &[input_path.to_string_lossy().into_owned()],
        &output,
        "create-race",
        &AtomicBool::new(false),
        &CreateOptions::default_zip(),
        |_| {
            if !saw_temporary.get() {
                saw_temporary.set(!temporary_archives(&root).is_empty());
                fs::write(&output, b"keep").unwrap();
            }
        },
    );
    let preserved = fs::read(&output).unwrap();
    let temporary_archives = temporary_archives(&root);
    fs::remove_dir_all(&root).unwrap();

    assert!(saw_temporary.get());
    assert_eq!(result.unwrap_err().code, "output_exists");
    assert_eq!(preserved, b"keep");
    assert!(temporary_archives.is_empty());
}

#[test]
fn renames_windows_namespace_collisions() {
    let root = common::temp_dir("namespace-collisions");
    let first_root = root.join("first");
    let second_root = root.join("second");
    let third_root = root.join("third");
    let fourth_root = root.join("fourth");
    fs::create_dir_all(&first_root).unwrap();
    fs::create_dir_all(&second_root).unwrap();
    fs::create_dir_all(third_root.join("foo")).unwrap();
    fs::create_dir_all(fourth_root.join("folder")).unwrap();
    let upper = first_root.join("Foo");
    let lower = second_root.join("foo");
    let directory = third_root.join("foo");
    let non_empty_directory = fourth_root.join("folder");
    fs::write(&upper, b"upper").unwrap();
    fs::write(&lower, b"lower").unwrap();
    fs::write(non_empty_directory.join("child.txt"), b"child").unwrap();
    let output = root.join("output.zip");
    let sources = [&upper, &lower, &directory, &non_empty_directory]
        .map(|path| path.to_string_lossy().into_owned());

    create_zip_archive(
        &sources,
        &output,
        "create-collisions",
        &AtomicBool::new(false),
        &CreateOptions::default_zip(),
        |_| {},
    )
    .unwrap();
    let mut archive = ZipArchive::new(File::open(&output).unwrap()).unwrap();
    let names: Vec<_> = (0..archive.len())
        .map(|index| archive.by_index(index).unwrap().name().to_string())
        .collect();
    drop(archive);

    let destination = root.join("extracted");
    fs::create_dir(&destination).unwrap();
    let extraction = extract_archive(
        &output,
        &destination,
        "extract-collisions",
        &AtomicBool::new(false),
        None,
        &FailOnConflict,
        |_| {},
    );
    let extracted = extraction.as_ref().ok().map(|_| {
        [
            fs::read(destination.join("Foo")).unwrap(),
            fs::read(destination.join("foo(1)")).unwrap(),
            fs::read(destination.join("folder").join("child.txt")).unwrap(),
        ]
    });
    let empty_directory_exists = destination.join("foo(2)").is_dir();
    fs::remove_dir_all(&root).unwrap();

    assert_eq!(names, ["Foo", "foo(1)", "foo(2)/", "folder/child.txt"]);
    assert!(extraction.is_ok(), "{extraction:?}");
    assert!(empty_directory_exists);
    assert_eq!(
        extracted,
        Some([b"upper".to_vec(), b"lower".to_vec(), b"child".to_vec()])
    );
}

#[cfg(windows)]
#[test]
fn rejects_source_path_through_junction_ancestor() {
    let root = common::temp_dir("source-junction-ancestor");
    let outside = root.join("outside");
    let linked = root.join("linked");
    let selected = linked.join("secret.txt");
    let output = root.join("output.zip");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), b"secret").unwrap();
    create_junction(&outside, &linked);

    let result = create_zip_archive(
        &[selected.to_string_lossy().into_owned()],
        &output,
        "create-source-junction-ancestor",
        &AtomicBool::new(false),
        &CreateOptions::default_zip(),
        |_| {},
    );
    fs::remove_dir(&linked).unwrap();
    fs::remove_dir_all(&root).unwrap();

    assert!(result.is_err());
}

#[cfg(windows)]
#[test]
fn rejects_top_level_and_nested_junction_sources() {
    let root = common::temp_dir("source-junctions");
    let outside = root.join("outside");
    let source = root.join("source");
    let top_level = root.join("top-level-link");
    let nested = source.join("nested-link");
    fs::create_dir_all(&outside).unwrap();
    fs::create_dir_all(&source).unwrap();
    fs::write(outside.join("secret.txt"), b"secret").unwrap();
    create_junction(&outside, &top_level);
    create_junction(&outside, &nested);

    let top_level_result = create_zip_archive(
        &[top_level.to_string_lossy().into_owned()],
        &root.join("top-level.zip"),
        "create-top-level-link",
        &AtomicBool::new(false),
        &CreateOptions::default_zip(),
        |_| {},
    );
    let nested_result = create_zip_archive(
        &[source.to_string_lossy().into_owned()],
        &root.join("nested.zip"),
        "create-nested-link",
        &AtomicBool::new(false),
        &CreateOptions::default_zip(),
        |_| {},
    );
    fs::remove_dir(&top_level).unwrap();
    fs::remove_dir(&nested).unwrap();
    fs::remove_dir_all(&root).unwrap();

    assert_eq!(top_level_result.unwrap_err().code, "invalid_source");
    assert_eq!(nested_result.unwrap_err().code, "invalid_source");
}

#[cfg(windows)]
#[test]
fn rejects_top_level_source_symlink_when_supported() {
    use std::os::windows::fs::symlink_file;

    let root = common::temp_dir("source-symlink");
    let target = root.join("target.txt");
    let link = root.join("link.txt");
    let output = root.join("output.zip");
    fs::write(&target, b"content").unwrap();
    match symlink_file(&target, &link) {
        Ok(()) => {}
        Err(error) if error.raw_os_error() == Some(1314) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(error) => panic!("cannot create test symlink: {error}"),
    }

    let result = create_zip_archive(
        &[link.to_string_lossy().into_owned()],
        &output,
        "create-source-symlink",
        &AtomicBool::new(false),
        &CreateOptions::default_zip(),
        |_| {},
    );
    fs::remove_file(&link).unwrap();
    fs::remove_dir_all(&root).unwrap();

    assert_eq!(result.unwrap_err().code, "invalid_source");
    assert!(!output.exists());
}

#[cfg(windows)]
#[test]
fn rejects_source_replaced_after_enumeration() {
    let root = common::temp_dir("source-swap");
    let first = root.join("first.txt");
    let source = root.join("source");
    let nested = source.join("nested");
    let outside = root.join("outside");
    let output = root.join("output.zip");
    fs::write(&first, b"first").unwrap();
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("secret.txt"), b"original").unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.txt"), b"secret").unwrap();
    let replaced = Cell::new(false);
    let result = create_zip_archive(
        &[
            first.to_string_lossy().into_owned(),
            source.to_string_lossy().into_owned(),
        ],
        &output,
        "create-source-swap",
        &AtomicBool::new(false),
        &CreateOptions::default_zip(),
        |_| {
            if !replaced.get() {
                fs::remove_dir_all(&nested).unwrap();
                create_junction(&outside, &nested);
                replaced.set(true);
            }
        },
    );
    fs::remove_dir(&nested).unwrap();
    let temporary_archives = temporary_archives(&root);
    fs::remove_dir_all(&root).unwrap();

    assert!(replaced.get());
    assert!(result.is_err());
    assert!(!output.exists());
    assert!(temporary_archives.is_empty());
}

#[test]
fn store_compression_writes_stored_method() {
    let root = common::temp_dir("create-store");
    let input = root.join("blob.bin");
    // compressible payload
    fs::write(&input, vec![b'A'; 4096]).unwrap();
    let output = root.join("out.zip");
    create_zip_archive(
        &[input.to_string_lossy().into_owned()],
        &output,
        "create-store",
        &AtomicBool::new(false),
        &options(CompressionPreset::Store, true, false),
        |_| {},
    )
    .unwrap();
    let mut archive = ZipArchive::new(File::open(&output).unwrap()).unwrap();
    let file = archive.by_name("blob.bin").unwrap();
    assert_eq!(file.compression(), CompressionMethod::Stored);
    drop(file);
    drop(archive);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deflated_normal_writes_deflated_method() {
    let root = common::temp_dir("create-deflate");
    let input = root.join("blob.bin");
    fs::write(&input, vec![b'B'; 4096]).unwrap();
    let output = root.join("out.zip");
    create_zip_archive(
        &[input.to_string_lossy().into_owned()],
        &output,
        "create-deflate",
        &AtomicBool::new(false),
        &options(CompressionPreset::Normal, true, false),
        |_| {},
    )
    .unwrap();
    let mut archive = ZipArchive::new(File::open(&output).unwrap()).unwrap();
    let file = archive.by_name("blob.bin").unwrap();
    assert_eq!(file.compression(), CompressionMethod::Deflated);
    drop(file);
    drop(archive);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn include_root_false_puts_children_at_archive_root() {
    let root = common::temp_dir("include-root-off");
    let folder = root.join("FolderName");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("a.txt"), b"hello").unwrap();
    let output = root.join("out.zip");
    create_zip_archive(
        &[folder.to_string_lossy().into_owned()],
        &output,
        "include-root-off",
        &AtomicBool::new(false),
        &options(CompressionPreset::Normal, false, false),
        |_| {},
    )
    .unwrap();
    let mut archive = ZipArchive::new(File::open(&output).unwrap()).unwrap();
    let names: Vec<_> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    drop(archive);
    fs::remove_dir_all(root).unwrap();
    assert!(names.iter().any(|n| n == "a.txt" || n == "a.txt/"));
    assert!(!names
        .iter()
        .any(|n| n.replace('\\', "/").starts_with("FolderName/")));
}

#[test]
fn include_root_true_keeps_folder_prefix() {
    let root = common::temp_dir("include-root-on");
    let folder = root.join("FolderName");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("a.txt"), b"hello").unwrap();
    let output = root.join("out.zip");
    create_zip_archive(
        &[folder.to_string_lossy().into_owned()],
        &output,
        "include-root-on",
        &AtomicBool::new(false),
        &options(CompressionPreset::Normal, true, false),
        |_| {},
    )
    .unwrap();
    let mut archive = ZipArchive::new(File::open(&output).unwrap()).unwrap();
    let names: Vec<_> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    drop(archive);
    fs::remove_dir_all(root).unwrap();
    assert!(names
        .iter()
        .any(|n| n.replace('\\', "/") == "FolderName/a.txt"));
}

#[test]
fn overwrite_false_preserves_existing_output() {
    let root = common::temp_dir("overwrite-off");
    let input = root.join("a.txt");
    let output = root.join("out.zip");
    fs::write(&input, b"new").unwrap();
    fs::write(&output, b"keep-me").unwrap();
    let err = create_zip_archive(
        &[input.to_string_lossy().into_owned()],
        &output,
        "overwrite-off",
        &AtomicBool::new(false),
        &options(CompressionPreset::Normal, true, false),
        |_| {},
    )
    .unwrap_err();
    let preserved = fs::read(&output).unwrap();
    fs::remove_dir_all(root).unwrap();
    assert_eq!(err.code, "output_exists");
    assert_eq!(preserved, b"keep-me");
}

#[test]
fn overwrite_true_replaces_existing_regular_file() {
    let root = common::temp_dir("overwrite-on");
    let input = root.join("a.txt");
    let output = root.join("out.zip");
    fs::write(&input, b"payload").unwrap();
    fs::write(&output, b"old-zip-bytes").unwrap();
    create_zip_archive(
        &[input.to_string_lossy().into_owned()],
        &output,
        "overwrite-on",
        &AtomicBool::new(false),
        &options(CompressionPreset::Store, true, true),
        |_| {},
    )
    .unwrap();
    let mut archive = ZipArchive::new(File::open(&output).unwrap()).unwrap();
    let mut file = archive.by_name("a.txt").unwrap();
    let mut buf = String::new();
    file.read_to_string(&mut buf).unwrap();
    drop(file);
    drop(archive);
    fs::remove_dir_all(root).unwrap();
    assert_eq!(buf, "payload");
}

#[test]
fn overwrite_true_rejects_directory_output() {
    let root = common::temp_dir("overwrite-dir");
    let input = root.join("a.txt");
    let output_dir = root.join("out.zip"); // name looks like zip but is a dir
    fs::write(&input, b"x").unwrap();
    fs::create_dir(&output_dir).unwrap();
    let err = create_zip_archive(
        &[input.to_string_lossy().into_owned()],
        &output_dir,
        "overwrite-dir",
        &AtomicBool::new(false),
        &options(CompressionPreset::Normal, true, true),
        |_| {},
    )
    .unwrap_err();
    assert!(output_dir.is_dir());
    fs::remove_dir_all(root).unwrap();
    assert_eq!(err.code, "invalid_output");
}
