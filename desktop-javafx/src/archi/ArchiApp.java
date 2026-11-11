package archi;

import com.google.gson.*;
import javafx.application.*;
import javafx.beans.property.ReadOnlyObjectWrapper;
import javafx.collections.FXCollections;
import javafx.geometry.*;
import javafx.scene.*;
import javafx.scene.control.*;
import javafx.scene.effect.GaussianBlur;
import javafx.scene.input.*;
import javafx.scene.layout.*;
import javafx.stage.*;
import java.nio.file.*;
import java.util.*;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicLong;
import java.util.function.Consumer;

public final class ArchiApp extends Application {
    private Stage stage;
    private WorkerClient worker;
    private final BorderPane content = new BorderPane();
    private final StackPane root = new StackPane(content);
    private final TableView<ArchiveModel.Entry> table = new TableView<>();
    private final TextField search = new TextField(), extension = new TextField();
    private final ComboBox<String> type = new ComboBox<>(FXCollections.observableArrayList("All", "Files", "Folders"));
    private final Label opened = new Label("No archive loaded"), loaded = new Label(), status = new Label("Ready."), totals = new Label();
    private final HBox breadcrumbs = new HBox(9);
    private final ProgressBar progress = new ProgressBar();
    private final Button extract = button("Extract ▾", this::extract), test = button("Test", this::test), cancel = button("Cancel", this::cancel);
    private final List<Control> archiveControls = new ArrayList<>();
    private final List<Button> editControls = new ArrayList<>();
    private Button create, rename, delete, replace, move;
    private final ComboBox<String> strategy = new ComboBox<>(FXCollections.observableArrayList("Auto", "Prefer fast", "Prefer compact"));
    private final ExecutorService queries = Executors.newSingleThreadExecutor(Thread.ofVirtual().name("archive-query").factory());
    private final AtomicLong queryRevision = new AtomicLong();
    private ArchiveModel archive;
    private String parent = "/", secret, operation;
    private boolean busy, closed, riskAcknowledged;
    private long sessionRevision;
    private Future<?> queryTask;
    private StackPane modal;
    private Node previousFocus;
    private double dragX, dragY;
    private final java.util.concurrent.atomic.AtomicReference<JsonObject> latestProgress = new java.util.concurrent.atomic.AtomicReference<>();
    private final java.util.concurrent.atomic.AtomicBoolean progressQueued = new java.util.concurrent.atomic.AtomicBoolean();

    @Override public void start(Stage primary) {
        stage = primary;
        stage.initStyle(StageStyle.UNDECORATED);
        stage.setTitle("Archi // archive_manager");
        content.getStyleClass().add("shell");
        content.setTop(new VBox(titlebar(), toolbar(), filters(), breadcrumbBar()));
        content.setCenter(archiveTable());
        content.setBottom(statusbar());
        Scene scene = new Scene(root, 800, 600);
        scene.getStylesheets().add(Objects.requireNonNull(getClass().getResource("/archi.css")).toExternalForm());
        scene.addEventFilter(KeyEvent.KEY_PRESSED, this::key);
        stage.setScene(scene);
        stage.setMinWidth(720); stage.setMinHeight(480);
        installResize(scene);
        stage.setOnCloseRequest(e -> { if (busy) { e.consume(); confirmClose(); } });
        stage.show();
        controls();
        try {
            worker = new WorkerClient(Path.of(System.getProperty("archi.worker", "target/debug/archi-worker.exe")));
            worker.request("hello", new JsonObject()).result().whenComplete((hello, error) -> Platform.runLater(() -> {
                if (error != null) { showError(error); return; }
                controls();
                var args = getParameters().getRaw();
                if (!args.isEmpty() && !args.getFirst().isBlank()) open(args.getFirst(), null, false);
            }));
        } catch (Exception e) { showError(e); }
    }

    private Node titlebar() {
        Label brand = new Label("• Archi // archive_manager"); brand.getStyleClass().add("brand");
        opened.getStyleClass().add("opened"); loaded.getStyleClass().add("badge");
        Region spacer = new Region(); HBox.setHgrow(spacer, Priority.ALWAYS);
        Button min = windowButton("Minimize", "min", () -> stage.setIconified(true));
        Button max = windowButton("Maximize / restore", "max", () -> stage.setMaximized(!stage.isMaximized()));
        Button close = windowButton("Close", "close", this::confirmClose);
        HBox bar = new HBox(13, brand, new Label("│"), opened, loaded, spacer, min, max, close);
        bar.getStyleClass().add("titlebar");
        bar.setOnMousePressed(e -> { if (!(e.getTarget() instanceof Button)) { dragX = e.getScreenX() - stage.getX(); dragY = e.getScreenY() - stage.getY(); } });
        bar.setOnMouseDragged(e -> { if (e.isPrimaryButtonDown() && !stage.isMaximized()) { stage.setX(e.getScreenX() - dragX); stage.setY(e.getScreenY() - dragY); } });
        bar.setOnMouseClicked(e -> { if (e.getClickCount() == 2 && !(e.getTarget() instanceof Button)) stage.setMaximized(!stage.isMaximized()); });
        return bar;
    }

    private Node toolbar() {
        FlowPane actions = new FlowPane(8, 8);
        create = button("Create", this::createDialog);
        rename = button("Rename", this::renameDialog); delete = button("Delete", this::deleteDialog); replace = button("Replace", this::replaceDialog);
        move = button("Move", this::moveDialog);
        Button add = button("Add", this::addDialog), folder = button("New Folder", this::folderDialog), compact = button("Compact", this::compactDialog);
        editControls.addAll(List.of(add, folder, rename, move, delete, replace, compact));
        strategy.setValue("Auto"); strategy.setAccessibleText("Archive edit strategy");
        actions.getChildren().addAll(button("Open", this::chooseArchive), create, extract, test,
            button("Properties", this::properties), add, folder, rename,
            move, delete, replace, compact, new Label("EDIT"), strategy, unavailable("Associations"));
        archiveControls.addAll(List.of(extract, test, (Control)actions.getChildren().get(4)));
        actions.getStyleClass().add("toolbar");
        return actions;
    }

    private Node filters() {
        search.setPromptText("Search archive..."); search.setAccessibleText("Search whole archive");
        extension.setPromptText("e.g. png"); extension.setPrefWidth(105); extension.setAccessibleText("Extension filter");
        type.setValue("All"); type.setPrefWidth(88); type.setAccessibleText("Entry type filter");
        HBox.setHgrow(search, Priority.ALWAYS);
        search.textProperty().addListener((o, a, b) -> query());
        extension.textProperty().addListener((o, a, b) -> query());
        type.valueProperty().addListener((o, a, b) -> query());
        HBox bar = new HBox(9, search, new Label("TYPE"), type, new Label("EXT"), extension,
            button("Clear", () -> { search.clear(); extension.clear(); type.setValue("All"); }));
        bar.setAlignment(Pos.CENTER_LEFT); bar.getStyleClass().add("filters");
        return bar;
    }

    private Node breadcrumbBar() {
        breadcrumbs.setAlignment(Pos.CENTER_LEFT); breadcrumbs.getStyleClass().add("breadcrumbs");
        VBox wrapper = new VBox(breadcrumbs); wrapper.setPadding(new Insets(16, 19, 12, 19));
        crumbs(); return wrapper;
    }

    private Node archiveTable() {
        table.setFixedCellSize(35);
        table.getSelectionModel().setSelectionMode(SelectionMode.MULTIPLE);
        table.setColumnResizePolicy(TableView.CONSTRAINED_RESIZE_POLICY_FLEX_LAST_COLUMN);
        table.setPlaceholder(new Label("No archive loaded\n\nOpen an archive or drop a file here."));
        var name = column("NAME", "name", 325, e -> e.name());
        name.setCellFactory(c -> new TableCell<>() {
            final DotIcon icon = new DotIcon(); final Label text = new Label(); final HBox box = new HBox(10, icon, text);
            { box.setAlignment(Pos.CENTER_LEFT); }
            @Override protected void updateItem(ArchiveModel.Entry item, boolean empty) {
                super.updateItem(item, empty);
                if (empty || item == null) { setGraphic(null); setText(null); setAccessibleText(null); }
                else { icon.set(item); text.setText(item.name()); setGraphic(box); setAccessibleText(item.name()); }
            }
        });
        table.getColumns().addAll(List.of(name,
            column("SIZE", "size", 100, e -> e.directory() ? "—" : ArchiveModel.bytes(e.size())),
            column("COMPRESSED", "compressed", 100, e -> e.compressed() == null ? "—" : ArchiveModel.bytes(e.compressed())),
            column("RATIO", "ratio", 90, e -> e.compressed() == null || e.size() == 0 ? "—" : Math.round(ArchiveModel.ratio(e)) + "%"),
            column("MODIFIED", "modified", 125, ArchiveModel.Entry::modified)));
        table.getSortOrder().add(name);
        table.setSortPolicy(t -> { query(); return true; });
        table.setRowFactory(t -> {
            TableRow<ArchiveModel.Entry> row = new TableRow<>();
            row.setOnMouseClicked(e -> { if (e.getClickCount() == 2 && !row.isEmpty() && row.getItem().directory()) navigate(row.getItem().path()); });
            return row;
        });
        table.getSelectionModel().getSelectedItems().addListener((javafx.collections.ListChangeListener<ArchiveModel.Entry>)c -> { updateTotals(); controls(); });
        root.setOnDragOver(e -> { if (!busy && modal == null && e.getDragboard().hasFiles()) e.acceptTransferModes(TransferMode.COPY); e.consume(); });
        root.setOnDragDropped(e -> {
            boolean accepted = !busy && modal == null && e.getDragboard().hasFiles();
            if (accepted) open(e.getDragboard().getFiles().getFirst().getAbsolutePath(), null, false);
            e.setDropCompleted(accepted); e.consume();
        });
        VBox wrapper = new VBox(table); VBox.setVgrow(table, Priority.ALWAYS);
        wrapper.setPadding(new Insets(0, 19, 16, 19)); return wrapper;
    }

    private TableColumn<ArchiveModel.Entry, ArchiveModel.Entry> column(String title, String field, int width, java.util.function.Function<ArchiveModel.Entry, String> format) {
        var c = new TableColumn<ArchiveModel.Entry, ArchiveModel.Entry>(title);
        c.setUserData(field); c.setPrefWidth(width); c.setMinWidth(field.equals("name") ? 160 : 65);
        c.setCellValueFactory(v -> new ReadOnlyObjectWrapper<>(v.getValue()));
        c.setCellFactory(v -> new TableCell<>() {
            @Override protected void updateItem(ArchiveModel.Entry item, boolean empty) {
                super.updateItem(item, empty); setGraphic(null); setText(empty || item == null ? null : format.apply(item));
            }
        });
        return c;
    }

    private Node statusbar() {
        Label prompt = new Label("archi@client:/archive$"); prompt.getStyleClass().add("prompt");
        Region space = new Region(); HBox.setHgrow(space, Priority.ALWAYS);
        progress.setPrefWidth(95); progress.setVisible(false); progress.managedProperty().bind(progress.visibleProperty());
        cancel.setVisible(false); cancel.managedProperty().bind(cancel.visibleProperty());
        HBox bar = new HBox(10, prompt, totals, status, space, progress, cancel);
        bar.setAlignment(Pos.CENTER_LEFT); bar.getStyleClass().add("statusbar"); return bar;
    }

    private void chooseArchive() {
        if (busy || modal != null) return;
        FileChooser chooser = new FileChooser(); chooser.setTitle("Open archive");
        chooser.getExtensionFilters().addAll(new FileChooser.ExtensionFilter("Archives", "*.zip", "*.7z", "*.rar", "*.tar", "*.gz", "*.bz2", "*.xz"), new FileChooser.ExtensionFilter("All files", "*.*"));
        var file = chooser.showOpenDialog(stage);
        if (file != null) open(file.getAbsolutePath(), null, false);
    }

    private void open(String path, String password, boolean remember) {
        open(path, password, remember, false);
    }

    private void open(String path, String password, boolean remember, boolean refresh) {
        if (busy || worker == null) return;
        long revision = ++sessionRevision;
        busy(true, "Opening archive...");
        var args = new JsonObject(); args.addProperty("path", path); if (password != null) args.addProperty("password", password);
        var call = worker.request("open", args); operation = call.id();
        call.result().thenApplyAsync(ArchiveModel::new, queries).whenComplete((model, error) -> Platform.runLater(() -> {
            if (closed || revision != sessionRevision) return;
            busy(false, error == null ? "Ready." : "Open failed.");
            if (error != null) {
                if (passwordError(error)) passwordDialog(path, "Archive is password-protected", password == null ? null : "Invalid password. Please try again.", (pw, keep) -> open(path, pw, keep, refresh));
                else showError(error);
                return;
            }
            archive = model;
            if (!refresh) { parent = "/"; search.clear(); extension.clear(); type.setValue("All"); table.getSelectionModel().clearSelection(); }
            if (!parent.equals("/") && model.entries.stream().noneMatch(e -> e.directory() && e.path().equals(parent))) parent = "/";
            secret = remember ? password : null; riskAcknowledged = false;
            stage.setTitle(Path.of(path).getFileName() + " — Archi");
            opened.setText("Open: " + Path.of(path).getFileName());
            loaded.setText("LOADED " + model.entries.size() + " ENTRIES.");
            crumbs(); query(); controls(); updateTotals();
        }));
    }

    private void query() {
        if (archive == null) return;
        if (queryTask != null) queryTask.cancel(true);
        var model = archive;
        String folder = parent, q = search.getText(), ext = extension.getText(), filter = type.getValue();
        var sorted = table.getSortOrder().isEmpty() ? null : table.getSortOrder().getFirst();
        String field = sorted == null ? "name" : sorted.getUserData().toString();
        boolean descending = sorted != null && sorted.getSortType() == TableColumn.SortType.DESCENDING;
        long revision = queryRevision.incrementAndGet();
        Set<String> selected = new HashSet<>(); table.getSelectionModel().getSelectedItems().forEach(e -> selected.add(e.path()));
        queryTask = queries.submit(() -> {
            var entries = model.query(folder, q, filter, ext, ArchiveModel.order(field, descending));
            if (Thread.currentThread().isInterrupted()) return;
            Platform.runLater(() -> {
                if (closed || revision != queryRevision.get() || archive != model) return;
                table.setItems(FXCollections.observableArrayList(entries));
                for (int i = 0; i < entries.size(); i++) if (selected.contains(entries.get(i).path())) table.getSelectionModel().select(i);
                table.setPlaceholder(new Label("No matching entries.")); updateTotals();
            });
        });
    }

    private void navigate(String path) { if (busy) return; parent = ArchiveModel.normalizeParent(path); search.clear(); extension.clear(); type.setValue("All"); table.getSelectionModel().clearSelection(); crumbs(); query(); }
    private void up() { int slash = parent.lastIndexOf('/'); navigate(slash < 0 ? "/" : parent.substring(0, slash)); }
    private void crumbs() {
        breadcrumbs.getChildren().setAll(button("R…", () -> navigate("/")));
        if (!parent.equals("/")) {
            String path = "";
            for (String part : parent.split("/")) {
                path += (path.isEmpty() ? "" : "/") + part; String dest = path;
                breadcrumbs.getChildren().addAll(new Label("/"), button(part, () -> navigate(dest)));
            }
        }
    }

    private void extract() {
        if (archive == null || busy) return;
        Runnable choose = () -> {
            DirectoryChooser chooser = new DirectoryChooser(); chooser.setTitle("Extract archive to...");
            var destination = chooser.showDialog(stage);
            if (destination == null) return;
            var args = archiveArgs(); args.addProperty("destination", destination.getAbsolutePath());
            var selection = new JsonArray(); table.getSelectionModel().getSelectedItems().forEach(e -> selection.add(e.path()));
            if (!selection.isEmpty()) args.add("selected_paths", selection);
            runOperation("extract", args, null);
        };
        var warnings = archive.metadata.getAsJsonArray("warnings");
        var risks = new ArrayList<String>();
        for (var w : warnings) {
            var warning = w.getAsJsonObject(); String code = ArchiveModel.string(warning, "code", "");
            if (!code.equals("encrypted")) risks.add(ArchiveModel.string(warning, "message", code));
        }
        if (!riskAcknowledged && !risks.isEmpty()) {
            showModal("Archive warnings", String.join("\n\n", risks), List.of(action("Cancel", this::dismiss), action("Acknowledge & continue", () -> { dismiss(); riskAcknowledged = true; choose.run(); })), null);
        } else choose.run();
    }

    private JsonObject archiveArgs() { var args = new JsonObject(); args.addProperty("path", archive.path); if (secret != null) args.addProperty("password", secret); return args; }
    private void test() { if (archive != null && !busy) runOperation("test", archiveArgs(), null); }

    private void runOperation(String command, JsonObject args, String suppliedPassword) {
        if (suppliedPassword != null) {
            if (command.equals("edit")) args.getAsJsonObject("options").addProperty("password", suppliedPassword);
            else args.addProperty("password", suppliedPassword);
        }
        busy(true, switch (command) { case "test" -> "Testing archive..."; case "create" -> "Creating archive..."; case "edit" -> "Updating archive..."; default -> "Extracting..."; });
        var call = worker.request(command, args, this::event); operation = call.id();
        call.result().whenComplete((result, error) -> Platform.runLater(() -> {
            if (closed) return;
            if (modal != null) dismiss();
            busy(false, error == null ? "Completed." : "Operation stopped.");
            if (error != null) {
                if (!command.equals("create") && passwordError(error)) passwordDialog(archive.path, "Archive is password-protected", (command.equals("edit") ? args.getAsJsonObject("options").has("password") : args.has("password")) ? "Invalid password. Please try again." : null, (password, remember) -> {
                    secret = remember ? password : null;
                    runOperation(command, args, password);
                });
                else if (!errorCode(error).equals("cancelled")) showError(error);
            } else if (command.equals("test")) {
                showModal("Archive test", "Passed: " + result.get("tested_ok") + "\nFailed: " + result.get("tested_failed"), List.of(action("Close", this::dismiss)), null);
            } else if (command.equals("create")) {
                var options = args.getAsJsonObject("options");
                open(result.get("destination").getAsString(), ArchiveModel.string(options, "password", null), false);
            } else if (command.equals("edit")) {
                open(args.get("path").getAsString(), ArchiveModel.string(args.getAsJsonObject("options"), "password", null), secret != null, true);
            }
        }));
    }

    private void event(JsonObject message) {
        if (message.get("event").getAsString().equals("progress")) {
            latestProgress.set(message);
            if (progressQueued.compareAndSet(false, true)) Platform.runLater(this::flushProgress);
        } else if (message.get("event").getAsString().equals("conflict")) Platform.runLater(() -> {
            if (!busy || !message.get("id").getAsString().equals(operation)) return;
            JsonObject data = message.getAsJsonObject("data");
            CheckBox all = new CheckBox("Apply to all conflicts");
            List<Button> buttons = new ArrayList<>();
            for (String decision : List.of("cancel", "skip", "rename", "overwrite")) buttons.add(action(capitalize(decision), () -> {
                var args = new JsonObject(); args.addProperty("operation_id", operation);
                args.addProperty("conflict_id", data.get("conflict_id").getAsString());
                args.addProperty("decision", decision); args.addProperty("apply_to_all", all.isSelected());
                dismiss(); worker.request("resolve_conflict", args).result().exceptionally(e -> { Platform.runLater(() -> { cancel(); showError(e); }); return null; });
            }));
            showModal("File already exists", data.get("entry_path").getAsString() + "\n\n" + data.get("dest_path").getAsString(), buttons, all);
        });
    }

    private void flushProgress() {
        var message = latestProgress.getAndSet(null);
        if (message != null && busy && message.get("id").getAsString().equals(operation)) {
            var p = message.getAsJsonObject("data");
            progress.setProgress(Math.max(0, Math.min(1, p.get("percentage").getAsDouble() / 100)));
            status.setText(p.get("extracted_files") + "/" + p.get("total_files") + " " + ArchiveModel.string(p, "current_file", ""));
        }
        progressQueued.set(false);
        if (latestProgress.get() != null && progressQueued.compareAndSet(false, true)) Platform.runLater(this::flushProgress);
    }

    private void cancel() {
        if (worker == null || operation == null) return;
        var args = new JsonObject(); args.addProperty("operation_id", operation);
        worker.request("cancel", args); status.setText("Cancelling...");
    }
    private void busy(boolean value, String message) {
        busy = value; if (!value) operation = null;
        progress.setVisible(value); cancel.setVisible(value); progress.setProgress(-1); status.setText(message); controls();
    }
    private void controls() {
        if (create != null) create.setDisable(busy || worker == null);
        archiveControls.forEach(c -> c.setDisable(busy || archive == null));
        boolean editable = archive != null && archive.metadata.getAsJsonObject("capabilities").get("edit").getAsBoolean()
            && !(ArchiveModel.string(archive.metadata, "format", "").equals("zip") && archive.metadata.getAsJsonArray("warnings").asList().stream().anyMatch(w -> ArchiveModel.string(w.getAsJsonObject(), "code", "").equals("encrypted")));
        editControls.forEach(c -> c.setDisable(busy || !editable));
        strategy.setDisable(busy || !editable);
        int selected = table.getSelectionModel().getSelectedItems().size();
        if (rename != null) {
            rename.setDisable(busy || !editable || selected != 1);
            delete.setDisable(busy || !editable || selected == 0);
            move.setDisable(busy || !editable || selected == 0);
            var entry = table.getSelectionModel().getSelectedItem();
            replace.setDisable(busy || !editable || selected != 1 || entry == null || entry.directory());
        }
        if (archive != null) {
            var cap = archive.metadata.getAsJsonObject("capabilities");
            extract.setDisable(busy || !cap.get("extract").getAsBoolean());
            test.setDisable(busy || !cap.get("test").getAsBoolean());
        }
    }
    private void updateTotals() {
        if (archive == null) { totals.setText(""); return; }
        long size = archive.metadata.getAsJsonObject("stats").get("total_uncompressed").getAsLong();
        totals.setText("Items: " + archive.entries.size() + " | Selected: " + table.getSelectionModel().getSelectedItems().size() + " | Size: " + ArchiveModel.bytes(size));
    }
    private void properties() {
        if (archive == null) return;
        var stats = archive.metadata.getAsJsonObject("stats");
        showModal("Archive properties", archive.path + "\n\nFormat: " + archive.metadata.get("format").getAsString()
            + "\nFiles: " + stats.get("file_count") + "\nFolders: " + stats.get("folder_count")
            + "\nSize: " + ArchiveModel.bytes(stats.get("total_uncompressed").getAsLong())
            + "\nPacked: " + ArchiveModel.bytes(stats.get("total_compressed").getAsLong()), List.of(action("Close", this::dismiss)), null);
    }

    private void createDialog() {
        if (busy || worker == null || modal != null) return;
        TextArea sources = new TextArea(); sources.setId("create-sources"); sources.setPrefRowCount(3); sources.setWrapText(true);
        sources.setPromptText("Source files or folders — one absolute path per line"); sources.setAccessibleText("Archive source paths, one per line");
        ComboBox<String> format = new ComboBox<>(FXCollections.observableArrayList("ZIP", "7z", "TAR", "TAR.GZ", "TAR.BZ2", "TAR.XZ"));
        format.setValue("ZIP"); format.setId("create-format"); format.setAccessibleText("Archive format");
        ComboBox<String> compression = new ComboBox<>(FXCollections.observableArrayList("Store", "Fast", "Normal", "Max"));
        compression.setValue("Normal"); compression.setAccessibleText("Compression level");
        TextField output = new TextField(); output.setId("create-output"); output.setPromptText("Output archive path"); output.setAccessibleText("Output archive path");
        PasswordField password = new PasswordField(); password.setId("create-password"); password.setPromptText("Optional password (ZIP / 7z only)"); password.setAccessibleText("New archive password");
        CheckBox includeRoot = new CheckBox("Include source folders"); includeRoot.setSelected(true);
        CheckBox overwrite = new CheckBox("Allow replacing existing output archive");
        Label hint = new Label("ZIP / 7z support AES-256. TAR formats cannot be encrypted."); hint.setWrapText(true);
        format.valueProperty().addListener((o, a, b) -> {
            password.setDisable(!b.equals("ZIP") && !b.equals("7z"));
            if (password.isDisabled()) password.clear();
            compression.setDisable(b.equals("TAR"));
        });
        Runnable addFiles = () -> {
            FileChooser chooser = new FileChooser(); chooser.setTitle("Add source files");
            var files = chooser.showOpenMultipleDialog(stage);
            if (files != null) files.forEach(file -> sources.appendText((sources.getText().isEmpty() ? "" : "\n") + file.getAbsolutePath()));
        };
        Runnable addFolder = () -> {
            DirectoryChooser chooser = new DirectoryChooser(); chooser.setTitle("Add source folder");
            var file = chooser.showDialog(stage);
            if (file != null) sources.appendText((sources.getText().isEmpty() ? "" : "\n") + file.getAbsolutePath());
        };
        Button browse = button("Browse…", () -> {
            FileChooser chooser = new FileChooser(); chooser.setTitle("Save archive");
            chooser.setInitialFileName("archive." + format.getValue().toLowerCase(Locale.ROOT));
            var file = chooser.showSaveDialog(stage); if (file != null) output.setText(file.getAbsolutePath());
        });
        VBox inputs = new VBox(10, new Label("Sources"), sources, new HBox(10, button("Add files", addFiles), button("Add folder", addFolder)),
            new HBox(12, new Label("Format"), format, new Label("Compression"), compression),
            new Label("Output"), new HBox(10, output, browse), new Label("Password"), password, includeRoot, overwrite, hint);
        HBox.setHgrow(output, Priority.ALWAYS);
        Button confirm = action("Create archive", () -> {
            var args = new JsonObject(); var paths = new JsonArray();
            sources.getText().lines().filter(s -> !s.isBlank()).forEach(paths::add);
            args.add("source_paths", paths); args.addProperty("output_path", output.getText());
            var options = new JsonObject();
            options.addProperty("format", switch (format.getValue()) { case "7z" -> "sevenZ"; case "TAR.GZ" -> "tarGz"; case "TAR.BZ2" -> "tarBz2"; case "TAR.XZ" -> "tarXz"; default -> format.getValue().toLowerCase(Locale.ROOT); });
            options.addProperty("compression", format.getValue().equals("TAR") ? "store" : compression.getValue().toLowerCase(Locale.ROOT));
            options.addProperty("includeRoot", includeRoot.isSelected()); options.addProperty("overwrite", overwrite.isSelected());
            if (!password.isDisabled() && !password.getText().isEmpty()) options.addProperty("password", password.getText());
            args.add("options", options); password.clear(); dismiss();
            if (overwrite.isSelected()) {
                showModal("Replace output archive?", "If output already exists, it will be replaced after successful creation.\n\n" + output.getText(),
                    List.of(action("Cancel", this::dismiss), action("Create & allow replacement", () -> { dismiss(); runOperation("create", args, null); })), null);
            } else runOperation("create", args, null);
        });
        confirm.disableProperty().bind(sources.textProperty().isEmpty().or(output.textProperty().isEmpty()));
        showModal("CREATE ARCHIVE", "Select source paths, format and destination.", List.of(action("Cancel", () -> { password.clear(); dismiss(); }), confirm), inputs);
        Platform.runLater(sources::requestFocus);
    }

    private JsonObject editArgs(String kind) {
        var args = new JsonObject(); args.addProperty("path", archive.path);
        var options = new JsonObject(); if (secret != null) options.addProperty("password", secret);
        options.addProperty("strategy", switch (strategy.getValue()) { case "Prefer fast" -> "preferFast"; case "Prefer compact" -> "preferCompact"; default -> "auto"; });
        args.add("options", options); var action = new JsonObject(); action.addProperty("kind", kind); args.add("action", action);
        return args;
    }

    private String childPath(String name) { return parent.equals("/") ? name : parent + "/" + name; }

    private void entryNameDialog(String title, String initial, Consumer<String> confirmed) {
        TextField field = new TextField(initial); field.setId("entry-name"); field.setAccessibleText(title);
        Label error = new Label("Use one name, without / or \\."); error.setWrapText(true);
        Button confirm = action("Confirm", () -> { String name = field.getText(); dismiss(); confirmed.accept(name); });
        confirm.disableProperty().bind(javafx.beans.binding.Bindings.createBooleanBinding(() -> field.getText().isBlank()
            || field.getText().contains("/") || field.getText().contains("\\") || field.getText().equals(".") || field.getText().equals(".."), field.textProperty()));
        confirm.setDefaultButton(true);
        showModal(title, "Archive paths are validated by the Rust engine before writing.", List.of(action("Cancel", this::dismiss), confirm), new VBox(12, field, error));
        Platform.runLater(() -> { field.requestFocus(); field.selectAll(); });
    }

    private void folderDialog() {
        if (busy || archive == null) return;
        entryNameDialog("New folder", "", name -> {
            var args = editArgs("create_folder"); args.getAsJsonObject("action").addProperty("folder_path", childPath(name)); runOperation("edit", args, null);
        });
    }

    private void renameDialog() {
        var selected = table.getSelectionModel().getSelectedItem();
        if (busy || selected == null) return;
        entryNameDialog("Rename entry", selected.name(), name -> {
            var args = editArgs("rename"); var action = args.getAsJsonObject("action");
            action.addProperty("from_path", selected.path());
            action.addProperty("to_path", selected.parent().equals("/") ? name : selected.parent() + "/" + name);
            runOperation("edit", args, null);
        });
    }

    private void deleteDialog() {
        if (busy || archive == null || table.getSelectionModel().getSelectedItems().isEmpty()) return;
        var args = editArgs("delete"); var paths = new JsonArray();
        table.getSelectionModel().getSelectedItems().forEach(e -> paths.add(e.path())); args.getAsJsonObject("action").add("paths", paths);
        showModal("Delete archive entries?", "Remove " + paths.size() + " selected entries, including folder contents?\n\nFast ZIP delete may leave recoverable data. Compact is not secure erase.",
            List.of(action("Cancel", this::dismiss), action("Delete entries", () -> { dismiss(); runOperation("edit", args, null); })), null);
    }

    private void moveDialog() {
        if (busy || archive == null || move.isDisabled()) return;
        var selected = List.copyOf(table.getSelectionModel().getSelectedItems());
        var folders = new ArrayList<String>(); folders.add("/");
        archive.entries.stream().filter(ArchiveModel.Entry::directory).map(ArchiveModel.Entry::path)
            .filter(path -> selected.stream().noneMatch(e -> path.equals(e.path()) || path.startsWith(e.path() + "/")))
            .sorted().forEach(folders::add);
        ComboBox<String> destination = new ComboBox<>(FXCollections.observableArrayList(folders));
        destination.setId("move-destination"); destination.setAccessibleText("Destination archive folder");
        destination.setMaxWidth(Double.MAX_VALUE); destination.setValue("/");
        Button confirm = action("Move entries", () -> {
            var args = editArgs("move"); var action = args.getAsJsonObject("action"); var paths = new JsonArray();
            selected.forEach(e -> paths.add(e.path())); action.add("source_paths", paths);
            action.addProperty("dest_folder", destination.getValue().equals("/") ? "" : destination.getValue());
            dismiss(); runOperation("edit", args, null);
        });
        confirm.disableProperty().bind(javafx.beans.binding.Bindings.createBooleanBinding(
            () -> destination.getValue() == null || selected.stream().allMatch(e -> e.parent().equals(destination.getValue())), destination.valueProperty()));
        showModal("Move archive entries", "Move " + selected.size() + " selected entries. Existing target collisions are rejected.",
            List.of(action("Cancel", this::dismiss), confirm), destination);
        Platform.runLater(destination::requestFocus);
    }

    private void addDialog() {
        if (busy || archive == null) return;
        TextArea sources = new TextArea(); sources.setPrefRowCount(3); sources.setWrapText(true); sources.setAccessibleText("Paths to add, one per line"); sources.setId("add-sources");
        Button files = button("Add files", () -> {
            var chooser = new FileChooser(); var chosen = chooser.showOpenMultipleDialog(stage);
            if (chosen != null) chosen.forEach(f -> sources.appendText((sources.getText().isEmpty() ? "" : "\n") + f.getAbsolutePath()));
        });
        Button folder = button("Add folder", () -> {
            var chooser = new DirectoryChooser(); var chosen = chooser.showDialog(stage);
            if (chosen != null) sources.appendText((sources.getText().isEmpty() ? "" : "\n") + chosen.getAbsolutePath());
        });
        Button confirm = action("Add to archive", () -> {
            var args = editArgs("add"); var action = args.getAsJsonObject("action"); var paths = new JsonArray();
            sources.getText().lines().filter(s -> !s.isBlank()).forEach(paths::add);
            action.add("source_paths", paths); action.addProperty("archive_parent", parent.equals("/") ? "" : parent);
            dismiss(); runOperation("edit", args, null);
        });
        confirm.disableProperty().bind(sources.textProperty().isEmpty());
        showModal("Add to archive", "Destination: " + parent + "\nExisting entry collisions are rejected without replacement.", List.of(action("Cancel", this::dismiss), confirm), new VBox(10, sources, new HBox(10, files, folder)));
    }

    private void replaceDialog() {
        var selected = table.getSelectionModel().getSelectedItem();
        if (busy || selected == null || selected.directory()) return;
        var chooser = new FileChooser(); chooser.setTitle("Replace " + selected.name()); var file = chooser.showOpenDialog(stage);
        if (file == null) return;
        var args = editArgs("replace"); var action = args.getAsJsonObject("action");
        action.addProperty("entry_path", selected.path()); action.addProperty("source_file", file.getAbsolutePath());
        showModal("Replace archive file?", selected.path() + "\n\nNew contents: " + file.getAbsolutePath(), List.of(action("Cancel", this::dismiss), action("Replace file", () -> { dismiss(); runOperation("edit", args, null); })), null);
    }

    private void compactDialog() {
        if (busy || archive == null) return;
        var args = editArgs("compact");
        showModal("Compact archive?", "Rewrite the archive and reclaim unused ZIP entry data. This is not secure erase.",
            List.of(action("Cancel", this::dismiss), action("Compact archive", () -> { dismiss(); runOperation("edit", args, null); })), null);
    }

    @FunctionalInterface private interface PasswordAction { void accept(String password, boolean remember); }
    private void passwordDialog(String path, String title, String error, PasswordAction confirmed) {
        PasswordField field = new PasswordField(); field.setPromptText("Type your password"); field.setAccessibleText("Archive password");
        CheckBox remember = new CheckBox("Remember on this session");
        VBox inputs = new VBox(12, new Label("Password"), field, remember, new Label("For opening existing encrypted archive."));
        if (error != null) { var label = new Label(error); label.getStyleClass().add("error"); inputs.getChildren().add(label); }
        Button confirm = action("Confirm", () -> { String password = field.getText(); field.clear(); dismiss(); confirmed.accept(password, remember.isSelected()); });
        confirm.disableProperty().bind(field.textProperty().isEmpty());
        confirm.setDefaultButton(true);
        showModal(title, "Enter the password to decrypt this archive.", List.of(action("Cancel", () -> { field.clear(); dismiss(); }), confirm), inputs);
        Platform.runLater(field::requestFocus);
    }

    private void showModal(String title, String message, List<Button> buttons, Node extra) {
        if (modal != null) dismiss();
        previousFocus = stage.getScene().getFocusOwner();
        content.setEffect(new GaussianBlur(6)); content.setDisable(true);
        Label heading = new Label(title); heading.getStyleClass().add("dialog-title");
        Label body = new Label(message); body.setWrapText(true); body.getStyleClass().add("dialog-body");
        HBox footer = new HBox(10); footer.setAlignment(Pos.CENTER_RIGHT); footer.getChildren().addAll(buttons);
        heading.setMinHeight(Region.USE_PREF_SIZE); footer.setMinHeight(Region.USE_PREF_SIZE);
        VBox contents = new VBox(22, body); if (extra != null) contents.getChildren().add(extra);
        contents.setMinHeight(Region.USE_PREF_SIZE);
        ScrollPane scroll = new ScrollPane(contents); scroll.setFitToWidth(true); scroll.setHbarPolicy(ScrollPane.ScrollBarPolicy.NEVER); scroll.getStyleClass().add("dialog-scroll");
        scroll.setFocusTraversable(false);
        // ScrollPane's default viewport height would scroll even short dialogs.
        scroll.prefViewportHeightProperty().bind(javafx.beans.binding.Bindings.createDoubleBinding(
            () -> contents.prefHeight(Math.max(1, scroll.getWidth() - 12)), scroll.widthProperty()));
        scroll.setMinHeight(0); VBox.setVgrow(scroll, Priority.ALWAYS);
        VBox card = new VBox(22, heading, scroll, footer);
        card.getStyleClass().add("dialog"); card.setMaxWidth(448); card.setMinHeight(0); card.setMaxHeight(Region.USE_PREF_SIZE);
        modal = new StackPane(card); modal.getStyleClass().add("modal-overlay");
        root.getChildren().add(modal);
        modal.applyCss(); modal.layout();
        StackPane shown = modal;
        Platform.runLater(() -> { if (modal == shown && !buttons.isEmpty()) buttons.getFirst().requestFocus(); });
    }
    private void dismiss() {
        if (modal == null) return;
        root.getChildren().remove(modal); modal = null; content.setDisable(false); content.setEffect(null);
        if (previousFocus != null) previousFocus.requestFocus();
    }
    private void showError(Throwable error) { showModal("Operation failed", unwrap(error).getMessage(), List.of(action("Close", this::dismiss)), null); }
    private static Throwable unwrap(Throwable e) { while ((e instanceof CompletionException || e instanceof ExecutionException) && e.getCause() != null) e = e.getCause(); return e; }
    private static String errorCode(Throwable e) { return unwrap(e) instanceof WorkerClient.WorkerError w ? w.code : ""; }
    private static boolean passwordError(Throwable e) { return Set.of("password_required", "invalid_password", "wrong_password").contains(errorCode(e)); }
    private static String capitalize(String s) { return Character.toUpperCase(s.charAt(0)) + s.substring(1); }

    private void confirmClose() {
        if (busy) showModal("Close Archi?", "Active operation will be cancelled.", List.of(action("Keep working", this::dismiss), action("Cancel & close", () -> { cancel(); stage.hide(); Platform.exit(); })), null);
        else { stage.hide(); Platform.exit(); }
    }
    private void key(KeyEvent e) {
        if (modal != null) {
            if (e.getCode() == KeyCode.ESCAPE) { if (busy) cancel(); dismiss(); e.consume(); }
            return;
        }
        if (e.isControlDown() && e.getCode() == KeyCode.O) { chooseArchive(); e.consume(); }
        else if (e.isControlDown() && e.getCode() == KeyCode.N) { createDialog(); e.consume(); }
        else if (e.isControlDown() && e.getCode() == KeyCode.F) { search.requestFocus(); e.consume(); }
        else if (!(e.getTarget() instanceof TextInputControl)) {
            if (e.getCode() == KeyCode.BACK_SPACE || e.isAltDown() && e.getCode() == KeyCode.UP) { up(); e.consume(); }
            else if (e.getCode() == KeyCode.ENTER) { var selected = table.getSelectionModel().getSelectedItem(); if (selected != null && selected.directory()) navigate(selected.path()); e.consume(); }
            else if (e.getCode() == KeyCode.ESCAPE && busy) { cancel(); e.consume(); }
            else if (e.getCode() == KeyCode.F2 && !rename.isDisabled()) { renameDialog(); e.consume(); }
            else if (e.getCode() == KeyCode.DELETE && !delete.isDisabled()) { deleteDialog(); e.consume(); }
        }
    }
    private void installResize(Scene scene) {
        final double[] start = new double[6];
        final Cursor[] active = {null};
        scene.addEventFilter(MouseEvent.MOUSE_MOVED, e -> {
            if (active[0] != null) return;
            boolean left = e.getSceneX() < 7, right = e.getSceneX() > scene.getWidth() - 7;
            boolean top = e.getSceneY() < 7, bottom = e.getSceneY() > scene.getHeight() - 7;
            scene.setCursor(modal != null || stage.isMaximized() ? Cursor.DEFAULT
                : top ? (left ? Cursor.NW_RESIZE : right ? Cursor.NE_RESIZE : Cursor.N_RESIZE)
                : bottom ? (left ? Cursor.SW_RESIZE : right ? Cursor.SE_RESIZE : Cursor.S_RESIZE)
                : left ? Cursor.W_RESIZE : right ? Cursor.E_RESIZE : Cursor.DEFAULT);
        });
        scene.addEventFilter(MouseEvent.MOUSE_PRESSED, e -> {
            Cursor cursor = scene.getCursor();
            if (!e.isPrimaryButtonDown() || modal != null || stage.isMaximized() || cursor == Cursor.DEFAULT || cursor == null) return;
            active[0] = cursor;
            start[0] = e.getScreenX(); start[1] = e.getScreenY();
            start[2] = stage.getWidth(); start[3] = stage.getHeight(); start[4] = stage.getX(); start[5] = stage.getY();
            e.consume();
        });
        scene.addEventFilter(MouseEvent.MOUSE_DRAGGED, e -> {
            Cursor cursor = active[0]; if (cursor == null) return;
            boolean west = cursor == Cursor.W_RESIZE || cursor == Cursor.NW_RESIZE || cursor == Cursor.SW_RESIZE;
            boolean east = cursor == Cursor.E_RESIZE || cursor == Cursor.NE_RESIZE || cursor == Cursor.SE_RESIZE;
            boolean north = cursor == Cursor.N_RESIZE || cursor == Cursor.NW_RESIZE || cursor == Cursor.NE_RESIZE;
            boolean south = cursor == Cursor.S_RESIZE || cursor == Cursor.SW_RESIZE || cursor == Cursor.SE_RESIZE;
            if (west || east) {
                double width = Math.max(stage.getMinWidth(), start[2] + (west ? start[0] - e.getScreenX() : e.getScreenX() - start[0]));
                stage.setWidth(width); if (west) stage.setX(start[4] + start[2] - width);
            }
            if (north || south) {
                double height = Math.max(stage.getMinHeight(), start[3] + (north ? start[1] - e.getScreenY() : e.getScreenY() - start[1]));
                stage.setHeight(height); if (north) stage.setY(start[5] + start[3] - height);
            }
            e.consume();
        });
        scene.addEventFilter(MouseEvent.MOUSE_RELEASED, e -> { if (active[0] != null) { active[0] = null; scene.setCursor(Cursor.DEFAULT); e.consume(); } });
        scene.addEventFilter(MouseEvent.MOUSE_EXITED, e -> { if (active[0] == null) scene.setCursor(Cursor.DEFAULT); });
    }
    private static Button button(String text, Runnable action) { Button b = new Button(text); b.setOnAction(e -> action.run()); return b; }
    private static Button action(String text, Runnable action) { return button(text, action); }
    private static Button unavailable(String text) { Button b = new Button(text); b.setDisable(true); b.setTooltip(new Tooltip("Not migrated yet")); return b; }
    private static Button windowButton(String description, String style, Runnable action) { var b = button("", action); b.setAccessibleText(description); b.setTooltip(new Tooltip(description)); b.getStyleClass().addAll("window-button", style); return b; }
    @Override public void stop() { closed = true; secret = null; queries.shutdownNow(); if (worker != null) worker.close(); }
}
