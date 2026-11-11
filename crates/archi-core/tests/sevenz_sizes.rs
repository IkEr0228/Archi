use archi_core::extraction::{extract_any, FailOnConflict};
use archi_core::models::CreateOptions;
use sevenz_rust2::Password;
use std::fs;
use std::sync::atomic::AtomicBool;

#[test]
fn mixed_encoder_sizes_and_empty_encrypted_archive_round_trip() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("archi-sevenz-sizes-{}-{nonce}", std::process::id()));
    let source = root.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("small.txt"), b"small file").unwrap();
    let large = vec![b'x'; 3 * 1024 * 1024];
    fs::write(source.join("large.txt"), &large).unwrap();
    let cancelled = AtomicBool::new(false);
    let options = CreateOptions {
        include_root: false,
        ..CreateOptions::default_7z()
    };
    for password in ["", "  secret  "] {
        let archive = root.join("mixed.7z");
        archi_core::sevenz_format::create_sevenz_archive(
            &[source.to_string_lossy().into_owned()],
            &archive,
            "create",
            &cancelled,
            Password::new(password),
            &options,
            |_| {},
        )
        .unwrap();
        let output = root.join("out");
        fs::create_dir(&output).unwrap();
        extract_any(
            &archive,
            &output,
            "extract",
            &cancelled,
            None,
            Some(password.into()),
            &FailOnConflict,
            |_| {},
        )
        .unwrap();
        assert_eq!(fs::read(output.join("small.txt")).unwrap(), b"small file");
        assert_eq!(fs::read(output.join("large.txt")).unwrap(), large);
        fs::remove_dir_all(output).unwrap();
        fs::remove_file(archive).unwrap();
    }
    let empty = root.join("empty");
    fs::create_dir(&empty).unwrap();
    let archive = root.join("empty.7z");
    archi_core::sevenz_format::create_sevenz_archive(
        &[empty.to_string_lossy().into_owned()],
        &archive,
        "empty",
        &cancelled,
        Password::new("secret"),
        &options,
        |_| {},
    )
    .unwrap();
    assert!(archi_core::archive::open_archive(&archive, Some("secret".into())).is_ok());
    fs::remove_dir_all(root).unwrap();
}
