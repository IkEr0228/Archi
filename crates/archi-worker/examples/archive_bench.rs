//! Deterministic warm-cache archive benchmark; correctness checked outside timed sections.
use archi_core::extraction::{extract_any, FailOnConflict};
use archi_core::models::{CompressionPreset, CreateFormat, CreateOptions};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

struct Lab(PathBuf);
impl Drop for Lab {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn verify(source: &Path, output: &Path) {
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        assert_eq!(
            fs::read(entry.path()).unwrap(),
            fs::read(output.join(entry.file_name())).unwrap()
        );
    }
    assert_eq!(
        fs::read_dir(source).unwrap().count(),
        fs::read_dir(output).unwrap().count()
    );
}

fn main() {
    let runs = std::env::args()
        .nth(1)
        .map(|s| s.parse::<usize>().unwrap())
        .unwrap_or(5);
    assert!((3..=50).contains(&runs), "Use 3–50 measured runs");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let lab = Lab(std::env::temp_dir().join(format!("archi-bench-{}-{nonce}", std::process::id())));
    fs::create_dir(&lab.0).unwrap();
    let mut results = Vec::new();
    for corpus in ["small-text", "large-text", "large-random"] {
        let source = lab.0.join(corpus);
        fs::create_dir(&source).unwrap();
        let (files, per_file) = if corpus == "small-text" {
            (1000, 1024)
        } else {
            (1, 8 * 1024 * 1024)
        };
        let mut random = 0x4d595df4d0f33173_u64;
        for index in 0..files {
            let data: Vec<u8> = (0..per_file)
                .map(|offset| {
                    if corpus == "large-random" {
                        random ^= random << 13;
                        random ^= random >> 7;
                        random ^= random << 17;
                        random as u8
                    } else {
                        b"Archi deterministic archive benchmark\n"[(offset + index) % 38]
                    }
                })
                .collect();
            fs::write(source.join(format!("file-{index:04}.txt")), data).unwrap();
        }
        for format in [CreateFormat::Zip, CreateFormat::TarGz, CreateFormat::SevenZ] {
            let archive = lab
                .0
                .join(format!("archive.{}", format.preferred_extension()));
            let output = lab.0.join("out");
            let options = CreateOptions {
                format,
                compression: CompressionPreset::Normal,
                include_root: false,
                overwrite: false,
                password: None,
            };
            let sources = vec![source.to_string_lossy().into_owned()];
            let cancelled = AtomicBool::new(false);
            let mut create = Vec::new();
            let mut extract = Vec::new();
            let mut archive_bytes = 0;
            for iteration in 0..=runs {
                fs::create_dir(&output).unwrap();
                let started = Instant::now();
                match format {
                    CreateFormat::Zip => archi_core::zipper::create_zip_archive(
                        &sources,
                        &archive,
                        "bench",
                        &cancelled,
                        &options,
                        |_| {},
                    ),
                    CreateFormat::SevenZ => archi_core::sevenz_format::create_sevenz_archive(
                        &sources,
                        &archive,
                        "bench",
                        &cancelled,
                        sevenz_rust2::Password::empty(),
                        &options,
                        |_| {},
                    ),
                    _ => archi_core::tar_create::create_tar_archive(
                        &sources,
                        &archive,
                        "bench",
                        &cancelled,
                        &options,
                        |_| {},
                    ),
                }
                .unwrap();
                let create_ms = started.elapsed().as_secs_f64() * 1000.0;
                let started = Instant::now();
                extract_any(
                    &archive,
                    &output,
                    "bench",
                    &cancelled,
                    None,
                    None,
                    &FailOnConflict,
                    |_| {},
                )
                .unwrap();
                let extract_ms = started.elapsed().as_secs_f64() * 1000.0;
                verify(&source, &output);
                archive_bytes = fs::metadata(&archive).unwrap().len();
                if iteration > 0 {
                    create.push(create_ms);
                    extract.push(extract_ms);
                }
                fs::remove_dir_all(&output).unwrap();
                fs::remove_file(&archive).unwrap();
            }
            let mut create_sorted = create.clone();
            create_sorted.sort_by(f64::total_cmp);
            let mut extract_sorted = extract.clone();
            extract_sorted.sort_by(f64::total_cmp);
            results.push(json!({"corpus":corpus,"files":files,"input_bytes": files * per_file,"format":format.as_str(),"archive_bytes":archive_bytes,
                "create_ms":create,"extract_ms":extract,"create_p50_ms":create_sorted[runs/2],"extract_p50_ms":extract_sorted[runs/2]}));
        }
    }
    println!("{}", serde_json::to_string_pretty(&json!({"schema":1,"runs":runs,"warmups":1,"cache":"warm; not cold-start or durable extraction throughput","allocator":"mimalloc","compression":"normal","results":results})).unwrap());
}
