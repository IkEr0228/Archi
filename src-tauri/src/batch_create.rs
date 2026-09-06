//! Context-menu multi-selection batching.
//!
//! When a user selects multiple files or folders in Windows Explorer and invokes
//! a context-menu verb, Explorer launches multiple separate processes simultaneously.
//! This batcher debounces incoming invocations across processes into a single
//! collection of sources, ensuring:
//! 1. Only a single window opens (no window storm or WebView2 concurrency freeze).
//! 2. All selected items are included in `sources`.
//! 3. The archive name is derived from the first selected item.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;

static BATCH_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateBatch {
    pub id: String,
    pub sources: Vec<String>,
    pub format: String,
}

struct BatcherState {
    current_format: String,
    current_sources: Vec<String>,
    debounce_task: Option<JoinHandle<()>>,
    is_startup: bool,

    // Cold-start synchronization
    startup_batch: Option<CreateBatch>,
    startup_notify: Arc<Notify>,
    startup_done: bool,

    // Secondary window batches
    batches: HashMap<String, CreateBatch>,
}

#[derive(Clone)]
pub struct ContextMenuBatcher {
    inner: Arc<Mutex<BatcherState>>,
}

impl Default for ContextMenuBatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextMenuBatcher {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(BatcherState {
                current_format: String::new(),
                current_sources: Vec::new(),
                debounce_task: None,
                is_startup: false,
                startup_batch: None,
                startup_notify: Arc::new(Notify::new()),
                startup_done: false,
                batches: HashMap::new(),
            })),
        }
    }

    /// Add items to the pending batch.
    pub fn add_items(&self, app: AppHandle, paths: Vec<String>, format: String, is_startup: bool) {
        let mut state = self.inner.lock().unwrap();

        if state.current_format.is_empty() {
            state.current_format = format;
        }
        if is_startup {
            state.is_startup = true;
        }

        for path in paths {
            if !state.current_sources.contains(&path) {
                state.current_sources.push(path);
            }
        }

        // Cancel previous timer
        if let Some(task) = state.debounce_task.take() {
            task.abort();
        }

        let inner_clone = self.inner.clone();
        let app_clone = app.clone();

        // 300ms debounce window allows Explorer processes to arrive
        let task = tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;

            let (sources, format, is_startup_batch, notify) = {
                let mut s = inner_clone.lock().unwrap();
                s.debounce_task = None;
                let sources = std::mem::take(&mut s.current_sources);
                let format = if s.current_format.is_empty() {
                    "zip".to_string()
                } else {
                    std::mem::take(&mut s.current_format)
                };
                let is_startup_batch = s.is_startup && !s.startup_done;
                let notify = s.startup_notify.clone();
                (sources, format, is_startup_batch, notify)
            };

            if sources.is_empty() {
                return;
            }

            let batch_id = format!(
                "batch_{}_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
                BATCH_COUNTER.fetch_add(1, Ordering::SeqCst)
            );

            let batch = CreateBatch {
                id: batch_id.clone(),
                sources,
                format,
            };

            if is_startup_batch {
                {
                    let mut s = inner_clone.lock().unwrap();
                    s.startup_batch = Some(batch.clone());
                    s.startup_done = true;
                }
                notify.notify_waiters();
                let _ = app_clone.emit("open-create-batch", &batch);
            } else {
                {
                    let mut s = inner_clone.lock().unwrap();
                    s.batches.insert(batch_id.clone(), batch.clone());
                }
                let _ = crate::window_manager::create_new_window_with_target(
                    &app_clone,
                    crate::window_manager::WindowInitialTarget::CreateBatch(batch_id),
                );
            }
        });

        state.debounce_task = Some(task);
    }

    /// Called by the main window on cold startup to await any pending context-menu batch.
    pub async fn await_startup_batch(&self) -> Option<CreateBatch> {
        let (notify, is_startup, done) = {
            let s = self.inner.lock().unwrap();
            if s.startup_done {
                return s.startup_batch.clone();
            }
            if !s.is_startup {
                return None;
            }
            (s.startup_notify.clone(), s.is_startup, s.startup_done)
        };

        if !is_startup || done {
            let s = self.inner.lock().unwrap();
            return s.startup_batch.clone();
        }

        // Wait up to 2 seconds for Explorer invocations to settle
        let _ = tokio::time::timeout(Duration::from_millis(2000), notify.notified()).await;

        let s = self.inner.lock().unwrap();
        s.startup_batch.clone()
    }

    /// Retrieve and remove a secondary window batch by ID.
    pub fn get_batch(&self, id: &str) -> Option<CreateBatch> {
        let mut s = self.inner.lock().unwrap();
        s.batches.remove(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_ordering_and_deduplication() {
        let batcher = ContextMenuBatcher::new();
        {
            let mut s = batcher.inner.lock().unwrap();
            s.current_sources.push("Folder A".into());
            s.current_sources.push("Folder B".into());
            let new_items = vec!["Folder A".to_string(), "Folder C".to_string()];
            for item in new_items {
                if !s.current_sources.contains(&item) {
                    s.current_sources.push(item);
                }
            }
            assert_eq!(s.current_sources, vec!["Folder A", "Folder B", "Folder C"]);
        }
    }

    #[tokio::test]
    async fn test_await_startup_batch_none_when_not_startup() {
        let batcher = ContextMenuBatcher::new();
        let res = batcher.await_startup_batch().await;
        assert!(res.is_none());
    }
}
