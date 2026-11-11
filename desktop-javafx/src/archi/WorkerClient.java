package archi;

import com.google.gson.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.Map;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Consumer;

/** One worker per application. Pipe IO and JSON parsing never run on FX thread. */
final class WorkerClient implements AutoCloseable {
    static final int MAX_FRAME = 1024 * 1024;
    private final Process process;
    private final Map<String, Pending> pending = new ConcurrentHashMap<>();
    private final ExecutorService writer = Executors.newSingleThreadExecutor(Thread.ofVirtual().name("worker-writer").factory());
    private final AtomicLong ids = new AtomicLong();
    private volatile boolean closed;
    private boolean failed;

    record Call(String id, CompletableFuture<JsonObject> result) {}
    private static final class Pending {
        final CompletableFuture<JsonObject> future = new CompletableFuture<>();
        final Consumer<JsonObject> events;
        JsonObject archive;
        final JsonArray entries = new JsonArray();
        Pending(Consumer<JsonObject> events) { this.events = events; }
    }
    static final class WorkerError extends RuntimeException {
        final String code;
        WorkerError(JsonObject data) {
            super(ArchiveModel.string(data, "message", "Worker failed"));
            code = ArchiveModel.string(data, "code", "worker_failed");
        }
    }

    WorkerClient(Path executable) throws IOException {
        process = new ProcessBuilder(executable.toAbsolutePath().toString()).redirectError(ProcessBuilder.Redirect.INHERIT).start();
        Thread.ofVirtual().name("worker-reader").start(this::read);
    }

    synchronized Call request(String command, JsonObject arguments, Consumer<JsonObject> events) {
        String id = Long.toString(ids.incrementAndGet());
        Pending p = new Pending(events);
        if (closed || failed || !process.isAlive() || pending.size() >= 16) {
            p.future.completeExceptionally(new IOException("Worker unavailable or request queue full"));
            return new Call(id, p.future);
        }
        JsonObject message = arguments.deepCopy();
        message.addProperty("version", 1);
        message.addProperty("id", id);
        message.addProperty("command", command);
        byte[] frame = (message + "\n").getBytes(StandardCharsets.UTF_8);
        if (frame.length > MAX_FRAME) {
            p.future.completeExceptionally(new IOException("Request exceeds frame limit"));
            return new Call(id, p.future);
        }
        pending.put(id, p);
        writer.execute(() -> {
            try {
                process.getOutputStream().write(frame);
                process.getOutputStream().flush();
            } catch (IOException e) { failAll(e); }
            finally { java.util.Arrays.fill(frame, (byte) 0); }
        });
        return new Call(id, p.future);
    }

    Call request(String command, JsonObject arguments) { return request(command, arguments, event -> {}); }

    private void read() {
        try (var input = new BufferedInputStream(process.getInputStream())) {
            byte[] bytes;
            while ((bytes = readFrame(input)) != null) {
                JsonObject message = JsonParser.parseString(new String(bytes, StandardCharsets.UTF_8)).getAsJsonObject();
                if (message.get("version").getAsInt() != 1) throw new IOException("Worker protocol mismatch");
                String id = message.get("id").getAsString();
                Pending p = pending.get(id);
                if (p == null) {
                    if (id.isEmpty()) throw new IOException("Worker rejected request framing/schema");
                    continue;
                }
                JsonElement data = message.get("data");
                switch (message.get("event").getAsString()) {
                    case "archive" -> p.archive = data.getAsJsonObject();
                    case "entries" -> p.entries.addAll(data.getAsJsonArray());
                    case "complete" -> {
                        pending.remove(id);
                        JsonObject result = p.archive == null ? data.getAsJsonObject() : p.archive;
                        if (p.archive != null) result.add("entries", p.entries);
                        p.future.complete(result);
                    }
                    case "error" -> { pending.remove(id); p.future.completeExceptionally(new WorkerError(data.getAsJsonObject())); }
                    default -> p.events.accept(message);
                }
            }
            failAll(new EOFException("Rust worker disconnected"));
        } catch (Exception e) { failAll(e); }
    }

    static byte[] readFrame(InputStream input) throws IOException {
        var frame = new ByteArrayOutputStream();
        for (int b; (b = input.read()) != -1;) {
            if (frame.size() >= MAX_FRAME) throw new IOException("Worker frame too large");
            if (b == '\n') return frame.toByteArray();
            frame.write(b);
        }
        if (frame.size() != 0) throw new EOFException("Truncated worker frame");
        return null;
    }

    private void failAll(Throwable error) {
        Pending[] abandoned;
        synchronized (this) {
            failed = true;
            abandoned = pending.values().toArray(Pending[]::new);
            pending.clear();
        }
        for (Pending p : abandoned) p.future.completeExceptionally(error);
        if (process.isAlive()) process.destroy();
    }

    @Override public synchronized void close() {
        if (closed) return;
        var shutdown = request("shutdown", new JsonObject()).result();
        closed = true;
        // Keep JVM alive until bounded worker cleanup finishes; virtual threads are daemon threads.
        Thread.ofPlatform().daemon(false).name("worker-shutdown").start(() -> {
            try { shutdown.get(5, TimeUnit.SECONDS); process.waitFor(2, TimeUnit.SECONDS); }
            catch (Exception ignored) { /* A wedged codec must not orphan a worker indefinitely. */ }
            finally {
                if (process.isAlive()) process.destroy();
                try { if (!process.waitFor(2, TimeUnit.SECONDS)) process.destroyForcibly(); }
                catch (InterruptedException e) { Thread.currentThread().interrupt(); }
                writer.shutdownNow();
                failAll(new IOException("Worker closed"));
            }
        });
    }
}
