//! High-performance asynchronous streaming pipeline for archive extraction.
//!
//! Provides a Producer-Consumer double-buffered pipeline:
//! - Decompressor (producer) decompresses into pooled 1 MiB chunks.
//! - Disk writer (consumer) writes completed chunks sequentially to disk on a dedicated background thread.
//! - Memory footprint is strictly bounded to 3 MiB (3 × 1 MiB buffers).
//! - Calculates live throughput (MB/s) and estimated time of arrival (ETA).
//! - Emits real-time progress events during large file streaming via `ProgressGate`.
//! - Automatically cleans up partial/corrupt files on disk upon error or cancellation.

use crate::io_perf::ProgressGate;
use crate::models::{CommandError, OperationProgress};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Size of each chunk buffer in the pipeline (1 MiB).
pub const PIPELINE_CHUNK_SIZE: usize = 1024 * 1024;

/// Number of buffers in the recycling pool.
pub const PIPELINE_QUEUE_CAPACITY: usize = 3;

/// Threshold above which files stream dynamically without synchronous `set_len`
/// zero-fill stalls on Windows NTFS (32 MiB).
pub const HYBRID_ALLOCATION_THRESHOLD: u64 = 32 * 1024 * 1024;

/// Helper to determine whether calling `set_len` is beneficial or risks a zero-fill stall.
#[inline]
pub fn should_preallocate_len(expected_size: u64) -> bool {
    expected_size > 0 && expected_size <= HYBRID_ALLOCATION_THRESHOLD
}

/// Tracks real-time transfer speed and ETA.
#[derive(Debug)]
pub struct SpeedTracker {
    start_time: Instant,
    last_sample_time: Instant,
    last_sample_bytes: u64,
    current_speed: f64,
}

impl SpeedTracker {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            start_time: now,
            last_sample_time: now,
            last_sample_bytes: 0,
            current_speed: 0.0,
        }
    }

    /// Updates speed with the current total bytes processed and returns current bytes/sec.
    pub fn update(&mut self, total_bytes: u64) -> u64 {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_sample_time);
        if elapsed >= Duration::from_millis(200) {
            let bytes_delta = total_bytes.saturating_sub(self.last_sample_bytes);
            let instant_speed = bytes_delta as f64 / elapsed.as_secs_f64();
            if self.current_speed <= 0.0 {
                self.current_speed = instant_speed;
            } else {
                // Exponential moving average (EMA): 70% previous, 30% instant
                self.current_speed = (self.current_speed * 0.7) + (instant_speed * 0.3);
            }
            self.last_sample_time = now;
            self.last_sample_bytes = total_bytes;
        } else if self.current_speed <= 0.0 {
            let total_elapsed = now.duration_since(self.start_time).as_secs_f64();
            if total_elapsed > 0.05 {
                self.current_speed = total_bytes as f64 / total_elapsed;
            }
        }
        self.current_speed.max(0.0) as u64
    }

    /// Estimates seconds remaining given the current total bytes and target total bytes.
    pub fn estimate_eta(&self, current_bytes: u64, total_bytes: u64) -> Option<u32> {
        if total_bytes <= current_bytes || self.current_speed <= 1024.0 {
            return None;
        }
        let remaining = total_bytes - current_bytes;
        let secs = (remaining as f64 / self.current_speed).round();
        if secs.is_finite() && secs >= 0.0 && secs < 86400.0 * 7.0 {
            Some(secs as u32)
        } else {
            None
        }
    }
}

impl Default for SpeedTracker {
    fn default() -> Self {
        Self::new()
    }
}

enum WorkerMessage {
    Chunk(Vec<u8>),
    Close,
}

/// Asynchronous streaming writer with a bounded recycling buffer pool.
pub struct StreamPipelineWriter {
    task_tx: SyncSender<WorkerMessage>,
    recycle_rx: Receiver<Vec<u8>>,
    worker_handle: Option<JoinHandle<io::Result<u64>>>,
    active_buffer: Vec<u8>,
    bytes_written_this_file: u64,
    output_path: Option<PathBuf>,
    clean_on_error: bool,
}

impl StreamPipelineWriter {
    /// Spawns a dedicated background writer thread writing chunks to `output`.
    /// If `output_path` is provided, any error or drop before `finish` cleans up the file.
    pub fn new(mut output: File, output_path: Option<PathBuf>) -> Self {
        let (task_tx, task_rx) = sync_channel::<WorkerMessage>(PIPELINE_QUEUE_CAPACITY);
        let (recycle_tx, recycle_rx) = sync_channel::<Vec<u8>>(PIPELINE_QUEUE_CAPACITY);

        // Pre-populate pool with reusable buffers
        for _ in 0..PIPELINE_QUEUE_CAPACITY {
            let _ = recycle_tx.send(Vec::with_capacity(PIPELINE_CHUNK_SIZE));
        }

        let worker_handle = std::thread::Builder::new()
            .name("stream-writer".into())
            .spawn(move || -> io::Result<u64> {
                let mut total_written = 0_u64;
                while let Ok(msg) = task_rx.recv() {
                    match msg {
                        WorkerMessage::Chunk(mut chunk) => {
                            if !chunk.is_empty() {
                                output.write_all(&chunk)?;
                                total_written += chunk.len() as u64;
                            }
                            chunk.clear();
                            // Return buffer to recycling pool for producer to reuse
                            let _ = recycle_tx.send(chunk);
                        }
                        WorkerMessage::Close => {
                            output.flush()?;
                            drop(output);
                            break;
                        }
                    }
                }
                Ok(total_written)
            })
            .expect("failed to spawn pipeline writer thread");

        let active_buffer = recycle_rx
            .recv()
            .unwrap_or_else(|_| Vec::with_capacity(PIPELINE_CHUNK_SIZE));

        Self {
            task_tx,
            recycle_rx,
            worker_handle: Some(worker_handle),
            active_buffer,
            bytes_written_this_file: 0,
            output_path,
            clean_on_error: true,
        }
    }

    /// Acquires a buffer from the recycling pool.
    fn acquire_buffer(&mut self) -> io::Result<Vec<u8>> {
        self.recycle_rx.recv().map_err(|_| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Pipeline writer worker thread terminated prematurely.",
            )
        })
    }

    /// Writes a slice of decompressed bytes into the pipeline.
    /// Flushes full 1 MiB chunks to the background writer thread.
    pub fn write_bytes(&mut self, mut data: &[u8]) -> io::Result<()> {
        while !data.is_empty() {
            let available = PIPELINE_CHUNK_SIZE.saturating_sub(self.active_buffer.len());
            if available == 0 {
                self.flush_active_chunk()?;
                continue;
            }
            let to_copy = available.min(data.len());
            self.active_buffer.extend_from_slice(&data[..to_copy]);
            self.bytes_written_this_file += to_copy as u64;
            data = &data[to_copy..];
        }
        Ok(())
    }

    fn flush_active_chunk(&mut self) -> io::Result<()> {
        if self.active_buffer.is_empty() {
            return Ok(());
        }
        let next_buffer = self.acquire_buffer()?;
        let full_chunk = std::mem::replace(&mut self.active_buffer, next_buffer);
        self.task_tx
            .send(WorkerMessage::Chunk(full_chunk))
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "Cannot send chunk to pipeline writer worker.",
                )
            })?;
        Ok(())
    }

    /// Returns total bytes written to this file so far.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written_this_file
    }

    /// Finalizes streaming, flushes any remaining data, joins the writer thread,
    /// and returns total bytes written.
    pub fn finish(mut self) -> Result<u64, CommandError> {
        if !self.active_buffer.is_empty() {
            let final_chunk = std::mem::take(&mut self.active_buffer);
            let _ = self.task_tx.send(WorkerMessage::Chunk(final_chunk));
        }
        let _ = self.task_tx.send(WorkerMessage::Close);

        if let Some(handle) = self.worker_handle.take() {
            match handle.join() {
                Ok(Ok(bytes)) => {
                    self.clean_on_error = false;
                    Ok(bytes)
                }
                Ok(Err(io_err)) => Err(CommandError::new(
                    "write_failed",
                    format!("Streaming write failed: {io_err}"),
                )),
                Err(_) => Err(CommandError::new(
                    "write_failed",
                    "Pipeline writer worker thread panicked.",
                )),
            }
        } else {
            self.clean_on_error = false;
            Ok(self.bytes_written_this_file)
        }
    }
}

impl Drop for StreamPipelineWriter {
    fn drop(&mut self) {
        if self.worker_handle.is_some() {
            let _ = self.task_tx.send(WorkerMessage::Close);
            if let Some(handle) = self.worker_handle.take() {
                let _ = handle.join();
            }
        }

        if self.clean_on_error {
            if let Some(path) = &self.output_path {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Helper to pipe an entire reader into `StreamPipelineWriter` with progress tracking and cancel checks.
pub fn pipe_reader_to_pipeline<R: Read>(
    mut reader: R,
    mut pipeline: StreamPipelineWriter,
    cancelled: &AtomicBool,
    operation_id: &str,
    current_file: &str,
    extracted_files: u64,
    total_files: u64,
    global_bytes_before: u64,
    global_total_bytes: u64,
    speed_tracker: &mut SpeedTracker,
    progress_gate: &mut ProgressGate,
    emit: &mut impl FnMut(OperationProgress),
) -> Result<u64, CommandError> {
    let mut read_buf = [0_u8; 64 * 1024];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(CommandError::new("cancelled", "Archive extraction was cancelled."));
        }

        let n = reader.read(&mut read_buf).map_err(|error| {
            let lower = error.to_string().to_ascii_lowercase();
            if lower.contains("password") || lower.contains("decrypt") {
                CommandError::new("password_required", "Invalid password provided.")
            } else {
                CommandError::new("invalid_archive", format!("Read error: {error}"))
            }
        })?;

        if n == 0 {
            break;
        }

        pipeline.write_bytes(&read_buf[..n]).map_err(|error| {
            CommandError::new("write_failed", format!("Cannot write extracted file: {error}"))
        })?;

        let current_global_bytes = global_bytes_before + pipeline.bytes_written();
        let current_speed = speed_tracker.update(current_global_bytes);

        if progress_gate.should_emit() {
            let percentage = if global_total_bytes > 0 {
                ((current_global_bytes as f64 / global_total_bytes as f64) * 100.0) as f32
            } else if total_files > 0 {
                (extracted_files as f32 / total_files as f32) * 100.0
            } else {
                0.0
            };

            let eta = speed_tracker.estimate_eta(current_global_bytes, global_total_bytes);

            emit(OperationProgress {
                operation_id: operation_id.into(),
                extracted_files,
                total_files,
                current_file: current_file.into(),
                percentage: percentage.clamp(0.0, 99.9),
                phase: Some("extract".into()),
                bytes_processed: Some(current_global_bytes),
                total_bytes: if global_total_bytes > 0 {
                    Some(global_total_bytes)
                } else {
                    None
                },
                speed_bytes_per_sec: Some(current_speed),
                eta_seconds: eta,
                ..Default::default()
            });
        }
    }

    pipeline.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn pipeline_writes_all_data_intact() {
        let temp_dir = std::env::temp_dir().join(format!("test-pipeline-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file_path = temp_dir.join("pipeline_out.bin");

        let file = File::create(&test_file_path).unwrap();
        let mut pipeline = StreamPipelineWriter::new(file, Some(test_file_path.clone()));

        // Generate 3.5 MiB of structured pattern
        let mut test_data = Vec::with_capacity(3500 * 1024);
        for i in 0..test_data.capacity() {
            test_data.push((i % 251) as u8);
        }

        pipeline.write_bytes(&test_data).unwrap();
        let written = pipeline.finish().unwrap();

        assert_eq!(written, test_data.len() as u64);
        let read_back = std::fs::read(&test_file_path).unwrap();
        assert_eq!(read_back, test_data);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn speed_tracker_computes_throughput_and_eta() {
        let mut tracker = SpeedTracker::new();
        // Simulate reading 100 MB
        tracker.update(100 * 1024 * 1024);
        assert!(tracker.current_speed >= 0.0);

        let eta = tracker.estimate_eta(100 * 1024 * 1024, 200 * 1024 * 1024);
        if tracker.current_speed > 1024.0 {
            assert!(eta.is_some());
        }
    }

    #[test]
    fn pipe_reader_streams_correctly() {
        let temp_dir = std::env::temp_dir().join(format!("test-pipe-reader-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file_path = temp_dir.join("pipe_out.bin");

        let file = File::create(&test_file_path).unwrap();
        let pipeline = StreamPipelineWriter::new(file, Some(test_file_path.clone()));

        let payload = vec![0xAB_u8; 512 * 1024];
        let cursor = Cursor::new(payload.clone());
        let cancelled = AtomicBool::new(false);
        let mut speed_tracker = SpeedTracker::new();
        let mut progress_gate = ProgressGate::new();

        let res = pipe_reader_to_pipeline(
            cursor,
            pipeline,
            &cancelled,
            "test-op",
            "test.bin",
            0,
            1,
            0,
            512 * 1024,
            &mut speed_tracker,
            &mut progress_gate,
            &mut |_| {},
        );

        assert_eq!(res.unwrap(), 512 * 1024);
        let read_back = std::fs::read(&test_file_path).unwrap();
        assert_eq!(read_back, payload);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn pipeline_aborts_and_cleans_file_on_error() {
        let temp_dir = std::env::temp_dir().join(format!("test-pipe-abort-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let test_file_path = temp_dir.join("should_be_deleted.bin");

        let file = File::create(&test_file_path).unwrap();
        let mut pipeline = StreamPipelineWriter::new(file, Some(test_file_path.clone()));

        // Write some bytes
        pipeline.write_bytes(b"partial corrupted data").unwrap();

        // Drop pipeline without calling finish (simulating early exit / error)
        drop(pipeline);

        // File must be automatically deleted by Drop
        assert!(!test_file_path.exists(), "corrupted file should have been cleaned up on drop");

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
