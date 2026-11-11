package archi;

import javafx.application.*;
import javafx.scene.Node;
import javafx.scene.control.*;
import javafx.scene.image.WritableImage;
import javafx.scene.input.KeyCode;
import javafx.scene.input.KeyEvent;
import javafx.scene.input.MouseEvent;
import javafx.scene.input.MouseButton;
import javafx.stage.*;
import java.awt.image.BufferedImage;
import java.nio.file.*;
import java.util.concurrent.*;
import java.util.function.Supplier;
import java.util.zip.*;
import javax.imageio.ImageIO;

/** Exercises the real application and its worker, without a test-only UI path. */
public final class FxSmokeTest {
    private static <T> T fx(Supplier<T> action) throws Exception {
        var future = new CompletableFuture<T>();
        Platform.runLater(() -> { try { future.complete(action.get()); } catch (Throwable e) { future.completeExceptionally(e); } });
        return future.get(5, TimeUnit.SECONDS);
    }
    private static Stage stage() { return (Stage)Window.getWindows().stream().filter(Window::isShowing).findFirst().orElseThrow(); }
    private static Button button(String text) {
        return stage().getScene().getRoot().lookupAll(".button").stream()
            .filter(n -> n instanceof Button b && b.getText().equals(text)).map(n -> (Button)n).findFirst().orElseThrow();
    }
    @SuppressWarnings("unchecked") private static TableView<ArchiveModel.Entry> table() { return (TableView<ArchiveModel.Entry>)stage().getScene().lookup(".table-view"); }
    private static boolean ready() { return !button("Create").isDisabled() && stage().getScene().lookup(".modal-overlay") == null; }
    private static boolean entry(String name) { return table().getItems().stream().anyMatch(e -> e.name().equals(name)); }
    private static void select(String name) { table().getSelectionModel().clearSelection(); table().getItems().stream().filter(e -> e.name().equals(name)).findFirst().ifPresent(e -> table().getSelectionModel().select(e)); }
    private static void escape() { stage().getScene().getRoot().fireEvent(new KeyEvent(KeyEvent.KEY_PRESSED, "", "", KeyCode.ESCAPE, false, false, false, false)); }
    private static void mouse(javafx.event.EventType<MouseEvent> type, double x, double y, double screenX, double screenY, boolean pressed) {
        stage().getScene().getRoot().fireEvent(new MouseEvent(type, x, y, screenX, screenY,
            pressed ? MouseButton.PRIMARY : MouseButton.NONE, pressed ? 1 : 0, false, false, false, false,
            pressed, false, false, false, false, false, null));
    }
    private static void assertResize() {
        double originX = stage().getX(), originY = stage().getY();
        for (int horizontal : new int[]{-1, 0, 1}) for (int vertical : new int[]{-1, 0, 1}) {
            if (horizontal == 0 && vertical == 0) continue;
            stage().setX(originX); stage().setY(originY); stage().setWidth(800); stage().setHeight(600);
            var scene = stage().getScene(); scene.getRoot().layout();
            double x = horizontal < 0 ? 2 : horizontal > 0 ? scene.getWidth() - 2 : scene.getWidth() / 2;
            double y = vertical < 0 ? 2 : vertical > 0 ? scene.getHeight() - 2 : scene.getHeight() / 2;
            mouse(MouseEvent.MOUSE_MOVED, x, y, originX + x, originY + y, false);
            assert scene.getCursor() != javafx.scene.Cursor.DEFAULT : "Missing edge cursor";
            mouse(MouseEvent.MOUSE_PRESSED, x, y, originX + x, originY + y, true);
            mouse(MouseEvent.MOUSE_DRAGGED, x, y, originX + x + horizontal * 20, originY + y + vertical * 20, true);
            assert Math.abs(stage().getWidth() - (horizontal == 0 ? 800 : 820)) < 1;
            assert Math.abs(stage().getHeight() - (vertical == 0 ? 600 : 620)) < 1;
            assert Math.abs(stage().getX() - (originX + (horizontal < 0 ? -20 : 0))) < 1;
            assert Math.abs(stage().getY() - (originY + (vertical < 0 ? -20 : 0))) < 1;
            mouse(MouseEvent.MOUSE_RELEASED, x, y, originX + x, originY + y, false);
        }
        stage().setX(originX); stage().setY(originY); stage().setWidth(800); stage().setHeight(600);
    }
    private static void assertModalLayout() {
        var scene = stage().getScene();
        scene.getRoot().applyCss(); scene.getRoot().layout();
        Node card = scene.lookup(".dialog"), heading = scene.lookup(".dialog-title"), confirm = button("Create archive");
        var cardBounds = card.localToScene(card.getLayoutBounds());
        var headingBounds = heading.localToScene(heading.getLayoutBounds());
        var confirmBounds = confirm.localToScene(confirm.getLayoutBounds());
        assert cardBounds.getMinY() >= 23 && cardBounds.getMaxY() <= scene.getHeight() - 23 : cardBounds;
        assert headingBounds.getMinY() > cardBounds.getMinY();
        assert confirmBounds.getMaxY() < cardBounds.getMaxY();
        ScrollPane scroll = (ScrollPane)scene.lookup(".dialog-scroll");
        scroll.setVvalue(1); scene.getRoot().layout();
        assert heading.localToScene(heading.getLayoutBounds()).equals(headingBounds) : "Heading scrolled";
        assert confirm.localToScene(confirm.getLayoutBounds()).equals(confirmBounds) : "Actions scrolled";
        assert ((TextArea)scene.lookup("#create-sources")).isWrapText();
        assert scene.lookup("#create-sources .scroll-bar:horizontal") == null
            || !scene.lookup("#create-sources .scroll-bar:horizontal").isVisible();
        scroll.setVvalue(0); scene.getRoot().layout();
    }
    private static void assertParityHooks() {
        var scene = stage().getScene();
        Node table = scene.lookup(".table-view");
        assert table.getAccessibleRole() == javafx.scene.AccessibleRole.TABLE_VIEW;
        assert table.getAccessibleText().equals("Archive entries");
        assert scene.getRoot().getOnDragOver() != null : "Missing archive drag-over handler";
        assert scene.getRoot().getOnDragDropped() != null : "Missing archive drop handler";
        assert scene.lookup(".titlebar").getAccessibleRole() == javafx.scene.AccessibleRole.TOOL_BAR;
        assert scene.getRoot().lookupAll(".window-button").stream()
            .anyMatch(n -> n instanceof Button b && "Close".equals(b.getAccessibleText()));
        assert scene.getRoot().lookupAll(".button").stream()
            .anyMatch(n -> n instanceof Button b && "New Window".equals(b.getText()));
    }
    private static void await(Supplier<Boolean> condition) throws Exception {
        long deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(20);
        while (!fx(condition)) { if (System.nanoTime() > deadline) throw new AssertionError("JavaFX state timeout"); Thread.sleep(50); }
    }
    private static void screenshot(String name) throws Exception {
        WritableImage image = fx(() -> stage().getScene().snapshot(null));
        BufferedImage png = new BufferedImage((int)image.getWidth(), (int)image.getHeight(), BufferedImage.TYPE_INT_ARGB);
        for (int y = 0; y < png.getHeight(); y++) for (int x = 0; x < png.getWidth(); x++) png.setRGB(x, y, image.getPixelReader().getArgb(x, y));
        ImageIO.write(png, "png", Path.of(System.getProperty("archi.smoke.output"), name).toFile());
    }
    public static void main(String[] args) throws Exception {
        boolean encrypted = args.length > 0;
        Path archive = Files.createTempFile("archi-fx-smoke-", ".zip");
        Path directory = Files.createTempDirectory("archi-fx-create-");
        Path source = directory.resolve("payload.txt"), created = directory.resolve("created.zip");
        Path added = directory.resolve("added.txt");
        Files.writeString(source, "Created from JavaFX"); Files.writeString(added, "Added from JavaFX");
        try (var zip = new ZipOutputStream(Files.newOutputStream(archive))) {
            for (String name : new String[]{"logs/", "logs/client.log", "readme.md", "config.json", "image.png"}) {
                zip.putNextEntry(new ZipEntry(name)); if (!name.endsWith("/")) zip.write("Archi smoke fixture".getBytes()); zip.closeEntry();
            }
        }
        var verdict = new CompletableFuture<Void>();
        Thread.ofPlatform().name("fx-smoke-driver").start(() -> {
            try {
                // launch() starts the toolkit asynchronously relative to this driver.
                for (int i = 0; i < 100; i++) {
                    try { fx(() -> true); break; } catch (IllegalStateException e) { Thread.sleep(50); }
                }
                await(() -> Window.getWindows().stream().anyMatch(Window::isShowing));
                if (encrypted) {
                    await(() -> stage().getScene().lookup(".password-field") != null);
                    await(() -> stage().getScene().getFocusOwner() instanceof PasswordField);
                    screenshot("smoke-password.png");
                    fx(() -> {
                        PasswordField field = (PasswordField)stage().getScene().lookup(".password-field");
                        field.setText("wrong");
                        stage().getScene().getRoot().lookupAll(".button").stream().filter(n -> n instanceof Button b && b.getText().equals("Confirm")).map(n -> (Button)n).findFirst().orElseThrow().fire();
                        return true;
                    });
                    await(() -> stage().getScene().lookup(".password-field") != null && stage().getScene().lookup(".error") != null);
                    fx(() -> {
                        ((PasswordField)stage().getScene().lookup(".password-field")).setText("password");
                        stage().getScene().getRoot().lookupAll(".button").stream().filter(n -> n instanceof Button b && b.getText().equals("Confirm")).map(n -> (Button)n).findFirst().orElseThrow().fire();
                        return true;
                    });
                    await(() -> stage().getScene().lookup(".modal-overlay") == null && ((TableView<?>)stage().getScene().lookup(".table-view")).getItems().size() == 1);
                    verdict.complete(null);
                    return;
                }
                await(() -> ((TableView<?>)stage().getScene().lookup(".table-view")).getItems().size() == 4);
                fx(() -> { assertResize(); return true; });
                fx(() -> { assertParityHooks(); return true; });
                screenshot("smoke-open.png");
                fx(() -> { ((TextField)stage().getScene().lookup(".filters .text-field")).setText("client.log"); return true; });
                await(() -> ((TableView<?>)stage().getScene().lookup(".table-view")).getItems().size() == 1);
                fx(() -> { ((TextField)stage().getScene().lookup(".filters .text-field")).clear(); return true; });
                await(() -> ((TableView<?>)stage().getScene().lookup(".table-view")).getItems().size() == 4);
                fx(() -> {
                    stage().getScene().getRoot().lookupAll(".button").stream().filter(n -> n instanceof Button b && b.getText().equals("Properties")).map(n -> (Button)n).findFirst().orElseThrow().fire();
                    return true;
                });
                await(() -> stage().getScene().lookup(".modal-overlay") != null);
                fx(() -> {
                    Node shell = stage().getScene().lookup(".shell");
                    assert shell.isDisabled() && shell.getEffect() instanceof javafx.scene.effect.GaussianBlur;
                    assert stage().getScene().lookup(".dialog").getEffect() != shell.getEffect();
                    ScrollPane scroll = (ScrollPane)stage().getScene().lookup(".dialog-scroll");
                    assert scroll.getViewportBounds().getHeight() >= scroll.getContent().getLayoutBounds().getHeight() - 1 : "Short properties dialog scrolls";
                    return true;
                });
                screenshot("smoke-modal.png");
                fx(() -> { button("Close").fire(); button("Create").fire(); return true; });
                await(() -> stage().getScene().lookup("#create-sources") != null);
                await(() -> stage().getScene().getFocusOwner() instanceof TextArea);
                fx(() -> {
                    ((TextArea)stage().getScene().lookup("#create-sources")).setText(source.toString());
                    ((TextField)stage().getScene().lookup("#create-output")).setText(created.toString());
                    assertModalLayout();
                    return true;
                });
                screenshot("smoke-create.png");
                fx(() -> { ((ScrollPane)stage().getScene().lookup(".dialog-scroll")).setVvalue(1); return true; });
                screenshot("smoke-create-footer.png");
                fx(() -> { stage().setWidth(720); stage().setHeight(480); return true; });
                await(() -> stage().getScene().getHeight() <= 480);
                fx(() -> { assertModalLayout(); return true; });
                screenshot("smoke-create-small.png");
                fx(() -> { stage().setWidth(800); stage().setHeight(600); return true; });
                await(() -> stage().getScene().getHeight() >= 600);
                fx(() -> { button("Create archive").fire(); return true; });
                await(() -> ready() && entry("payload.txt") && table().getItems().size() == 1);
                try (var zip = new ZipFile(created.toFile())) { assert zip.getEntry("payload.txt") != null; }
                fx(() -> { button("New Folder").fire(); return true; });
                await(() -> stage().getScene().lookup("#entry-name") != null);
                fx(() -> { ((TextField)stage().getScene().lookup("#entry-name")).setText("folder"); button("Confirm").fire(); return true; });
                await(() -> ready() && entry("folder"));
                fx(() -> { select("payload.txt"); button("Rename").fire(); return true; });
                await(() -> stage().getScene().lookup("#entry-name") != null);
                fx(() -> { ((TextField)stage().getScene().lookup("#entry-name")).setText("renamed.txt"); button("Confirm").fire(); return true; });
                await(() -> ready() && entry("renamed.txt") && !entry("payload.txt"));
                fx(() -> { button("Add").fire(); return true; });
                await(() -> stage().getScene().lookup("#add-sources") != null);
                fx(() -> { ((TextArea)stage().getScene().lookup("#add-sources")).setText(added.toString()); button("Add to archive").fire(); return true; });
                await(() -> ready() && entry("added.txt"));
                // Query and surviving selection must survive mutation refresh.
                fx(() -> { ((TextField)stage().getScene().lookup(".filters .text-field")).setText("renamed"); return true; });
                await(() -> ready() && table().getItems().size() == 1 && entry("renamed.txt"));
                fx(() -> { select("renamed.txt"); button("Compact").fire(); return true; });
                await(() -> stage().getScene().lookup(".dialog") != null);
                fx(() -> { button("Compact archive").fire(); return true; });
                await(() -> ready() && table().getItems().size() == 1 && !table().getSelectionModel().isEmpty());
                assert fx(() -> ((TextField)stage().getScene().lookup(".filters .text-field")).getText().equals("renamed"));
                fx(() -> { ((TextField)stage().getScene().lookup(".filters .text-field")).clear(); return true; });
                await(() -> ready() && table().getItems().size() == 3);
                fx(() -> { select("added.txt"); button("Delete").fire(); return true; });
                await(() -> stage().getScene().lookup(".dialog") != null);
                fx(() -> { button("Delete entries").fire(); return true; });
                await(() -> ready() && !entry("added.txt") && table().getItems().size() == 2);
                fx(() -> { select("renamed.txt"); button("Move").fire(); return true; });
                await(() -> stage().getScene().lookup("#move-destination") != null);
                fx(() -> {
                    @SuppressWarnings("unchecked") var destination = (ComboBox<String>)stage().getScene().lookup("#move-destination");
                    assert button("Move entries").isDisabled();
                    destination.setValue("folder"); button("Move entries").fire(); return true;
                });
                await(() -> ready() && !entry("renamed.txt") && table().getItems().size() == 1);
                fx(() -> { select("folder"); table().fireEvent(new KeyEvent(KeyEvent.KEY_PRESSED, "", "", KeyCode.ENTER, false, false, false, false)); return true; });
                await(() -> ready() && entry("renamed.txt"));
                fx(() -> { select("renamed.txt"); button("Compact").fire(); return true; });
                await(() -> stage().getScene().lookup(".dialog") != null);
                fx(() -> { button("Compact archive").fire(); return true; });
                await(() -> ready() && entry("renamed.txt") && !table().getSelectionModel().isEmpty());
                assert fx(() -> button("folder") != null) : "Refresh lost current folder";
                try (var zip = new ZipFile(created.toFile())) {
                    assert zip.getEntry("folder/") != null;
                    assert zip.getEntry("added.txt") == null;
                    assert zip.getEntry("renamed.txt") == null;
                    assert new String(zip.getInputStream(zip.getEntry("folder/renamed.txt")).readAllBytes(), java.nio.charset.StandardCharsets.UTF_8).equals("Created from JavaFX");
                }
                // Escape cancels dialog, restores focus and never submits a mutation.
                fx(() -> { button("Create").fire(); return true; });
                await(() -> stage().getScene().lookup("#create-sources") != null);
                fx(() -> { escape(); return true; });
                await(FxSmokeTest::ready);
                screenshot("smoke-edited.png");
                // Creation password opens 7z once; edits must prompt unless remembered.
                Path encrypted7z = directory.resolve("encrypted.7z");
                fx(() -> { button("Create").fire(); return true; });
                await(() -> stage().getScene().lookup("#create-sources") != null);
                fx(() -> {
                    ((TextArea)stage().getScene().lookup("#create-sources")).setText(source.toString());
                    ((TextField)stage().getScene().lookup("#create-output")).setText(encrypted7z.toString());
                    @SuppressWarnings("unchecked") var format = (ComboBox<String>)stage().getScene().lookup("#create-format");
                    format.setValue("7z"); ((PasswordField)stage().getScene().lookup("#create-password")).setText("  secret  ");
                    button("Create archive").fire(); return true;
                });
                await(() -> ready() && stage().getTitle().startsWith("encrypted.7z") && entry("payload.txt"));
                fx(() -> { button("New Folder").fire(); return true; });
                await(() -> stage().getScene().lookup("#entry-name") != null);
                fx(() -> { ((TextField)stage().getScene().lookup("#entry-name")).setText("private-folder"); button("Confirm").fire(); return true; });
                await(() -> stage().getScene().lookup(".password-field") != null);
                fx(() -> { ((PasswordField)stage().getScene().lookup(".password-field")).setText("wrong"); button("Confirm").fire(); return true; });
                await(() -> stage().getScene().lookup(".password-field") != null && stage().getScene().lookup(".error") != null);
                fx(() -> {
                    ((PasswordField)stage().getScene().lookup(".password-field")).setText("  secret  ");
                    ((CheckBox)stage().getScene().lookup(".check-box")).setSelected(true);
                    button("Confirm").fire(); return true;
                });
                await(() -> ready() && entry("private-folder"));
                fx(() -> { button("Compact").fire(); return true; });
                await(() -> stage().getScene().lookup(".dialog") != null);
                fx(() -> { button("Compact archive").fire(); return true; });
                await(() -> ready() && entry("private-folder"));
                // ZIP listing needs no password, but mutation controls must remain disabled.
                fx(() -> { button("Create").fire(); return true; });
                await(() -> stage().getScene().lookup("#create-sources") != null);
                fx(() -> {
                    ((TextArea)stage().getScene().lookup("#create-sources")).setText(source.toString());
                    ((TextField)stage().getScene().lookup("#create-output")).setText(directory.resolve("encrypted.zip").toString());
                    ((PasswordField)stage().getScene().lookup("#create-password")).setText("secret");
                    button("Create archive").fire(); return true;
                });
                await(() -> ready() && stage().getTitle().startsWith("encrypted.zip") && entry("payload.txt"));
                assert fx(() -> button("New Folder").isDisabled() && button("Compact").isDisabled() && button("Add").isDisabled());
                verdict.complete(null);
            } catch (Throwable e) { verdict.completeExceptionally(e); }
            finally { Platform.runLater(Platform::exit); }
        });
        System.setProperty("archi.path", encrypted ? args[0] : archive.toString());
        Application.launch(ArchiApp.class, args);
        try { verdict.get(30, TimeUnit.SECONDS); System.out.println(encrypted ? "JavaFX RAR password/focus/retry smoke assertions passed." : "JavaFX layout/resize/create/edit/encryption smoke assertions passed."); }
        finally {
            Files.deleteIfExists(archive);
            try (var paths = Files.walk(directory)) { for (Path path : paths.sorted(java.util.Comparator.reverseOrder()).toList()) Files.deleteIfExists(path); }
        }
    }
}
