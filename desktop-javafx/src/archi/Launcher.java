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
        for (int i = 0; i < args.length; i++) {
            if (args[i].equals("--extract-here") && i + 1 < args.length) {
                System.setProperty("archi.action", args[i].substring(2));
                System.setProperty("archi.path", args[++i]);
            } else if (!args[i].startsWith("--") && System.getProperty("archi.path") == null) {
                System.setProperty("archi.path", args[i]);
            }
        }
        javafx.application.Application.launch(ArchiApp.class, args);
    }
}
