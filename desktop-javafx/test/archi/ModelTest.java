package archi;

import com.google.gson.*;
import java.io.*;
import java.nio.charset.StandardCharsets;
import java.util.*;

public final class ModelTest {
    public static void main(String[] args) throws Exception {
        var data = JsonParser.parseString("""
            {"archive_path":"x.zip","entries":[
              {"path":"empty","name":"empty","parent_path":"/","is_directory":true,"uncompressed_size":0},
              {"path":"dir/a.md","name":"a.md","parent_path":"dir","is_directory":false,"uncompressed_size":12,"compressed_size":6},
              {"path":"dir","name":"dir","parent_path":"/","is_directory":true,"uncompressed_size":0},
              {"path":"Ж.txt","name":"Ж.txt","parent_path":"/","is_directory":false,"uncompressed_size":5}
            ]}
            """).getAsJsonObject();
        var model = new ArchiveModel(data);
        var order = ArchiveModel.order("name", false);
        assert model.query("/", "", "All", "", order).size() == 3;
        assert model.query("empty", "", "All", "", order).isEmpty();
        assert model.query("empty", "A.MD", "All", "", order).size() == 1;
        assert model.query("/", "", "Files", ".MD", order).getFirst().path().equals("dir/a.md");
        assert model.query("/", "", "Files", " md ", order).getFirst().path().equals("dir/a.md");
        assert model.query("/", "ж", "All", "", order).size() == 1;
        assert model.query("dir", "", "All", "", order).getFirst().compressed() == 6;
        assert model.children.get("/").size() == 3;
        assert ArchiveModel.bytes(1024).equals("1.00 KB");
        assert ArchiveModel.normalizeParent(null).equals("/");
        assert ArchiveModel.normalizeParent("").equals("/");
        assert ArchiveModel.normalizeParent("dir///").equals("dir");
        assert ArchiveModel.normalizeParent("///").equals("/");
        Thread.currentThread().interrupt();
        try { assert model.query("/", "", "All", "", order).isEmpty(); }
        finally { Thread.interrupted(); }
        var input = new ByteArrayInputStream("{}\n{}\n".getBytes(StandardCharsets.UTF_8));
        assert new String(WorkerClient.readFrame(input), StandardCharsets.UTF_8).equals("{}");
        assert WorkerClient.readFrame(new ByteArrayInputStream(new byte[0])) == null;
        try { WorkerClient.readFrame(new ByteArrayInputStream(new byte[WorkerClient.MAX_FRAME + 1])); throw new AssertionError("Frame limit missing"); }
        catch (IOException expected) {}
        try { WorkerClient.readFrame(new ByteArrayInputStream(new byte[]{'{'})); throw new AssertionError("Truncated frame accepted"); }
        catch (EOFException expected) {}
        System.out.println("Java model/query/framing assertions passed.");
    }
}
