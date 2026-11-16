package archi;

import com.google.gson.*;
import java.util.*;

final class ArchiveModel {
    record Entry(String path, String name, String parent, boolean directory, long size,
                 Long compressed, String modified, String nameLower, String pathLower) {
        static Entry from(JsonObject o) {
            String path = string(o, "path", "");
            String name = string(o, "name", path);
            return new Entry(path, name, normalizeParent(string(o, "parent_path", "/")),
                o.get("is_directory").getAsBoolean(), o.get("uncompressed_size").getAsLong(),
                o.has("compressed_size") && !o.get("compressed_size").isJsonNull() ? o.get("compressed_size").getAsLong() : null,
                string(o, "modified_at", "—"), name.toLowerCase(Locale.ROOT), path.toLowerCase(Locale.ROOT));
        }
    }
    final String path;
    final JsonObject metadata;
    final List<Entry> entries;
    final Map<String, List<Entry>> children;

    ArchiveModel(JsonObject data) {
        metadata = data;
        path = string(data, "archive_path", "");
        var list = new ArrayList<Entry>();
        var index = new HashMap<String, List<Entry>>();
        for (JsonElement value : data.getAsJsonArray("entries")) {
            var entry = Entry.from(value.getAsJsonObject());
            list.add(entry);
            index.computeIfAbsent(entry.parent(), ignored -> new ArrayList<>()).add(entry);
        }
        entries = List.copyOf(list);
        index.replaceAll((key, value) -> List.copyOf(value));
        children = Map.copyOf(index);
        // Keep entry data once; metadata has only the small archive envelope.
        data.remove("entries");
    }

    List<Entry> query(String parent, String query, String type, String extension, Comparator<Entry> order) {
        String q = query.strip().toLowerCase(Locale.ROOT);
        String ext = extension.strip().toLowerCase(Locale.ROOT).replaceFirst("^\\.", "");
        String extensionSuffix = "." + ext;
        boolean wholeArchive = !q.isEmpty() || !type.equals("All") || !ext.isEmpty();
        var source = wholeArchive ? entries : children.getOrDefault(normalizeParent(parent), List.of());
        var result = new ArrayList<Entry>();
        for (Entry e : source) {
            if (Thread.currentThread().isInterrupted()) return List.of();
            if (!q.isEmpty() && !e.nameLower().contains(q) && !e.pathLower().contains(q)) continue;
            if (type.equals("Files") && e.directory() || type.equals("Folders") && !e.directory()) continue;
            if (!ext.isEmpty() && (e.directory() || !e.nameLower().endsWith(extensionSuffix))) continue;
            result.add(e);
        }
        if (result.size() > 1) result.sort(order);
        return result;
    }

    static Comparator<Entry> order(String field, boolean descending) {
        Comparator<Entry> value = switch (field) {
            case "size" -> Comparator.comparingLong(Entry::size);
            case "compressed" -> Comparator.comparing(Entry::compressed, Comparator.nullsFirst(Long::compare));
            case "ratio" -> Comparator.comparingDouble(ArchiveModel::ratio);
            case "modified" -> Comparator.comparing(Entry::modified);
            default -> Comparator.comparing(Entry::nameLower).thenComparing(Entry::name);
        };
        if (descending) value = value.reversed();
        return Comparator.comparing((Entry e) -> !e.directory()).thenComparing(value).thenComparing(Entry::path);
    }
    static double ratio(Entry e) { return e.size() == 0 || e.compressed() == null ? 0 : 100.0 * (1 - (double)e.compressed() / e.size()); }
    static String normalizeParent(String path) {
        if (path == null || path.isEmpty()) return "/";
        int end = path.length();
        while (end > 0 && path.charAt(end - 1) == '/') end--;
        return end == 0 ? "/" : path.substring(0, end);
    }
    static String string(JsonObject o, String key, String fallback) { return o.has(key) && !o.get(key).isJsonNull() ? o.get(key).getAsString() : fallback; }
    static String bytes(long n) {
        if (n < 1024) return n + " B";
        int unit = Math.min(4, (int)(Math.log(n) / Math.log(1024)));
        return String.format(Locale.ROOT, "%.2f %s", n / Math.pow(1024, unit), new String[]{"B", "KB", "MB", "GB", "TB"}[unit]);
    }
}
