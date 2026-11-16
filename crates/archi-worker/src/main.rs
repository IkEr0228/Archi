use archi_core::archive::open_archive;
use archi_core::extraction::{extract_transactional, ConflictResolver};
use archi_core::models::{
    CommandError, ConflictDecision, CreateFormat, CreateOptions, EditOptions, OperationProgress,
};
use archi_core::operations::OperationRegistry;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

const VERSION: u32 = 1;
const MAX_FRAME: usize = 1024 * 1024;
const LIST_BATCH: usize = 256;

#[derive(Deserialize)]
struct Request {
    version: u32,
    id: String,
    #[serde(flatten)]
    command: Command,
}

#[derive(Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
enum Command {
    Hello,
    Open {
        path: String,
        password: Option<String>,
    },
    Extract {
        path: String,
        destination: String,
        selected_paths: Option<Vec<String>>,
        password: Option<String>,
    },
    Test {
        path: String,
        password: Option<String>,
    },
    Create {
        source_paths: Vec<String>,
        output_path: String,
        options: CreateOptions,
    },
    Edit {
        path: String,
        #[serde(default)]
        options: EditOptions,
        action: EditAction,
    },
    Cancel {
        operation_id: String,
    },
    ResolveConflict {
        operation_id: String,
        conflict_id: String,
        decision: ConflictDecision,
        #[serde(default)]
        apply_to_all: bool,
    },
    Shutdown,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum EditAction {
    Delete {
        paths: Vec<String>,
    },
    Rename {
        from_path: String,
        to_path: String,
    },
    CreateFolder {
        folder_path: String,
    },
    Add {
        source_paths: Vec<String>,
        archive_parent: String,
    },
    Replace {
        entry_path: String,
        source_file: String,
    },
    Move {
        source_paths: Vec<String>,
        dest_folder: String,
    },
    Compact,
}

#[derive(Clone)]
struct Output(Arc<Mutex<io::Stdout>>);

#[derive(Serialize)]
struct Response<'a, T> {
    version: u32,
    id: &'a str,
    event: &'a str,
    data: T,
}

impl Output {
    fn send(&self, id: &str, event: &str, data: impl Serialize) -> io::Result<()> {
        let frame = serde_json::to_vec(&Response {
            version: VERSION,
            id,
            event,
            data,
        })?;
        if frame.len() > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Response exceeds frame limit",
            ));
        }
        let mut writer = self
            .0
            .lock()
            .map_err(|_| io::Error::other("Output unavailable"))?;
        writer.write_all(&frame)?;
        writer.write_all(b"\n")?;
        writer.flush()
    }

    fn error(&self, id: &str, error: CommandError) {
        let _ = self.send(id, "error", error);
    }
}

struct Resolver {
    registry: OperationRegistry,
    output: Output,
    sequence: std::sync::atomic::AtomicU64,
}

impl ConflictResolver for Resolver {
    fn resolve_file_exists(
        &self,
        operation_id: &str,
        entry_path: &str,
        dest_path: &Path,
    ) -> Result<ConflictDecision, CommandError> {
        if let Some(policy) = self.registry.peek_apply_policy(operation_id) {
            return Ok(policy);
        }
        let conflict_id = self
            .sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .to_string();
        self.registry
            .install_conflict_waiter(operation_id, &conflict_id)
            .map_err(|e| CommandError::new("operation_failed", e))?;
        self.output
            .send(
                operation_id,
                "conflict",
                json!({
                    "conflict_id": conflict_id, "entry_path": entry_path,
                    "dest_path": dest_path.to_string_lossy()
                }),
            )
            .map_err(|_| CommandError::new("client_disconnected", "Cannot deliver conflict"))?;
        self.registry
            .recv_conflict_decision(operation_id, &conflict_id)
            .map_err(|e| CommandError::new("operation_failed", e))
    }
}

fn execute(
    command: Command,
    id: &str,
    state: Arc<archi_core::operations::OperationState>,
    registry: &OperationRegistry,
    output: &Output,
) -> Result<Value, CommandError> {
    let result = (|| {
        let emit = |progress: OperationProgress| {
            if output.send(id, "progress", progress).is_err() {
                registry.cancel(id);
            }
        };
        let result = match command {
            Command::Open { path, password } => {
                let mut info = open_archive(Path::new(&path), password)?;
                let entries = std::mem::take(&mut info.entries);
                output.send(id, "archive", &info).map_err(output_error)?;
                for batch in entries.chunks(LIST_BATCH) {
                    if state.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                        return Err(CommandError::new("cancelled", "Listing cancelled"));
                    }
                    output.send(id, "entries", batch).map_err(output_error)?;
                }
                json!({"entry_count": entries.len()})
            }
            Command::Extract {
                path,
                destination,
                selected_paths,
                password,
            } => {
                let resolver = Resolver {
                    registry: registry.clone(),
                    output: output.clone(),
                    sequence: 0.into(),
                };
                json!(extract_transactional(
                    Path::new(&path),
                    Path::new(&destination),
                    id,
                    &state.cancelled,
                    selected_paths.as_deref(),
                    password,
                    &resolver,
                    emit
                )?)
            }
            Command::Test { path, password } => json!(archi_core::testing::test_archive(
                Path::new(&path),
                id,
                &state.cancelled,
                password,
                emit
            )?),
            Command::Create {
                source_paths,
                output_path,
                options,
            } => {
                let path = Path::new(&output_path);
                let summary = match options.format {
                    CreateFormat::Zip => archi_core::zipper::create_zip_archive(
                        &source_paths,
                        path,
                        id,
                        &state.cancelled,
                        &options,
                        emit,
                    ),
                    CreateFormat::SevenZ => archi_core::sevenz_format::create_sevenz_archive(
                        &source_paths,
                        path,
                        id,
                        &state.cancelled,
                        options
                            .password
                            .as_deref()
                            .map(sevenz_rust2::Password::new)
                            .unwrap_or_default(),
                        &options,
                        emit,
                    ),
                    _ => {
                        if options.password.is_some() {
                            return Err(CommandError::new(
                                "unsupported_encryption",
                                "TAR formats cannot be encrypted. Choose ZIP or 7z.",
                            ));
                        }
                        archi_core::tar_create::create_tar_archive(
                            &source_paths,
                            path,
                            id,
                            &state.cancelled,
                            &options,
                            emit,
                        )
                    }
                }?;
                json!(summary)
            }
            Command::Edit {
                path,
                options,
                action,
            } => {
                use archi_core::archive_edit as edit;
                let path = Path::new(&path);
                // Core dispatch rejects read-only formats; ZIP planner gates encryption.
                let password = options.password.clone();
                let summary = match action {
                    EditAction::Delete { paths } => edit::delete_entries(
                        path,
                        &paths,
                        id,
                        &state.cancelled,
                        emit,
                        password,
                        &options,
                    ),
                    EditAction::Rename { from_path, to_path } => edit::rename_entry(
                        path,
                        &from_path,
                        &to_path,
                        id,
                        &state.cancelled,
                        emit,
                        password,
                        &options,
                    ),
                    EditAction::CreateFolder { folder_path } => edit::create_folder(
                        path,
                        &folder_path,
                        id,
                        &state.cancelled,
                        emit,
                        password,
                        &options,
                    ),
                    EditAction::Add {
                        source_paths,
                        archive_parent,
                    } => edit::add_paths(
                        path,
                        &source_paths,
                        &archive_parent,
                        id,
                        &state.cancelled,
                        emit,
                        password,
                        &options,
                    ),
                    EditAction::Replace {
                        entry_path,
                        source_file,
                    } => edit::replace_file(
                        path,
                        &entry_path,
                        Path::new(&source_file),
                        id,
                        &state.cancelled,
                        emit,
                        password,
                        &options,
                    ),
                    EditAction::Move {
                        source_paths,
                        dest_folder,
                    } => edit::move_entries(
                        path,
                        &source_paths,
                        &dest_folder,
                        id,
                        &state.cancelled,
                        emit,
                        password,
                        &options,
                    ),
                    EditAction::Compact => {
                        edit::compact_archive(path, id, &state.cancelled, emit, password, &options)
                    }
                }?;
                json!(summary)
            }
            _ => {
                return Err(CommandError::new(
                    "invalid_request",
                    "Not an archive operation",
                ))
            }
        };
        Ok(result)
    })();
    registry.finish(id);
    result
}

fn output_error(_: io::Error) -> CommandError {
    CommandError::new("transport_failed", "Cannot deliver worker response")
}

/// Read at most one bounded NDJSON frame, never allocate an unbounded read_line.
fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut frame = Vec::new();
    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            return if frame.is_empty() {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Truncated frame",
                ))
            };
        }
        let end = buffer.iter().position(|&b| b == b'\n');
        let take = end.map_or(buffer.len(), |n| n + 1);
        if frame.len() + take > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Request exceeds frame limit",
            ));
        }
        frame.extend_from_slice(&buffer[..take]);
        reader.consume(take);
        if end.is_some() {
            return Ok(Some(frame));
        }
    }
}

fn main() {
    let output = Output(Arc::new(Mutex::new(io::stdout())));
    let registry = OperationRegistry::default();
    // A single codec operation bounds CPU/RAM use; control messages remain independent.
    let mut active: Option<(String, Arc<std::sync::atomic::AtomicBool>, JoinHandle<()>)> = None;
    let mut reader = io::stdin().lock();
    loop {
        let frame = match read_frame(&mut reader) {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(_) => {
                output.error(
                    "",
                    CommandError::new("invalid_frame", "Invalid or oversized frame"),
                );
                break;
            }
        };
        let request: Request = match serde_json::from_slice(&frame) {
            Ok(request) => request,
            Err(_) => {
                output.error(
                    "",
                    CommandError::new("invalid_request", "Invalid request schema"),
                );
                continue;
            }
        };
        if request.version != VERSION || request.id.is_empty() || request.id.len() > 128 {
            output.error(
                &request.id,
                CommandError::new("invalid_request", "Unsupported protocol or request ID"),
            );
            continue;
        }
        if active.as_ref().is_some_and(|(_, done, task)| {
            done.load(std::sync::atomic::Ordering::Acquire) || task.is_finished()
        }) {
            let _ = active.take().unwrap().2.join();
        }
        match request.command {
            Command::Hello => {
                let _ = output.send(&request.id, "complete", json!({
                "protocol_version": VERSION, "max_frame_bytes": MAX_FRAME,
                "commands": ["open", "extract", "test", "create", "edit", "cancel", "resolve_conflict", "shutdown"]
            }));
            }
            Command::Cancel { operation_id } => {
                let cancelled = registry.cancel(&operation_id);
                let _ = output.send(&request.id, "complete", json!({"cancelled": cancelled}));
            }
            Command::ResolveConflict {
                operation_id,
                conflict_id,
                decision,
                apply_to_all,
            } => {
                match registry.resolve_conflict(&operation_id, &conflict_id, decision, apply_to_all)
                {
                    Ok(()) => {
                        let _ = output.send(&request.id, "complete", json!({}));
                    }
                    Err(e) => output.error(&request.id, CommandError::new("invalid_operation", e)),
                }
            }
            Command::Shutdown => {
                if let Some((id, _, task)) = active.take() {
                    registry.cancel(&id);
                    let _ = task.join();
                }
                let _ = output.send(&request.id, "complete", json!({}));
                break;
            }
            command => {
                if active.is_some() {
                    output.error(
                        &request.id,
                        CommandError::new("busy", "Another archive operation is active"),
                    );
                    continue;
                }
                // Register before spawning: an immediate cancel must never miss its operation.
                let registration = match &command {
                    Command::Edit { path, .. } => registry.start_edit(&request.id, path),
                    _ => registry.start(&request.id),
                };
                let state = match registration {
                    Ok(state) => state,
                    Err(e) => {
                        output.error(&request.id, CommandError::new("operation_failed", e));
                        continue;
                    }
                };
                let reg = registry.clone();
                let out = output.clone();
                let id = request.id;
                let task_id = id.clone();
                let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let task_done = done.clone();
                let task = thread::spawn(move || {
                    let result = execute(command, &task_id, state, &reg, &out);
                    // Publish idle before the terminal frame: its receiver may immediately submit again.
                    task_done.store(true, std::sync::atomic::Ordering::Release);
                    match result {
                        Ok(value) => {
                            if out.send(&task_id, "complete", value).is_err() {
                                out.error(
                                    &task_id,
                                    output_error(io::Error::other("Response failed")),
                                );
                            }
                        }
                        Err(e) => out.error(&task_id, e),
                    }
                });
                active = Some((id, done, task));
            }
        }
    }
    if let Some((id, _, task)) = active {
        registry.cancel(&id);
        let _ = task.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn typed_response_preserves_protocol_envelope() {
        let entries = ["dir/file.txt", "кириллица.txt"];
        let frame = serde_json::to_vec(&Response {
            version: VERSION,
            id: "listing-1",
            event: "entries",
            data: &entries,
        })
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&frame).unwrap(),
            json!({"version": 1, "id": "listing-1", "event": "entries", "data": entries})
        );
    }

    #[test]
    fn framing_is_bounded_and_requires_newline() {
        assert_eq!(
            read_frame(&mut Cursor::new(b"{}\n{}\n")).unwrap().unwrap(),
            b"{}\n"
        );
        assert!(read_frame(&mut Cursor::new(vec![b'x'; MAX_FRAME + 1])).is_err());
        assert!(read_frame(&mut Cursor::new(b"{}")).is_err());
        assert!(read_frame(&mut Cursor::new(b"")).unwrap().is_none());
    }

    #[test]
    fn password_whitespace_is_data_and_version_is_required() {
        let r: Request = serde_json::from_str(
            r#"{"version":1,"id":"a","command":"open","path":"x","password":"  secret  "}"#,
        )
        .unwrap();
        assert!(matches!(r.command, Command::Open { password: Some(p), .. } if p == "  secret  "));
        assert!(serde_json::from_str::<Request>(r#"{"id":"a","command":"hello"}"#).is_err());
    }
}
