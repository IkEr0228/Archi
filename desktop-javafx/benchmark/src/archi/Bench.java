package archi;

import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import java.util.*;
import java.util.function.Supplier;

public final class Bench {
    private static final int TOTAL = 50_000;
    private static final int FOLDERS = 500;
    private static final int FILES_PER_FOLDER = 99;
    private static final int WARMUPS = 10;
    private static final int SAMPLES = 20;

    public static void main(String[] args) {
        if (args.length != 1) throw new IllegalArgumentException("usage: Bench <baseline|current>");
        var model = new ArchiveModel(data());
        if (model.entries.size() != TOTAL) throw new AssertionError("entry count: " + model.entries.size());
        var extensionOrder = ArchiveModel.order("name", false);
        var folderOrder = ArchiveModel.order("size", true);
        Supplier<List<ArchiveModel.Entry>> extension = () -> model.query("/", "", "Files", ".JaVa", extensionOrder);
        Supplier<List<ArchiveModel.Entry>> folder = () -> model.query("folder-000", "", "All", "", folderOrder);
        if (extension.get().size() != 12_375) throw new AssertionError("extension count: " + extension.get().size());
        if (folder.get().size() != FILES_PER_FOLDER) throw new AssertionError("folder count: " + folder.get().size());
        for (int i = 0; i < WARMUPS; i++) { extension.get(); folder.get(); }
        var extensionMs = new double[SAMPLES];
        var folderMs = new double[SAMPLES];
        for (int i = 0; i < SAMPLES; i++) {
            long start = System.nanoTime();
            var result = extension.get();
            extensionMs[i] = elapsedMs(start);
            if (result.size() != 12_375) throw new AssertionError("extension count changed");
            start = System.nanoTime();
            result = folder.get();
            folderMs[i] = elapsedMs(start);
            if (result.size() != FILES_PER_FOLDER) throw new AssertionError("folder count changed");
        }
        System.out.printf(Locale.ROOT,
            "{\"variant\":\"%s\",\"entries\":%d,\"extension_count\":%d,\"folder_count\":%d,\"warmups\":%d,\"samples\":%d,\"extension_ms\":%s,\"folder_ms\":%s}%n",
            args[0], model.entries.size(), extension.get().size(), folder.get().size(), WARMUPS, SAMPLES,
            array(extensionMs), array(folderMs));
    }

    private static double elapsedMs(long start) { return (System.nanoTime() - start) / 1_000_000.0; }
    private static String array(double[] values) {
        var out = new StringBuilder("[");
        for (int i = 0; i < values.length; i++) { if (i > 0) out.append(','); out.append(String.format(Locale.ROOT, "%.6f", values[i])); }
        return out.append(']').toString();
    }
    private static JsonObject data() {
        var root = JsonParser.parseString("{\"archive_path\":\"bench.zip\",\"entries\":[]}").getAsJsonObject();
        var entries = root.getAsJsonArray("entries");
        for (int folder = 0; folder < FOLDERS; folder++) {
            String folderName = String.format(Locale.ROOT, "folder-%03d", folder);
            entries.add(entry(folderName, folderName, "/", true, 0, null));
            for (int file = 0; file < FILES_PER_FOLDER; file++) {
                int index = folder * FILES_PER_FOLDER + file;
                String extension = index % 4 == 0 ? "JaVa" : "txt";
                String name = String.format(Locale.ROOT, "MiXeD-%05d.%s", index, extension);
                entries.add(entry(folderName + "/" + name, name, folderName, false, 1000L + index, 500L + index));
            }
        }
        return root;
    }
    private static JsonObject entry(String path, String name, String parent, boolean directory, long size, Long compressed) {
        var o = new JsonObject(); o.addProperty("path", path); o.addProperty("name", name); o.addProperty("parent_path", parent);
        o.addProperty("is_directory", directory); o.addProperty("uncompressed_size", size);
        if (compressed == null) o.add("compressed_size", null); else o.addProperty("compressed_size", compressed);
        return o;
    }
}
