mod common;

use archi_backend_lib::archive::open_archive;
use archi_backend_lib::extraction::{extract_any, FailOnConflict};
use archi_backend_lib::format_detect::{detect_format, ArchiveFormat};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::{self, File};
use std::io::Write;
use std::sync::atomic::AtomicBool;
use tar::Builder;

fn write_plain_tar(path: &std::path::Path, name: &str, data: &[u8]) {
    write_plain_tar_entries(path, &[(name, data)]);
}

fn write_plain_tar_entries(path: &std::path::Path, entries: &[(&str, &[u8])]) {
    let file = File::create(path).unwrap();
    let mut builder = Builder::new(file);
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, *data).unwrap();
    }
    builder.into_inner().unwrap();
}

fn write_tar_gz(path: &std::path::Path, name: &str, data: &[u8]) {
    let file = File::create(path).unwrap();
    let enc = GzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(enc);
    let mut header = tar::Header::new_gnu();
    header.set_path(name).unwrap();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append(&header, data).unwrap();
    builder.into_inner().unwrap().finish().unwrap();
}

#[test]
fn open_lists_tar_nested() {
    let root = common::temp_dir("open-tar");
    let tar_path = root.join("n.tar");
    write_plain_tar(&tar_path, "dir/a.txt", b"payload");

    let info = open_archive(&tar_path).unwrap();
    assert_eq!(info.format, "tar");
    assert!(info.capabilities.extract);
    assert!(!info.capabilities.create);
    assert!(info.capabilities.test);
    assert!(info.capabilities.edit);
    assert!(info.entries.iter().any(|e| e.path == "dir"));
    assert!(info
        .entries
        .iter()
        .any(|e| e.path == "dir/a.txt" && !e.is_directory));

    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_tar_writes_file() {
    let root = common::temp_dir("extract-tar");
    let tar_path = root.join("n.tar");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();
    write_plain_tar(&tar_path, "hello.txt", b"hello-tar");

    let summary = extract_any(
        &tar_path,
        &dest,
        "tar-ex-1",
        &AtomicBool::new(false),
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(summary.extracted_files, 1);
    assert_eq!(fs::read(dest.join("hello.txt")).unwrap(), b"hello-tar");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_tar_selected_file_only() {
    let root = common::temp_dir("extract-tar-sel-file");
    let tar_path = root.join("n.tar");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();
    write_plain_tar_entries(&tar_path, &[("keep.txt", b"keep"), ("skip.txt", b"skip")]);
    let selected = vec!["keep.txt".to_string()];
    let summary = extract_any(
        &tar_path,
        &dest,
        "tar-sel-file",
        &AtomicBool::new(false),
        Some(&selected),
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(summary.extracted_files, 1);
    assert_eq!(fs::read(dest.join("keep.txt")).unwrap(), b"keep");
    assert!(!dest.join("skip.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_tar_selected_directory_recursively() {
    let root = common::temp_dir("extract-tar-sel-dir");
    let tar_path = root.join("n.tar");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();
    write_plain_tar_entries(
        &tar_path,
        &[
            ("docs/a.txt", b"a"),
            ("docs/sub/b.txt", b"b"),
            ("other/c.txt", b"c"),
        ],
    );
    let selected = vec!["docs".to_string()];
    let summary = extract_any(
        &tar_path,
        &dest,
        "tar-sel-dir",
        &AtomicBool::new(false),
        Some(&selected),
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(summary.extracted_files, 2);
    assert_eq!(fs::read(dest.join("docs").join("a.txt")).unwrap(), b"a");
    assert_eq!(
        fs::read(dest.join("docs").join("sub").join("b.txt")).unwrap(),
        b"b"
    );
    assert!(!dest.join("other").join("c.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_tar_invalid_selection_writes_nothing() {
    // Mixed valid + missing selection must fail *before* writing keep.txt.
    let root = common::temp_dir("extract-tar-sel-invalid");
    let tar_path = root.join("n.tar");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();
    write_plain_tar_entries(&tar_path, &[("keep.txt", b"keep"), ("skip.txt", b"skip")]);
    let selected = vec!["keep.txt".to_string(), "missing.txt".to_string()];
    let err = extract_any(
        &tar_path,
        &dest,
        "tar-sel-invalid",
        &AtomicBool::new(false),
        Some(&selected),
        &FailOnConflict,
        |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code, "invalid_selection");
    assert!(!dest.join("keep.txt").exists());
    assert!(!dest.join("skip.txt").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_tar_gz_writes_file() {
    let root = common::temp_dir("extract-tgz");
    let path = root.join("n.tar.gz");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();
    write_tar_gz(&path, "x.bin", b"gzipped-tar");

    assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::TarGz);
    extract_any(
        &path,
        &dest,
        "tgz-1",
        &AtomicBool::new(false),
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("x.bin")).unwrap(), b"gzipped-tar");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_single_gzip() {
    let root = common::temp_dir("extract-gz");
    let path = root.join("notes.txt.gz");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();
    {
        let file = File::create(&path).unwrap();
        let mut enc = GzEncoder::new(file, Compression::default());
        enc.write_all(b"just gzip").unwrap();
        enc.finish().unwrap();
    }
    assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::Gzip);
    let info = open_archive(&path).unwrap();
    assert_eq!(info.format, "gzip");
    assert_eq!(info.entries.len(), 1);
    assert_eq!(info.entries[0].name, "notes.txt");

    extract_any(
        &path,
        &dest,
        "gz-1",
        &AtomicBool::new(false),
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("notes.txt")).unwrap(), b"just gzip");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn open_gzip_size_matches_isize_payload() {
    // Open uses gzip ISIZE trailer, not full inflate; small fixture size must match.
    let root = common::temp_dir("open-gz-isize");
    let path = root.join("payload.bin.gz");
    let payload = b"hello gzip isize size check!!";
    {
        let file = File::create(&path).unwrap();
        let mut enc = GzEncoder::new(file, Compression::default());
        enc.write_all(payload).unwrap();
        enc.finish().unwrap();
    }

    let info = open_archive(&path).unwrap();
    assert_eq!(info.format, "gzip");
    assert_eq!(info.entries.len(), 1);
    assert_eq!(
        info.entries[0].uncompressed_size,
        payload.len() as u64,
        "open size must equal ISIZE (payload length for small files)"
    );
    assert_eq!(info.stats.total_uncompressed, payload.len() as u64);
    assert_eq!(
        info.entries[0].compressed_size,
        Some(fs::metadata(&path).unwrap().len())
    );

    fs::remove_dir_all(root).unwrap();
}

fn write_tar_bz2(path: &std::path::Path, name: &str, data: &[u8]) {
    use bzip2::write::BzEncoder;
    use bzip2::Compression;
    let file = File::create(path).unwrap();
    let enc = BzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(enc);
    let mut header = tar::Header::new_gnu();
    header.set_path(name).unwrap();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append(&header, data).unwrap();
    builder.into_inner().unwrap().finish().unwrap();
}

#[test]
fn extract_tar_bz2_writes_file() {
    let root = common::temp_dir("extract-tbz");
    let path = root.join("n.tar.bz2");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();
    write_tar_bz2(&path, "y.bin", b"bzipped-tar");

    assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::TarBz2);
    let info = open_archive(&path).unwrap();
    assert_eq!(info.format, "tar.bz2");
    assert!(!info.capabilities.create);

    extract_any(
        &path,
        &dest,
        "tbz-1",
        &AtomicBool::new(false),
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("y.bin")).unwrap(), b"bzipped-tar");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_single_bzip2() {
    use bzip2::write::BzEncoder;
    use bzip2::Compression;

    let root = common::temp_dir("extract-bz2");
    let path = root.join("notes.txt.bz2");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();
    {
        let file = File::create(&path).unwrap();
        let mut enc = BzEncoder::new(file, Compression::default());
        enc.write_all(b"just bzip2").unwrap();
        enc.finish().unwrap();
    }
    assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::Bzip2);
    let info = open_archive(&path).unwrap();
    assert_eq!(info.format, "bzip2");
    assert_eq!(info.entries.len(), 1);
    assert_eq!(info.entries[0].name, "notes.txt");

    extract_any(
        &path,
        &dest,
        "bz2-1",
        &AtomicBool::new(false),
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("notes.txt")).unwrap(), b"just bzip2");
    fs::remove_dir_all(root).unwrap();
}

fn write_tar_xz(path: &std::path::Path, name: &str, data: &[u8]) {
    use xz2::write::XzEncoder;
    let file = File::create(path).unwrap();
    let enc = XzEncoder::new(file, 6);
    let mut builder = Builder::new(enc);
    let mut header = tar::Header::new_gnu();
    header.set_path(name).unwrap();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append(&header, data).unwrap();
    builder.into_inner().unwrap().finish().unwrap();
}

#[test]
fn extract_tar_xz_writes_file() {
    let root = common::temp_dir("extract-txz");
    let path = root.join("n.tar.xz");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();
    write_tar_xz(&path, "y.bin", b"xzipped-tar");

    assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::TarXz);
    let info = open_archive(&path).unwrap();
    assert_eq!(info.format, "tar.xz");
    assert!(!info.capabilities.create);

    extract_any(
        &path,
        &dest,
        "txz-1",
        &AtomicBool::new(false),
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("y.bin")).unwrap(), b"xzipped-tar");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn extract_single_xz() {
    use xz2::write::XzEncoder;

    let root = common::temp_dir("extract-xz");
    let path = root.join("notes.txt.xz");
    let dest = root.join("out");
    fs::create_dir(&dest).unwrap();
    {
        let file = File::create(&path).unwrap();
        let mut enc = XzEncoder::new(file, 6);
        enc.write_all(b"just xz").unwrap();
        enc.finish().unwrap();
    }
    assert_eq!(detect_format(&path).unwrap(), ArchiveFormat::Xz);
    let info = open_archive(&path).unwrap();
    assert_eq!(info.format, "xz");
    assert_eq!(info.entries.len(), 1);
    assert_eq!(info.entries[0].name, "notes.txt");

    extract_any(
        &path,
        &dest,
        "xz-1",
        &AtomicBool::new(false),
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();
    assert_eq!(fs::read(dest.join("notes.txt")).unwrap(), b"just xz");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn zip_open_still_works() {
    let root = common::temp_dir("zip-still");
    let zip_path = root.join("a.zip");
    {
        use zip::write::FileOptions;
        use zip::ZipWriter;
        let file = File::create(&zip_path).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.start_file("z.txt", FileOptions::default()).unwrap();
        zip.write_all(b"zip").unwrap();
        zip.finish().unwrap();
    }
    let info = open_archive(&zip_path).unwrap();
    assert_eq!(info.format, "zip");
    assert!(info.capabilities.create);
    assert!(info.capabilities.test);
    fs::remove_dir_all(root).unwrap();
}
