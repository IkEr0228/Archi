package archi;

import javafx.scene.Group;
import javafx.scene.paint.Color;
import javafx.scene.shape.SVGPath;

final class DotIcon extends Group {
    private final SVGPath glyph = new SVGPath();
    private String current;
    DotIcon() { getChildren().add(glyph); setMouseTransparent(true); }
    void set(ArchiveModel.Entry entry) {
        String ext = entry.nameLower().replaceFirst("^.*\\.", "");
        String type = entry.directory() ? "folder" : switch (ext) {
            case "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico" -> "image";
            case "rs", "java", "js", "ts", "md", "json", "xml", "css", "html", "py", "c", "cpp", "yml", "yaml" -> "code";
            case "zip", "7z", "rar", "tar", "gz", "xz", "bz2" -> "archive";
            default -> "file";
        };
        if (type.equals(current)) return;
        current = type;
        String[] rows = switch (type) {
            case "folder" -> new String[]{"01110000", "11111111", "10000001", "10000001", "10000001", "10000001", "11111111"};
            case "image" -> new String[]{"11111111", "10000011", "10100001", "11010001", "10001011", "10010101", "11111111"};
            case "code" -> new String[]{"11111111", "01000010", "10011001", "00100100", "10011001", "01000010", "11111111"};
            case "archive" -> new String[]{"11111111", "11011011", "11011011", "11100111", "11100111", "11011011", "11111111"};
            default -> new String[]{"11111100", "10000110", "10000010", "10000010", "10000010", "10000010", "11111110"};
        };
        var path = new StringBuilder();
        for (int y = 0; y < rows.length; y++) for (int x = 0; x < 8; x++) {
            if (rows[y].charAt(x) == '1') path.append("M").append(x * 2.5).append(' ').append(y * 2.5).append("h1.7v1.7h-1.7z");
        }
        glyph.setContent(path.toString());
        glyph.setFill(Color.web(switch (type) {
            case "folder", "archive" -> "#ffd5ae";
            case "image" -> "#ffbebe";
            case "code" -> "#9be9df";
            default -> "#c8b9b9";
        }));
    }
}
