use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zip::write::SimpleFileOptions;

struct Worker {
    child: Child,
    input: Option<ChildStdin>,
    events: Receiver<Value>,
}

impl Worker {
    fn success(&mut self, command: &str, args: Value) -> Value {
        self.send("operation", command, args);
        let terminal = self.terminal("operation");
        assert_eq!(terminal["event"], "complete", "{terminal}");
        terminal["data"].clone()
    }

    fn new() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_archi-worker"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = child.stdout.take().unwrap();
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                assert!(line.len() < 1024 * 1024);
                let event: Value = serde_json::from_str(&line).unwrap();
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            events,
        }
    }

    fn raw(&mut self, data: &[u8]) {
        self.input.as_mut().unwrap().write_all(data).unwrap();
        self.input.as_mut().unwrap().flush().unwrap();
    }

    fn send(&mut self, id: &str, command: &str, mut args: Value) {
        args["version"] = json!(1);
        args["id"] = json!(id);
        args["command"] = json!(command);
        self.raw(format!("{args}\n").as_bytes());
    }

    fn event(&self) -> Value {
        let event = self
            .events
            .recv_timeout(Duration::from_secs(15))
            .expect("worker response timeout");
        assert_eq!(event["version"], 1);
        event
    }

    fn terminal(&self, id: &str) -> Value {
        loop {
            let event = self.event();
            if event["id"] == id && matches!(event["event"].as_str(), Some("complete" | "error")) {
                return event;
            }
        }
    }

    fn wait_exit(&mut self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker failed to exit"
            );
            std::thread::yield_now();
        }
    }

    fn shutdown(&mut self) {
        self.send("shutdown", "shutdown", json!({}));
        assert_eq!(self.terminal("shutdown")["event"], "complete");
        self.wait_exit();
    }
}

fn create_args(source: &Path, output: &Path, format: &str, password: Option<&str>) -> Value {
    json!({"source_paths":[source], "output_path":output, "options": {
        "format":format, "compression":"fast", "includeRoot":false,
        "overwrite":false, "password":password
    }})
}

fn edit_args(archive: &Path, action: Value, password: Option<&str>) -> Value {
    json!({"path":archive, "action":action, "options":{"strategy":"preferCompact", "password":password}})
}

fn children(directory: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    paths
}

#[test]
fn create_all_formats_round_trips_and_preserves_existing_output_on_error() {
    let fixture = Fixture::new();
    let source = fixture.0.join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("payload.txt"), b"worker create payload").unwrap();
    let mut worker = Worker::new();
    for format in ["zip", "sevenZ", "tar", "tarGz", "tarBz2", "tarXz"] {
        let archive = fixture.0.join(format!("archive-{format}"));
        let args = create_args(&source, &archive, format, None);
        let result = worker.success("create", args.clone());
        assert_eq!(
            Path::new(result["destination"].as_str().unwrap()),
            archive.canonicalize().unwrap()
        );
        let destination = fixture.0.join(format!("out-{format}"));
        fs::create_dir(&destination).unwrap();
        worker.success(
            "extract",
            json!({"path":archive, "destination":destination}),
        );
        assert_eq!(
            fs::read(destination.join("payload.txt")).unwrap(),
            b"worker create payload"
        );
        assert!(
            !destination.join("source").exists(),
            "includeRoot must be respected"
        );
        let original = fs::read(&archive).unwrap();
        let before = children(&fixture.0);
        worker.send("exists", "create", args.clone());
        assert_eq!(worker.terminal("exists")["event"], "error");
        let mut invalid = args;
        invalid["options"]["overwrite"] = json!(true);
        invalid["source_paths"] = json!([fixture.0.join("missing")]);
        worker.send("invalid", "create", invalid);
        assert_eq!(worker.terminal("invalid")["event"], "error");
        assert_eq!(fs::read(&archive).unwrap(), original);
        assert_eq!(children(&fixture.0), before, "temporary output leaked");
    }
    worker.shutdown();
}

#[test]
fn encrypted_creation_and_tar_rejection_never_downgrade_to_plaintext() {
    let fixture = Fixture::new();
    let source = fixture.0.join("payload.txt");
    fs::write(&source, b"private payload").unwrap();
    let mut worker = Worker::new();
    for format in ["zip", "sevenZ"] {
        let archive = fixture.0.join(format!("encrypted-{format}"));
        worker.success(
            "create",
            create_args(&source, &archive, format, Some("  secret  ")),
        );
        worker.send("no-password", "test", json!({"path":archive}));
        let terminal = worker.terminal("no-password");
        assert!(
            terminal["event"] == "error"
                || terminal["data"]["tested_failed"].as_u64().unwrap_or(0) > 0,
            "{terminal}"
        );
        let destination = fixture.0.join(format!("out-{format}"));
        fs::create_dir(&destination).unwrap();
        worker.success(
            "extract",
            json!({"path":archive,"destination":destination,"password":"  secret  "}),
        );
        assert_eq!(
            fs::read(destination.join("payload.txt")).unwrap(),
            b"private payload"
        );
        if format == "sevenZ" {
            worker.success(
                "edit",
                edit_args(
                    &archive,
                    json!({"kind":"rename","from_path":"payload.txt","to_path":"renamed.txt"}),
                    Some("  secret  "),
                ),
            );
            worker.send("still-encrypted", "open", json!({"path":archive}));
            assert_eq!(
                worker.terminal("still-encrypted")["data"]["code"],
                "password_required"
            );
            let destination = fixture.0.join("edited-7z");
            fs::create_dir(&destination).unwrap();
            worker.success(
                "extract",
                json!({"path":archive,"destination":destination,"password":"  secret  "}),
            );
            assert_eq!(
                fs::read(destination.join("renamed.txt")).unwrap(),
                b"private payload"
            );
        }
    }
    for format in ["tar", "tarGz", "tarBz2", "tarXz"] {
        let output = fixture.0.join(format);
        fs::write(&output, b"existing output").unwrap();
        let before = children(&fixture.0);
        let mut args = create_args(&source, &output, format, Some("secret"));
        args["options"]["overwrite"] = json!(true);
        worker.send("tar-encryption", "create", args);
        assert_eq!(
            worker.terminal("tar-encryption")["data"]["code"],
            "unsupported_encryption"
        );
        assert_eq!(fs::read(&output).unwrap(), b"existing output");
        assert_eq!(children(&fixture.0), before);
    }
    worker.shutdown();
}

#[test]
fn every_edit_action_round_trips_across_writable_formats() {
    let fixture = Fixture::new();
    let source = fixture.0.join("payload.txt");
    let added = fixture.0.join("added.txt");
    fs::write(&source, b"original payload").unwrap();
    fs::write(&added, b"replacement payload").unwrap();
    let mut worker = Worker::new();
    for format in ["zip", "sevenZ", "tar", "tarGz", "tarBz2", "tarXz"] {
        let archive = fixture.0.join(format!("edit-{format}"));
        worker.success("create", create_args(&source, &archive, format, None));
        for action in [
            json!({"kind":"create_folder","folder_path":"folder"}),
            json!({"kind":"rename","from_path":"payload.txt","to_path":"renamed.txt"}),
            json!({"kind":"add","source_paths":[added],"archive_parent":"folder"}),
            json!({"kind":"replace","entry_path":"renamed.txt","source_file":added}),
            json!({"kind":"move","source_paths":["renamed.txt"],"dest_folder":"folder"}),
            json!({"kind":"delete","paths":["folder/added.txt"]}),
            json!({"kind":"compact"}),
        ] {
            worker.success("edit", edit_args(&archive, action, None));
        }
        let destination = fixture.0.join(format!("out-{format}"));
        fs::create_dir(&destination).unwrap();
        worker.success("extract", json!({"path":archive,"destination":destination}));
        assert_eq!(
            fs::read(destination.join("folder/renamed.txt")).unwrap(),
            b"replacement payload"
        );
        assert_eq!(children(&destination), vec![destination.join("folder")]);
        assert_eq!(
            children(&destination.join("folder")),
            vec![destination.join("folder/renamed.txt")]
        );
        worker.success("test", json!({"path":archive}));
        let original = fs::read(&archive).unwrap();
        let before = children(&fixture.0);
        for action in [
            json!({"kind":"rename","from_path":"folder/renamed.txt","to_path":"../escape"}),
            json!({"kind":"add","source_paths":[fixture.0.join("missing")],"archive_parent":""}),
            json!({"kind":"replace","entry_path":"folder/renamed.txt","source_file":fixture.0.join("missing")}),
            json!({"kind":"create_folder","folder_path":"folder/renamed.txt"}),
        ] {
            worker.send("invalid-edit", "edit", edit_args(&archive, action, None));
            assert_eq!(worker.terminal("invalid-edit")["event"], "error");
            assert_eq!(
                fs::read(&archive).unwrap(),
                original,
                "failed edit changed {format}"
            );
            assert_eq!(children(&fixture.0), before, "temporary edit file leaked");
        }
    }
    worker.shutdown();
}

#[test]
fn every_encrypted_zip_edit_is_rejected_without_changing_archive() {
    let fixture = Fixture::new();
    let archive = fixture.zip(2, Some("secret"));
    let source = fixture.0.join("source.txt");
    fs::write(&source, b"replacement").unwrap();
    let original = fs::read(&archive).unwrap();
    let before = children(&fixture.0);
    let mut worker = Worker::new();
    for action in [
        json!({"kind":"delete","paths":["file-0.txt"]}),
        json!({"kind":"rename","from_path":"file-0.txt","to_path":"renamed.txt"}),
        json!({"kind":"create_folder","folder_path":"new"}),
        json!({"kind":"add","source_paths":[source],"archive_parent":""}),
        json!({"kind":"replace","entry_path":"file-0.txt","source_file":source}),
        json!({"kind":"move","source_paths":["file-0.txt"],"dest_folder":"new"}),
        json!({"kind":"compact"}),
    ] {
        worker.send(
            "encrypted-edit",
            "edit",
            edit_args(&archive, action, Some("secret")),
        );
        assert_eq!(
            worker.terminal("encrypted-edit")["data"]["code"],
            "unsupported_encrypted_edit"
        );
        assert_eq!(fs::read(&archive).unwrap(), original);
        assert_eq!(children(&fixture.0), before);
    }
    worker.shutdown();
}

#[test]
fn immediate_create_and_edit_cancel_preserve_output_and_remove_temporary_files() {
    let fixture = Fixture::new();
    let source = fixture.0.join("large.bin");
    File::create(&source)
        .unwrap()
        .set_len(32 * 1024 * 1024)
        .unwrap();
    let archive = fixture.zip(600, None);
    let original = fs::read(&archive).unwrap();
    let output = fixture.0.join("existing-output");
    fs::write(&output, b"keep output").unwrap();
    let before = children(&fixture.0);
    let mut worker = Worker::new();
    for format in ["zip", "sevenZ", "tar", "tarGz", "tarBz2", "tarXz"] {
        let mut args = create_args(&source, &output, format, None);
        args["options"]["overwrite"] = json!(true);
        args["version"] = json!(1);
        args["id"] = json!("mutation");
        args["command"] = json!("create");
        worker.raw(
            format!(
                "{args}\n{}\n",
                json!({"version":1,"id":"cancel","command":"cancel","operation_id":"mutation"})
            )
            .as_bytes(),
        );
        assert_cancelled(&worker);
        assert_eq!(fs::read(&output).unwrap(), b"keep output");
        assert_eq!(children(&fixture.0), before);
    }
    let mut args = edit_args(
        &archive,
        json!({"kind":"add","source_paths":[source],"archive_parent":""}),
        None,
    );
    args["version"] = json!(1);
    args["id"] = json!("mutation");
    args["command"] = json!("edit");
    worker.raw(
        format!(
            "{args}\n{}\n",
            json!({"version":1,"id":"cancel","command":"cancel","operation_id":"mutation"})
        )
        .as_bytes(),
    );
    assert_cancelled(&worker);
    assert_eq!(fs::read(&archive).unwrap(), original);
    assert_eq!(children(&fixture.0), before);
    // Original remains readable after cancellation, not only byte-identical.
    let mut zip = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
    let mut contents = String::new();
    zip.by_name("file-0.txt")
        .unwrap()
        .read_to_string(&mut contents)
        .unwrap();
    assert_eq!(contents, "content-0");
    worker.shutdown();
}

fn assert_cancelled(worker: &Worker) {
    let mut acknowledged = false;
    let mut stopped = false;
    while !acknowledged || !stopped {
        let event = worker.event();
        if event["id"] == "cancel" {
            assert_eq!(event["data"]["cancelled"], true);
            acknowledged = true;
        } else if event["id"] == "mutation"
            && matches!(event["event"].as_str(), Some("complete" | "error"))
        {
            assert_eq!(event["event"], "error", "{event}");
            assert_eq!(event["data"]["code"], "cancelled", "{event}");
            stopped = true;
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("archi-protocol-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn zip(&self, count: usize, password: Option<&str>) -> PathBuf {
        let path = self.0.join("fixture.zip");
        let mut writer = zip::ZipWriter::new(File::create(&path).unwrap());
        let mut options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        if let Some(password) = password {
            options = options.with_aes_encryption(zip::AesMode::Aes256, password);
        }
        for index in 0..count {
            writer
                .start_file(format!("file-{index}.txt"), options)
                .unwrap();
            writer
                .write_all(format!("content-{index}").as_bytes())
                .unwrap();
        }
        writer.finish().unwrap();
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn hello_batched_listing_extract_test_and_terminal_chaining() {
    let fixture = Fixture::new();
    let archive = fixture.zip(600, None);
    let destination = fixture.0.join("out");
    fs::create_dir(&destination).unwrap();
    let mut worker = Worker::new();
    worker.send("hello", "hello", json!({}));
    assert_eq!(worker.terminal("hello")["data"]["protocol_version"], 1);
    worker.send("open", "open", json!({"path": archive}));
    let mut entries = 0;
    let mut batches = 0;
    loop {
        let event = worker.event();
        assert_eq!(event["id"], "open");
        match event["event"].as_str().unwrap() {
            "entries" => {
                let batch = event["data"].as_array().unwrap();
                assert!(batch.len() <= 256);
                entries += batch.len();
                batches += 1;
            }
            "archive" => assert_eq!(event["data"]["format"], "zip"),
            "complete" => break,
            other => panic!("unexpected event: {other}"),
        }
    }
    assert_eq!((entries, batches), (600, 3));
    worker.send("extract", "extract", json!({"path": archive, "destination": destination, "selected_paths": ["file-0.txt", "file-599.txt"]}));
    assert_eq!(worker.terminal("extract")["event"], "complete");
    assert_eq!(
        fs::read(destination.join("file-0.txt")).unwrap(),
        b"content-0"
    );
    assert_eq!(
        fs::read(destination.join("file-599.txt")).unwrap(),
        b"content-599"
    );
    assert!(!destination.join("file-1.txt").exists());
    worker.send("test", "test", json!({"path": archive}));
    let tested = worker.terminal("test");
    assert_eq!(tested["data"]["tested_ok"], 600);
    // Next operation is legal immediately after either terminal event.
    for _ in 0..200 {
        worker.send(
            "missing",
            "open",
            json!({"path": fixture.0.join("missing.zip")}),
        );
        assert_eq!(worker.terminal("missing")["data"]["code"], "not_found");
    }
    worker.shutdown();
}

fn start_conflict(worker: &mut Worker, archive: &Path, destination: &Path) -> Value {
    worker.send(
        "extract",
        "extract",
        json!({"path": archive, "destination": destination}),
    );
    loop {
        let event = worker.event();
        if event["event"] == "conflict" {
            return event["data"].clone();
        }
        assert_ne!(event["event"], "error");
    }
}

#[test]
fn conflict_busy_cancel_and_shutdown_preserve_existing_file() {
    let fixture = Fixture::new();
    let archive = fixture.zip(1, None);
    let destination = fixture.0.join("out");
    fs::create_dir(&destination).unwrap();
    let existing = destination.join("file-0.txt");
    fs::write(&existing, b"original").unwrap();
    let mut worker = Worker::new();
    let conflict = start_conflict(&mut worker, &archive, &destination);
    worker.send("other", "open", json!({"path": archive}));
    assert_eq!(worker.terminal("other")["data"]["code"], "busy");
    worker.send("resolve", "resolve_conflict", json!({"operation_id": "extract", "conflict_id": conflict["conflict_id"], "decision":"skip", "apply_to_all":true}));
    // Acknowledgement and operation terminal may arrive in either order.
    let mut resolved = false;
    let mut extracted = false;
    while !resolved || !extracted {
        let e = worker.event();
        assert_ne!(e["event"], "error");
        if e["event"] == "complete" {
            resolved |= e["id"] == "resolve";
            extracted |= e["id"] == "extract";
        }
    }
    assert_eq!(fs::read(&existing).unwrap(), b"original");
    start_conflict(&mut worker, &archive, &destination);
    worker.send("cancel", "cancel", json!({"operation_id":"extract"}));
    let mut cancelled = false;
    let mut stopped = false;
    while !cancelled || !stopped {
        let e = worker.event();
        if e["id"] == "cancel" {
            assert_eq!(e["data"]["cancelled"], true);
            cancelled = true;
        }
        if e["id"] == "extract" && e["event"] == "error" {
            assert_eq!(e["data"]["code"], "cancelled");
            stopped = true;
        }
    }
    start_conflict(&mut worker, &archive, &destination);
    worker.shutdown();
    assert_eq!(fs::read(&existing).unwrap(), b"original");
}

#[test]
fn password_whitespace_round_trips_and_sevenz_opens() {
    let fixture = Fixture::new();
    let archive = fixture.zip(1, Some("  secret  "));
    let destination = fixture.0.join("out");
    fs::create_dir(&destination).unwrap();
    let mut worker = Worker::new();
    worker.send(
        "bad",
        "extract",
        json!({"path":archive,"destination":destination,"password":"secret"}),
    );
    assert_eq!(worker.terminal("bad")["event"], "error");
    assert!(!destination.join("file-0.txt").exists());
    worker.send(
        "good",
        "extract",
        json!({"path":archive,"destination":destination,"password":"  secret  "}),
    );
    assert_eq!(worker.terminal("good")["event"], "complete");
    assert_eq!(
        fs::read(destination.join("file-0.txt")).unwrap(),
        b"content-0"
    );
    let sevenz = fixture.0.join("fixture.7z");
    sevenz_rust2::compress_to_path(&destination, &sevenz).unwrap();
    worker.send("7z", "open", json!({"path":sevenz}));
    assert_eq!(worker.event()["data"]["format"], "7z");
    assert_eq!(worker.terminal("7z")["event"], "complete");
    worker.shutdown();
}

#[test]
fn malformed_oversized_and_truncated_frames_and_eof() {
    let mut worker = Worker::new();
    worker.raw(b"not json\n");
    assert_eq!(worker.event()["data"]["code"], "invalid_request");
    worker.send("version", "hello", json!({}));
    assert_eq!(worker.terminal("version")["event"], "complete");
    worker.raw(&vec![b'x'; 1024 * 1024 + 1]);
    assert_eq!(worker.event()["data"]["code"], "invalid_frame");
    worker.wait_exit();
    let mut worker = Worker::new();
    worker.raw(b"{");
    worker.input.take();
    assert_eq!(worker.event()["data"]["code"], "invalid_frame");
    worker.wait_exit();
    let mut worker = Worker::new();
    worker.input.take();
    worker.wait_exit();
}

#[test]
fn immediate_cancel_is_registered_and_eof_releases_conflict_waiter() {
    let fixture = Fixture::new();
    let archive = fixture.zip(600, None);
    let destination = fixture.0.join("out");
    fs::create_dir(&destination).unwrap();
    let mut worker = Worker::new();
    // Pipeline both requests in one write, before any operation event is consumed.
    worker.raw(format!("{}\n{}\n",
        json!({"version":1,"id":"extract","command":"extract","path":archive,"destination":destination}),
        json!({"version":1,"id":"cancel","command":"cancel","operation_id":"extract"})
    ).as_bytes());
    let mut cancelled = false;
    let mut stopped = false;
    while !cancelled || !stopped {
        let e = worker.event();
        if e["id"] == "cancel" {
            assert_eq!(e["data"]["cancelled"], true);
            cancelled = true;
        }
        if e["id"] == "extract" && e["event"] == "error" {
            assert_eq!(e["data"]["code"], "cancelled");
            stopped = true;
        }
    }
    fs::write(destination.join("file-0.txt"), b"original").unwrap();
    start_conflict(&mut worker, &archive, &destination);
    worker.input.take();
    worker.wait_exit();
    assert_eq!(
        fs::read(destination.join("file-0.txt")).unwrap(),
        b"original"
    );
}
