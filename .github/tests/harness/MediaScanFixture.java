import java.io.File;
import java.util.Set;

public class MediaScanFixture {
    static class android {
        static class net { static class Uri {} }
        static class os { static class Bundle {} }
    }
    static final android.net.Uri collection = new android.net.Uri();
    static final long[] augmented = {1, 4};
    static class ParcelFileDescriptor implements AutoCloseable {
        boolean closed;
        int number = 17;
        public int getFd() { assert !closed; return number; }
        public void close() { assert !closed; closed = true; }
    }
    static int depth;
    static int fdDepth;
    static void enterProviderInternalCall() { depth++; }
    static void exitProviderInternalCall() { depth--; }
    static void pushScannedFile(String path, int fd) { assert fd >= 0; fdDepth++; }
    static void popScannedFile() { fdDepth--; }
    static Object fieldValue(Object receiver, String name) {
        assert receiver instanceof MediaScanFixture;
        if (name.equals("mSingleFile")) return false;
        assert name.equals("mFilesUri");
        return collection;
    }
    static class MediaScanFiles {
        static class Unavailable extends RuntimeException {}
        boolean unavailable;
        ParcelFileDescriptor fd;
        ParcelFileDescriptor open(String path) {
            assert depth == 1;
            if (unavailable) throw new Unavailable();
            return fd;
        }
        long[] includePrivateFiles(android.net.Uri uri, android.os.Bundle args, long[] seen) {
            assert depth == 1 && uri == collection && seen.length == 1;
            return augmented;
        }
    }
    final MediaScanFiles mediaScanFiles = new MediaScanFiles();
    boolean fail;
    Object callBackup(Object[] raw) {
        Object[] slots = raw.length == 1 ? (Object[])raw[0] : raw;
        assert slots[0] == this && depth == 0;
        if (slots[1] instanceof File) {
            assert fdDepth == (mediaScanFiles.fd == null ? 0 : 1);
            if (fail) throw new IllegalStateException("scan");
        } else if (slots[1] instanceof java.nio.file.Path) {
            assert fdDepth == 1;
            assert ((java.nio.file.attribute.BasicFileAttributes) slots[2]).size() == 27;
        } else {
            assert slots[1] instanceof android.os.Bundle && slots[2] == augmented;
        }
        return this;
    }
    static class Cursor {
        String owner = "com.example.owner", volume = "external_primary";
        long generation = 19, id = 42;
        String path = "/storage/emulated/0/Documents/name";
        long getLong(int column) { return column == 0 ? id : generation; }
        String getString(int column) { return column == 1 ? volume : column == 3 ? owner : path; }
    }
    int user() { return 0; }
    /* 注入生产方法 */

    public static void main(String[] args) throws Throwable {
        MediaScanFixture fixture = new MediaScanFixture();
        for (boolean nested : new boolean[] {false, true}) {
            long[] seen = {1};
            Object[] slots = {fixture, new android.os.Bundle(), seen};
            assert fixture.mediaScanCallback(nested ? new Object[]{slots} : slots) == fixture;
            assert slots[2] == seen;
            slots = new Object[]{fixture, new File("/logical"), 2};
            for (boolean fail : new boolean[]{false, true}) {
                fixture.fail = fail;
                fixture.mediaScanFiles.fd = new ParcelFileDescriptor();
                try {
                    fixture.mediaScanCallback(nested ? new Object[]{slots} : slots);
                    assert !fail;
                } catch (IllegalStateException error) {
                    assert fail && error.getMessage().equals("scan");
                }
                assert fixture.mediaScanFiles.fd.closed && depth == 0 && fdDepth == 0;
            }
            fixture.fail = false;
            fixture.mediaScanFiles.fd = null;
            assert fixture.mediaScanCallback(slots) == fixture;
        }
        fixture.mediaScanFiles.unavailable = true;
        java.nio.file.Path regular = java.nio.file.Files.createTempFile("scan-missing-private", ".txt");
        try {
            Object[] visit = {fixture, regular, java.nio.file.Files.readAttributes(regular,
                java.nio.file.attribute.BasicFileAttributes.class)};
            assert fixture.mediaScanCallback(visit) == java.nio.file.FileVisitResult.CONTINUE;
            assert depth == 0 && fdDepth == 0;
        } finally { java.nio.file.Files.delete(regular); }
        fixture.mediaScanFiles.unavailable = false;
        Cursor row = new Cursor();
        Set<String> ids = Set.of("database/external_primary/42/19/com.example.owner");
        assert fixture.verifiedBacking(row, ids, "database").equals(
            "/data/media/0/Android/data/com.example.owner/sdcard/Documents/name");
        assert fixture.verifiedBacking(row, ids, null) == null;
        assert fixture.verifiedBacking(row, ids, "new-database") == null;
        row.generation++;
        assert fixture.verifiedBacking(row, ids, "database") == null;
        row.generation--;
        row.id++;
        assert fixture.verifiedBacking(row, ids, "database") == null;
        row.id--;
        row.volume = "internal";
        assert fixture.verifiedBacking(row, ids, "database") == null;
        row.volume = "external_primary";
        row.owner = "com.example.other";
        assert fixture.verifiedBacking(row, ids, "database") == null;
        for (String path : new String[]{"/storage/emulated/10/name", "/storage/emulated/0/",
             "/storage/emulated/0/Android/data/name", "/storage/emulated/0/../name",
             "/storage/emulated/0/Documents//name", "/storage/emulated/0/./name"}) {
            assert backing(path, "com.example.owner", 0) == null;
        }
        java.nio.file.Path fds = java.nio.file.Path.of("/proc/self/fd");
        if (java.nio.file.Files.isDirectory(fds)) {
            java.nio.file.Path backing = java.nio.file.Files.createTempFile("scan-backing", ".test");
            java.nio.file.Path logical = java.nio.file.Files.createTempFile("scan-public", ".test");
            try {
                java.nio.file.Files.write(backing, new byte[27]);
                java.nio.file.Files.write(logical, new byte[7]);
                try (java.io.InputStream input = java.nio.file.Files.newInputStream(backing);
                     java.util.stream.Stream<java.nio.file.Path> entries = java.nio.file.Files.list(fds)) {
                    ParcelFileDescriptor fd = new ParcelFileDescriptor();
                    fd.number = entries.filter(entry -> {
                        try { return java.nio.file.Files.isSameFile(entry, backing); }
                        catch (java.io.IOException e) { return false; }
                    }).mapToInt(entry -> Integer.parseInt(entry.getFileName().toString())).findFirst().orElseThrow();
                    fixture.mediaScanFiles.fd = fd;
                    Object[] visit = {fixture, logical, java.nio.file.Files.readAttributes(logical,
                            java.nio.file.attribute.BasicFileAttributes.class)};
                    assert fixture.mediaScanCallback(visit) == fixture && fd.closed;
                    assert ((java.nio.file.attribute.BasicFileAttributes) visit[2]).size() == 7;
                    assert fdDepth == 0 && java.nio.file.Files.size(logical) == 7;
                }
            } finally {
                java.nio.file.Files.delete(logical);
                java.nio.file.Files.delete(backing);
            }
        }
    }
}
