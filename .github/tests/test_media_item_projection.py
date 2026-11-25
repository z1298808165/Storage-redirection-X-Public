"""验证单项查询补入路径列后仍保留调用方请求的列。"""

from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from test_mount_metadata_scope import function_body


ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(shutil.which("javac") and shutil.which("java"), "requires JDK")
class MediaItemProjectionTest(unittest.TestCase):
    def test_projection_patch(self):
        source = (ROOT / "java_src/org/srx/hook/Hooker.java").read_text(encoding="utf8")
        fixture = """
import java.util.Arrays;
import java.util.regex.Pattern;
public class ProjectionTest {
    static void check(String path, String[] columns, boolean shouldPatch, boolean nested) {
        Object uri = new android.net.Uri(path);
        Object[] args = {uri, columns, null, null};
        Object[] raw = nested ? new Object[]{args} : new Object[]{new Object(), uri, columns, null, null};
        ProjectionPatch patch = ProjectionPatch.apply(raw, args);
        assert (patch.visibleColumns != null) == shouldPatch : path + Arrays.toString(columns);
        assert (nested ? ((Object[]) raw[0])[1] : raw[2]) == args[1];
        if (shouldPatch) {
            assert patch.visibleColumns == columns;
            String[] expected = Arrays.copyOf(columns, columns.length + 1);
            expected[columns.length] = "_data";
            assert Arrays.equals(expected, (String[]) args[1]);
        } else {
            assert args[1] == columns;
        }
    }
    public static void main(String[] args) {
        String base = "content://media/external_primary/";
        for (boolean nested : new boolean[]{false, true}) {
            for (String collection : new String[]{"file", "downloads", "images/media", "audio/media", "video/media"}) {
                String item = base + collection + "/42?limit=1";
                check(item, new String[]{"_size", "is_pending"}, true, nested);
                check(item, new String[]{"is_pending", "_size"}, true, nested);
                check(item, new String[]{"_size", "_data"}, false, nested);
                check(item, new String[]{"COUNT(*)"}, false, nested);
                check(item, new String[]{"MAX(_size) AS largest"}, false, nested);
                check(item, null, false, nested);
                check(base + collection, new String[]{"_size"}, false, nested);
                check(base + collection, new String[]{"_id", "_size"}, true, nested);
                check(base + collection, new String[]{"COUNT(_id)"}, false, nested);
            }
            for (String other : new String[]{"audio/albums/42", "audio/genres/42", "images%2Fmedia/42", "file/abc"}) {
                check(base + other, new String[]{"title"}, false, nested);
            }
            check("content://other/external/file/42", new String[]{"_size"}, false, nested);
        }
    }
"""
        patch = "private static final class ProjectionPatch"
        replace = "private static void replaceActualArg("
        fixture += patch + " " + function_body(source, patch)
        fixture += source[source.index(replace):source.index("{", source.index(replace))]
        fixture += function_body(source, replace) + "\n}"
        uri = """
package android.net;
public final class Uri {
    final java.net.URI value;
    public Uri(String uri) { value = java.net.URI.create(uri); }
    public String getEncodedPath() { return value.getRawPath(); }
    public String getAuthority() { return value.getAuthority(); }
}
"""
        with tempfile.TemporaryDirectory() as directory:
            tmp = Path(directory)
            (tmp / "Uri.java").write_text(uri, encoding="utf8")
            (tmp / "ProjectionTest.java").write_text(fixture, encoding="utf8")
            subprocess.run(["javac", "-d", ".", "Uri.java", "ProjectionTest.java"],
                           cwd=tmp, check=True, timeout=40)
            subprocess.run(["java", "-ea", "-cp", ".", "ProjectionTest"], cwd=tmp, check=True, timeout=20)
