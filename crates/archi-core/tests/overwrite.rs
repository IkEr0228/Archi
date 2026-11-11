use archi_core::extraction::{
    extract_any, extract_transactional, ConflictResolver, ScriptedConflictResolver,
};
use archi_core::models::{CommandError, ConflictDecision};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

struct Lab(PathBuf);
impl Lab {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "archi-overwrite-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn fixture(path: &Path, format: &str) {
    let payload = vec![b'x'; 1024 * 1024];
    let file = fs::File::create(path).unwrap();
    match format {
        "gz" => {
            let mut w = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            w.write_all(&payload).unwrap();
            w.finish().unwrap();
        }
        "bz2" => {
            let mut w = bzip2::write::BzEncoder::new(file, bzip2::Compression::default());
            w.write_all(&payload).unwrap();
            w.finish().unwrap();
        }
        "xz" => {
            let mut w = xz2::write::XzEncoder::new(file, 6);
            w.write_all(&payload).unwrap();
            w.finish().unwrap();
        }
        "zip" => {
            let mut w = zip::ZipWriter::new(file);
            w.start_file(
                "data",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
            w.write_all(&payload).unwrap();
            w.finish().unwrap();
        }
        "7z" => {
            let mut writer = sevenz_rust2::ArchiveWriter::new(file).unwrap();
            writer
                .push_archive_entry(
                    sevenz_rust2::ArchiveEntry::new_file("data"),
                    Some(std::io::Cursor::new(payload)),
                )
                .unwrap();
            writer.finish().unwrap();
        }
        "tar.gz" | "tar.bz2" | "tar.xz" => {
            let mut tar = tar::Builder::new(Vec::new());
            let mut header = tar::Header::new_gnu();
            header.set_size(payload.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, "data", payload.as_slice())
                .unwrap();
            let bytes = tar.into_inner().unwrap();
            match format {
                "tar.gz" => {
                    let mut w = flate2::write::GzEncoder::new(file, flate2::Compression::default());
                    w.write_all(&bytes).unwrap();
                    w.finish().unwrap();
                }
                "tar.bz2" => {
                    let mut w = bzip2::write::BzEncoder::new(file, bzip2::Compression::default());
                    w.write_all(&bytes).unwrap();
                    w.finish().unwrap();
                }
                _ => {
                    let mut w = xz2::write::XzEncoder::new(file, 6);
                    w.write_all(&bytes).unwrap();
                    w.finish().unwrap();
                }
            }
        }
        _ => unreachable!(),
    }
}

fn assert_only_original(destination: &Path) {
    assert_eq!(fs::read(destination.join("data")).unwrap(), b"original");
    assert_eq!(
        fs::read_dir(destination).unwrap().count(),
        1,
        "partial file leaked"
    );
}

#[test]
fn corrupt_streams_and_zip_preserve_existing_bytes_and_clean_temp() {
    for format in ["gz", "bz2", "xz", "zip", "7z"] {
        let lab = Lab::new();
        let path = lab.0.join(format!("data.{format}"));
        fixture(&path, format);
        let mut bytes = fs::read(&path).unwrap();
        // Corrupt payload/checksum, retaining enough structure for format detection.
        let offset = match format {
            "zip" => 100,
            "7z" => 36,
            "gz" => bytes.len() - 8,
            _ => bytes.len() - 9,
        };
        bytes[offset] ^= 0x80;
        fs::write(&path, bytes).unwrap();
        let destination = lab.0.join("out");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("data"), b"original").unwrap();
        let resolver = ScriptedConflictResolver::new(vec![ConflictDecision::Overwrite]);
        // Bypass format sniffing: verify the writer's cleanup, not detector rejection.
        let cancelled = AtomicBool::new(false);
        let result = match format {
            "gz" => archi_core::gzip_format::extract_gzip(
                &path,
                &destination,
                "overwrite",
                &cancelled,
                None,
                &resolver,
                |_| {},
            ),
            "bz2" => archi_core::bzip2_format::extract_bzip2(
                &path,
                &destination,
                "overwrite",
                &cancelled,
                None,
                &resolver,
                |_| {},
            ),
            "xz" => archi_core::xz_format::extract_xz(
                &path,
                &destination,
                "overwrite",
                &cancelled,
                None,
                &resolver,
                |_| {},
            ),
            _ => extract_any(
                &path,
                &destination,
                "overwrite",
                &cancelled,
                None,
                None,
                &resolver,
                |_| {},
            ),
        };
        assert!(result.is_err(), "{format}: corruption accepted");
        assert_only_original(&destination);
    }
}

struct CancelOnOverwrite<'a>(&'a AtomicBool);
impl ConflictResolver for CancelOnOverwrite<'_> {
    fn resolve_file_exists(
        &self,
        _: &str,
        _: &str,
        _: &Path,
    ) -> Result<ConflictDecision, CommandError> {
        self.0.store(true, Ordering::Relaxed);
        Ok(ConflictDecision::Overwrite)
    }
}

#[test]
fn cancellation_after_overwrite_decision_preserves_existing_bytes() {
    for format in ["gz", "bz2", "xz", "zip", "7z"] {
        let lab = Lab::new();
        let path = lab.0.join(format!("data.{format}"));
        fixture(&path, format);
        let destination = lab.0.join("out");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("data"), b"original").unwrap();
        let cancelled = AtomicBool::new(false);
        let error = extract_any(
            &path,
            &destination,
            "overwrite",
            &cancelled,
            None,
            None,
            &CancelOnOverwrite(&cancelled),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(error.code, "cancelled", "{format}");
        assert_only_original(&destination);
    }
}

#[test]
fn successful_overwrite_and_rename_keep_expected_bytes() {
    for format in [
        "gz", "bz2", "xz", "zip", "7z", "tar.gz", "tar.bz2", "tar.xz",
    ] {
        for decision in [
            ConflictDecision::Overwrite,
            ConflictDecision::Rename,
            ConflictDecision::Skip,
        ] {
            let lab = Lab::new();
            let path = lab.0.join(format!("data.{format}"));
            fixture(&path, format);
            let destination = lab.0.join("out");
            fs::create_dir(&destination).unwrap();
            fs::write(destination.join("data"), b"original").unwrap();
            let summary = extract_any(
                &path,
                &destination,
                "overwrite",
                &AtomicBool::new(false),
                None,
                None,
                &ScriptedConflictResolver::new(vec![decision]),
                |_| {},
            )
            .unwrap();
            if decision == ConflictDecision::Skip {
                assert_eq!(summary.skipped_files, 1);
                assert_only_original(&destination);
            } else {
                let target = if decision == ConflictDecision::Rename {
                    assert_eq!(fs::read(destination.join("data")).unwrap(), b"original");
                    fs::read_dir(&destination)
                        .unwrap()
                        .map(|e| e.unwrap().path())
                        .find(|p| p.file_name().unwrap() != "data")
                        .unwrap()
                } else {
                    destination.join("data")
                };
                assert_eq!(fs::read(target).unwrap(), vec![b'x'; 1024 * 1024]);
                assert!(!fs::read_dir(&destination).unwrap().any(|e| e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains("archi-part")));
            }
        }
    }
}

#[test]
fn transactional_extract_restores_prior_replacement_after_tar_crc_failure() {
    let lab = Lab::new();
    let path = lab.0.join("broken.tar.gz");
    let destination = lab.0.join("out");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join("data"), b"original").unwrap();
    fs::write(destination.join("keep"), b"keep").unwrap();

    let mut tar_bytes = Vec::new();
    {
        let mut tar = tar::Builder::new(&mut tar_bytes);
        for (name, bytes) in [
            ("data", b"replacement".as_slice()),
            ("created", b"new".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, name, bytes).unwrap();
        }
        tar.finish().unwrap();
    }
    let file = fs::File::create(&path).unwrap();
    let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    encoder.write_all(&tar_bytes).unwrap();
    encoder.finish().unwrap();
    let mut bytes = fs::read(&path).unwrap();
    let footer = bytes.len() - 1;
    bytes[footer] ^= 0x80;
    fs::write(&path, bytes).unwrap();

    let cancelled = AtomicBool::new(false);
    let error = extract_transactional(
        &path,
        &destination,
        "transaction",
        &cancelled,
        None,
        None,
        &ScriptedConflictResolver::new([ConflictDecision::Overwrite]),
        |_| {},
    )
    .unwrap_err();

    assert!(error.code == "invalid_archive" || error.code == "transaction_failed");
    assert_eq!(fs::read(destination.join("data")).unwrap(), b"original");
    assert_eq!(fs::read(destination.join("keep")).unwrap(), b"keep");
    assert!(!destination.join("created").exists());
}

#[test]
fn encrypted_zip_and_sevenz_wrong_password_preserve_overwrite_target() {
    for format in ["zip", "7z"] {
        let lab = Lab::new();
        let source = lab.0.join("data");
        fs::write(&source, vec![b'x'; 1024 * 1024]).unwrap();
        let path = lab.0.join(format!("encrypted.{format}"));
        let options = archi_core::models::CreateOptions {
            password: Some("correct".into()),
            ..if format == "7z" {
                archi_core::models::CreateOptions::default_7z()
            } else {
                archi_core::models::CreateOptions::default_zip()
            }
        };
        let sources = vec![source.to_string_lossy().into_owned()];
        let cancelled = AtomicBool::new(false);
        if format == "zip" {
            archi_core::zipper::create_zip_archive(
                &sources,
                &path,
                "create",
                &cancelled,
                &options,
                |_| {},
            )
            .unwrap();
        } else {
            archi_core::sevenz_format::create_sevenz_archive(
                &sources,
                &path,
                "create",
                &cancelled,
                sevenz_rust2::Password::new("correct"),
                &options,
                |_| {},
            )
            .unwrap();
        }
        let destination = lab.0.join("out");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("data"), b"original").unwrap();
        assert!(extract_any(
            &path,
            &destination,
            "extract",
            &cancelled,
            None,
            Some("wrong".into()),
            &ScriptedConflictResolver::new(vec![ConflictDecision::Overwrite]),
            |_| {}
        )
        .is_err());
        assert_only_original(&destination);
        extract_any(
            &path,
            &destination,
            "extract",
            &cancelled,
            None,
            Some("correct".into()),
            &ScriptedConflictResolver::new(vec![ConflictDecision::Overwrite]),
            |_| {},
        )
        .unwrap();
        assert_eq!(
            fs::read(destination.join("data")).unwrap(),
            vec![b'x'; 1024 * 1024]
        );
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
    }
}

#[test]
fn late_tar_codec_errors_do_not_publish_replacements() {
    for format in ["tar.gz", "tar.bz2", "tar.xz"] {
        let lab = Lab::new();
        let path = lab.0.join(format!("data.{format}"));
        fixture(&path, format);
        let mut bytes = fs::read(&path).unwrap();
        let offset = bytes.len() - if format == "tar.gz" { 8 } else { 9 };
        bytes[offset] ^= 0x80;
        fs::write(&path, bytes).unwrap();
        let destination = lab.0.join("out");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("data"), b"original").unwrap();
        let resolver = ScriptedConflictResolver::new(vec![ConflictDecision::Overwrite]);
        let cancelled = AtomicBool::new(false);
        let result = match format {
            "tar.gz" => archi_core::tar_format::extract_tar_gz(
                &path,
                &destination,
                "overwrite",
                &cancelled,
                None,
                &resolver,
                |_| {},
            ),
            "tar.bz2" => archi_core::tar_format::extract_tar_bz2(
                &path,
                &destination,
                "overwrite",
                &cancelled,
                None,
                &resolver,
                |_| {},
            ),
            _ => archi_core::tar_format::extract_tar_xz(
                &path,
                &destination,
                "overwrite",
                &cancelled,
                None,
                &resolver,
                |_| {},
            ),
        };
        assert!(result.is_err(), "{format}: corrupted trailer accepted");
        assert_only_original(&destination);
    }
}

#[test]
fn rar_wrong_password_preserves_existing_file_and_removes_staging() {
    let lab = Lab::new();
    let destination = lab.0.join("out");
    fs::create_dir(&destination).unwrap();
    fs::write(destination.join(".gitignore"), b"original").unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/crypted.rar");
    let result = extract_any(
        &path,
        &destination,
        "overwrite",
        &AtomicBool::new(false),
        None,
        Some("incorrect".into()),
        &ScriptedConflictResolver::new(vec![ConflictDecision::Overwrite]),
        |_| {},
    );
    assert!(result.is_err());
    assert_eq!(
        fs::read(destination.join(".gitignore")).unwrap(),
        b"original"
    );
    assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
}

#[test]
fn rar_overwrite_success_and_cancel_preserve_expected_file() {
    for cancel in [false, true] {
        let lab = Lab::new();
        let destination = lab.0.join("out");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("VERSION"), b"original").unwrap();
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/version.rar");
        let cancelled = AtomicBool::new(false);
        let resolver = ScriptedConflictResolver::new(vec![ConflictDecision::Overwrite]);
        let cancel_resolver = CancelOnOverwrite(&cancelled);
        let result = extract_any(
            &path,
            &destination,
            "overwrite",
            &cancelled,
            None,
            None,
            if cancel { &cancel_resolver } else { &resolver },
            |_| {},
        );
        if cancel {
            assert_eq!(result.unwrap_err().code, "cancelled");
        } else {
            result.unwrap();
        }
        assert_eq!(
            fs::read(destination.join("VERSION")).unwrap(),
            if cancel {
                b"original".as_slice()
            } else {
                b"unrar-0.4.0".as_slice()
            }
        );
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
    }
}
