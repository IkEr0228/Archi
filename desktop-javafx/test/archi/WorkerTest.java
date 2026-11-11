package archi;

import com.google.gson.*;
import java.nio.file.*;
import java.util.concurrent.*;
import java.util.zip.*;

public final class WorkerTest {
    public static void main(String[] args) throws Exception {
        Path fixture = Files.createTempFile("archi-java-worker-", ".zip");
        Path output = Files.createTempDirectory("archi-java-worker-out-");
        try {
            try (var zip = new ZipOutputStream(Files.newOutputStream(fixture))) {
                for (int i = 0; i < 600; i++) {
                    zip.putNextEntry(new ZipEntry("file-" + i + ".txt"));
                    zip.write(("content-" + i).getBytes(java.nio.charset.StandardCharsets.UTF_8));
                    zip.closeEntry();
                }
            }
            try (var client = new WorkerClient(Path.of(System.getProperty("archi.worker")))) {
                assert client.request("hello", new JsonObject()).result().get(10, TimeUnit.SECONDS).get("protocol_version").getAsInt() == 1;
                var arguments = new JsonObject(); arguments.addProperty("path", fixture.toString());
                var archive = new ArchiveModel(client.request("open", arguments).result().get(10, TimeUnit.SECONDS));
                assert archive.entries.size() == 600;
                arguments.addProperty("destination", output.toString());
                var selected = new JsonArray(); selected.add("file-599.txt"); arguments.add("selected_paths", selected);
                client.request("extract", arguments).result().get(10, TimeUnit.SECONDS);
                assert Files.readString(output.resolve("file-599.txt")).equals("content-599");
                arguments = new JsonObject(); arguments.addProperty("path", fixture.toString());
                assert client.request("test", arguments).result().get(10, TimeUnit.SECONDS).get("tested_ok").getAsInt() == 600;
                for (int i = 0; i < 100; i++) {
                    var missing = new JsonObject(); missing.addProperty("path", fixture + ".missing");
                    try { client.request("open", missing).result().get(10, TimeUnit.SECONDS); throw new AssertionError("Missing archive accepted"); }
                    catch (ExecutionException e) { assert e.getCause() instanceof WorkerClient.WorkerError w && w.code.equals("not_found"); }
                }
            }
            System.out.println("Java → Rust process integration assertions passed.");
        } finally {
            Files.deleteIfExists(output.resolve("file-599.txt")); Files.deleteIfExists(output); Files.deleteIfExists(fixture);
        }
    }
}
