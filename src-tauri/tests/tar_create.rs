mod common;

use archi_backend_lib::archive::open_archive;
use archi_backend_lib::extraction::{extract_any, FailOnConflict};
use archi_backend_lib::format_detect::{detect_format, ArchiveFormat};
use archi_backend_lib::models::{CompressionPreset, CreateFormat, CreateOptions};
use archi_backend_lib::tar_create::create_tar_archive;
use std::fs;
use std::sync::atomic::AtomicBool;

fn options(format: CreateFormat, compression: CompressionPreset) -> CreateOptions {
    CreateOptions {
        format,
        compression,
        include_root: true,
        overwrite: false,
    }
}

fn write_source_tree(root: &std::path::Path) -> std::path::PathBuf {
    let src = root.join("pack");
    fs::create_dir_all(src.join("nested")).unwrap();
    fs::write(src.join("a.txt"), b"hello-tar-create").unwrap();
    fs::write(src.join("nested").join("b.bin"), b"nested-bytes").unwrap();
    src
}

fn round_trip(format: CreateFormat, expected_detect: ArchiveFormat, ext: &str) {
    let root = common::temp_dir(&format!("create-{}", format.as_str().replace('.', "-")));
    let src = write_source_tree(&root);
    let out = root.join(format!("out.{ext}"));
    let dest = root.join("extract");
    fs::create_dir(&dest).unwrap();

    create_tar_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "create-1",
        &AtomicBool::new(false),
        &options(format, CompressionPreset::Normal),
        |_| {},
    )
    .unwrap();

    assert_eq!(detect_format(&out).unwrap(), expected_detect);
    let info = open_archive(&out).unwrap();
    assert_eq!(info.format, format.as_str());
    assert!(!info.capabilities.create);

    extract_any(
        &out,
        &dest,
        "ex-1",
        &AtomicBool::new(false),
        None,
        &FailOnConflict,
        |_| {},
    )
    .unwrap();

    assert_eq!(
        fs::read(dest.join("pack").join("a.txt")).unwrap(),
        b"hello-tar-create"
    );
    assert_eq!(
        fs::read(dest.join("pack").join("nested").join("b.bin")).unwrap(),
        b"nested-bytes"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn create_tar_round_trip() {
    round_trip(CreateFormat::Tar, ArchiveFormat::Tar, "tar");
}

#[test]
fn create_tar_gz_round_trip() {
    round_trip(CreateFormat::TarGz, ArchiveFormat::TarGz, "tar.gz");
}

#[test]
fn create_tar_bz2_round_trip() {
    round_trip(CreateFormat::TarBz2, ArchiveFormat::TarBz2, "tar.bz2");
}

#[test]
fn create_tar_xz_round_trip() {
    round_trip(CreateFormat::TarXz, ArchiveFormat::TarXz, "tar.xz");
}

#[test]
fn create_tar_rejects_output_inside_source() {
    let root = common::temp_dir("create-contain");
    let src = write_source_tree(&root);
    let out = src.join("evil.tar.xz");
    let err = create_tar_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "create-bad",
        &AtomicBool::new(false),
        &options(CreateFormat::TarXz, CompressionPreset::Fast),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(err.code, "output_inside_source");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn create_tar_xz_shrinks_and_reports_ratio() {
    let root = common::temp_dir("create-ratio");
    let src = root.join("data");
    fs::create_dir_all(&src).unwrap();
    // Highly compressible payload (~1 MiB of repeated text).
    fs::write(src.join("big.txt"), "RATIO".repeat(200_000).as_bytes()).unwrap();
    let out = root.join("packed.tar.xz");
    create_tar_archive(
        &[src.to_string_lossy().into_owned()],
        &out,
        "ratio-1",
        &AtomicBool::new(false),
        &options(CreateFormat::TarXz, CompressionPreset::Max),
        |_| {},
    )
    .unwrap();

    let on_disk = fs::metadata(&out).unwrap().len();
    let info = open_archive(&out).unwrap();
    assert_eq!(info.stats.total_compressed, on_disk);
    assert!(
        info.stats.total_compressed < info.stats.total_uncompressed,
        "expected stream compression: packed {} vs uncomp {}",
        info.stats.total_compressed,
        info.stats.total_uncompressed
    );
    let file = info
        .entries
        .iter()
        .find(|e| !e.is_directory)
        .expect("file entry");
    assert!(
        file.compressed_size.is_some(),
        "per-entry compressed size must be estimated for UI ratio"
    );
    assert!(file.compressed_size.unwrap() < file.uncompressed_size);
    fs::remove_dir_all(root).unwrap();
}
