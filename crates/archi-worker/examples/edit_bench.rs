//! Real archive edit benchmark. Fixture setup and content validation are not timed.
use archi_core::models::EditOptions;
use archi_core::{archive, archive_edit, extraction};
use serde_json::json;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::atomic::AtomicBool,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

struct Worker {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
impl Worker {
    fn new(path: &str) -> Self {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut worker = Self {
            child,
            input,
            output,
        };
        worker.request("hello", json!({}));
        worker
    }
    fn request(&mut self, command: &str, mut args: serde_json::Value) {
        args["version"] = json!(1);
        args["id"] = json!("bench");
        args["command"] = json!(command);
        writeln!(self.input, "{args}").unwrap();
        self.input.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(
                self.output.read_line(&mut line).unwrap() > 0,
                "worker exited"
            );
            assert!(line.len() <= 1024 * 1024);
            let event: serde_json::Value = serde_json::from_str(&line).unwrap();
            assert_ne!(event["event"], "error", "{event}");
            if event["event"] == "complete" {
                assert_eq!(event["data"]["tested_failed"].as_u64().unwrap_or(0), 0);
                return;
            }
        }
    }
    fn operation(
        &mut self,
        operation: &str,
        path: &Path,
        added: &Path,
        batch: &Path,
        replacement: &Path,
    ) {
        if matches!(operation, "open" | "test") {
            self.request(operation, json!({"path":path}));
            return;
        }
        let action = match operation {
            "rename-leaf" => {
                json!({"kind":"rename", "from_path":"tree/item-000000.txt", "to_path":"tree/renamed.txt"})
            }
            "rename-tree" => json!({"kind":"rename", "from_path":"tree", "to_path":"renamed"}),
            "move-tree" => {
                json!({"kind":"move", "source_paths":["tree"], "dest_folder":"destination"})
            }
            "delete" => json!({"kind":"delete", "paths":["tree/item-000000.txt"]}),
            "compact" => json!({"kind":"compact"}),
            "add" | "add-1000" => {
                json!({"kind":"add", "source_paths":[if operation == "add" { added } else { batch }], "archive_parent":""})
            }
            "replace" => {
                json!({"kind":"replace", "entry_path":"tree/item-000000.txt", "source_file":replacement})
            }
            "mkdir" => json!({"kind":"create_folder", "folder_path":"new-folder"}),
            _ => unreachable!(),
        };
        self.request("edit", json!({"path":path, "action":action}));
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let _ = self.child.kill();
        } else {
            self.request("shutdown", json!({}));
        }
        let _ = self.child.wait();
    }
}

fn main() {
    let count: usize = std::env::args()
        .nth(1)
        .unwrap_or("10000".into())
        .parse()
        .unwrap();
    let runs: usize = std::env::args()
        .nth(2)
        .unwrap_or("5".into())
        .parse()
        .unwrap();
    assert!(count >= 10 && (3..=50).contains(&runs));
    let root = std::env::temp_dir().join(format!(
        "archi-edit-bench-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let fixture = root.join("fixture.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&fixture).unwrap());
    for index in 0..count {
        zip.start_file(
            format!("tree/item-{index:06}.txt"),
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
        zip.write_all(format!("payload-{index}").as_bytes())
            .unwrap();
    }
    zip.finish().unwrap();
    let added = root.join("added.txt");
    fs::write(&added, b"added payload").unwrap();
    let replacement = root.join("replacement.txt");
    fs::write(&replacement, b"replacement payload").unwrap();
    let batch = root.join("batch");
    fs::create_dir(&batch).unwrap();
    for index in 0..1000 {
        fs::write(
            batch.join(format!("new-{index:06}.txt")),
            format!("added-{index}"),
        )
        .unwrap();
    }
    let cancelled = AtomicBool::new(false);
    let options = EditOptions::default();
    // Optional executable: measure full NDJSON roundtrip through one long-lived worker.
    let mut worker = std::env::args().nth(3).map(|path| Worker::new(&path));
    let mut results = Vec::new();
    for operation in [
        "open",
        "rename-leaf",
        "rename-tree",
        "move-tree",
        "delete",
        "compact",
        "test",
        "add",
        "add-1000",
        "replace",
        "mkdir",
    ] {
        let mut samples = Vec::new();
        for iteration in 0..=runs {
            let path: PathBuf = root.join("working.zip");
            fs::copy(&fixture, &path).unwrap();
            let started = Instant::now();
            if let Some(worker) = worker.as_mut() {
                worker.operation(operation, &path, &added, &batch, &replacement);
            } else {
                match operation {
                    "open" => {
                        archive::open_archive(&path, None).unwrap();
                    }
                    "rename-leaf" => {
                        archive_edit::rename_entry(
                            &path,
                            "tree/item-000000.txt",
                            "tree/renamed.txt",
                            "bench",
                            &cancelled,
                            |_| {},
                            None,
                            &options,
                        )
                        .unwrap();
                    }
                    "rename-tree" => {
                        archive_edit::rename_entry(
                            &path,
                            "tree",
                            "renamed",
                            "bench",
                            &cancelled,
                            |_| {},
                            None,
                            &options,
                        )
                        .unwrap();
                    }
                    "move-tree" => {
                        archive_edit::move_entries(
                            &path,
                            &["tree".into()],
                            "destination",
                            "bench",
                            &cancelled,
                            |_| {},
                            None,
                            &options,
                        )
                        .unwrap();
                    }
                    "delete" => {
                        archive_edit::delete_entries(
                            &path,
                            &["tree/item-000000.txt".into()],
                            "bench",
                            &cancelled,
                            |_| {},
                            None,
                            &options,
                        )
                        .unwrap();
                    }
                    "compact" => {
                        archive_edit::compact_archive(
                            &path,
                            "bench",
                            &cancelled,
                            |_| {},
                            None,
                            &options,
                        )
                        .unwrap();
                    }
                    "test" => {
                        assert_eq!(
                            archi_core::testing::test_archive(
                                &path,
                                "bench",
                                &cancelled,
                                None,
                                |_| {}
                            )
                            .unwrap()
                            .tested_failed,
                            0
                        );
                    }
                    "add" | "add-1000" => {
                        let source = if operation == "add" { &added } else { &batch };
                        archive_edit::add_paths(
                            &path,
                            &[source.to_string_lossy().into_owned()],
                            "",
                            "bench",
                            &cancelled,
                            |_| {},
                            None,
                            &options,
                        )
                        .unwrap();
                    }
                    "replace" => {
                        archive_edit::replace_file(
                            &path,
                            "tree/item-000000.txt",
                            &replacement,
                            "bench",
                            &cancelled,
                            |_| {},
                            None,
                            &options,
                        )
                        .unwrap();
                    }
                    "mkdir" => {
                        archive_edit::create_folder(
                            &path,
                            "new-folder",
                            "bench",
                            &cancelled,
                            |_| {},
                            None,
                            &options,
                        )
                        .unwrap();
                    }
                    _ => unreachable!(),
                }
            }
            let elapsed = started.elapsed().as_secs_f64() * 1000.0;
            // Decode every member and compare deterministic payloads outside timing.
            let mut verified = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
            for index in 0..verified.len() {
                let mut entry = verified.by_index(index).unwrap();
                if entry.is_dir() {
                    continue;
                }
                let mut bytes = String::new();
                entry.read_to_string(&mut bytes).unwrap();
                let expected = if entry.name() == "added.txt" {
                    "added payload".into()
                } else if let Some(name) = entry.name().strip_prefix("batch/new-") {
                    format!(
                        "added-{}",
                        name.trim_end_matches(".txt").parse::<usize>().unwrap()
                    )
                } else if operation == "replace" && entry.name() == "tree/item-000000.txt" {
                    "replacement payload".into()
                } else if entry.name() == "tree/renamed.txt" {
                    "payload-0".into()
                } else {
                    format!(
                        "payload-{}",
                        entry
                            .name()
                            .rsplit("item-")
                            .next()
                            .unwrap()
                            .trim_end_matches(".txt")
                            .parse::<usize>()
                            .unwrap()
                    )
                };
                assert_eq!(bytes, expected, "{}", entry.name());
            }
            drop(verified);
            let info = archive::open_archive(&path, None).unwrap();
            let added_count = match operation {
                "add" => 1,
                "add-1000" => 1000,
                _ => 0,
            };
            assert_eq!(
                info.stats.file_count,
                (count + added_count - usize::from(operation == "delete")) as u64
            );
            if operation == "mkdir" {
                assert!(info
                    .entries
                    .iter()
                    .any(|e| e.path == "new-folder" && e.is_directory));
            }
            let selected = match operation {
                "add" => "added.txt",
                "add-1000" => "batch/new-000000.txt",
                "rename-leaf" => "tree/renamed.txt",
                "rename-tree" => "renamed/item-000000.txt",
                "move-tree" => "destination/tree/item-000000.txt",
                "delete" => "tree/item-000001.txt",
                _ => "tree/item-000000.txt",
            };
            let output = root.join("out");
            fs::create_dir(&output).unwrap();
            extraction::extract_any(
                &path,
                &output,
                "verify",
                &cancelled,
                Some(&[selected.into()]),
                None,
                &extraction::FailOnConflict,
                |_| {},
            )
            .unwrap();
            assert_eq!(
                fs::read_to_string(output.join(selected)).unwrap(),
                match operation {
                    "add" => "added payload",
                    "add-1000" => "added-0",
                    "replace" => "replacement payload",
                    "delete" => "payload-1",
                    _ => "payload-0",
                }
            );
            fs::remove_dir_all(output).unwrap();
            fs::remove_file(path).unwrap();
            if iteration > 0 {
                samples.push(elapsed);
            }
        }
        let mut sorted = samples.clone();
        sorted.sort_by(f64::total_cmp);
        results.push(json!({"operation":operation,"samples_ms":samples,"p50_ms":sorted[runs/2],"p95_ms":sorted[((runs as f64 * 0.95).ceil() as usize - 1).min(runs-1)]}));
    }
    fs::remove_dir_all(root).unwrap();
    println!("{}", serde_json::to_string_pretty(&json!({"schema":1,"entries":count,"runs":runs,"warmups":1,"cache":"warm","execution":if worker.is_some() { "worker-ndjson" } else { "core" },"results":results})).unwrap());
}
