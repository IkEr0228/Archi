package archi;

public final class Launcher {
    public static void main(String[] args) {
        if (System.getProperty("archi.worker") == null) {
            try {
                var application = java.nio.file.Path.of(Launcher.class.getProtectionDomain().getCodeSource().getLocation().toURI());
                var worker = application.resolveSibling("archi-worker.exe");
                if (java.nio.file.Files.isRegularFile(worker)) System.setProperty("archi.worker", worker.toString());
            } catch (java.net.URISyntaxException e) { throw new IllegalStateException("Cannot locate application files", e); }
        }
        javafx.application.Application.launch(ArchiApp.class, args);
    }
}
