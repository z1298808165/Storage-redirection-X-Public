"""验证 MediaProvider FileUtils 参数路由使用生产回调。"""

from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from test_mount_metadata_scope import function_body


ROOT = Path(__file__).resolve().parents[2]


def method(source, signature):
    start = source.index(signature)
    return source[start : source.index("{", start)] + function_body(source, signature)


@unittest.skipUnless(shutil.which("javac") and shutil.which("java"), "需要 JDK")
class MediaFileUtilsRoutingTest(unittest.TestCase):
    def test_file_utils_arguments_are_preserved_while_parent_is_redirected(self):
        hooker = (ROOT / "java_src/org/srx/hook/Hooker.java").read_text(encoding="utf-8")
        methods = [
            method(hooker, "public Object providerMediaFileUtilsCallback("),
            method(hooker, "private static Object[] replaceFileArgument("),
            method(hooker, "private static Object[] replaceArgument("),
        ]
        fixture = r'''
import java.io.File;
import java.lang.reflect.Method;
import java.util.Arrays;
import java.util.Objects;

public class MediaFileUtilsRoutingFixture {
  static final int ANDROID_APP_UID_START = 10000;
  static final ThreadLocal<Boolean> LOGICAL_MEDIA_FILE_COLUMNS = new ThreadLocal<>();
  static final ThreadLocal<MediaFileBuildContext> MEDIA_FILE_BUILD_CONTEXT = new ThreadLocal<>();
  static final String PUBLIC_PARENT = "/storage/emulated/0/Documents/Media";
  static final String PRIVATE_PARENT = "/data/media/0/Android/data/com.example.app/sdcard/Documents/Media";
  static final int CALLER_UID = 10123;
  static final class MediaFileBuildContext {
    final String publicPath;
    MediaFileBuildContext(String path) { publicPath = path; }
  }
  static final class HookerFixture {
    Method target;
    Object[] received;
    Object result;
    Object callBackup(Object[] args) {
      Object[] actual = unwrapArgs(args);
      received = actual.clone();
      File parent = (File) actual[0];
      result = new File(parent, "created (1).txt");
      return result;
    }
    Object[] unwrapArgs(Object[] args) {
      return args != null && args.length == 1 && args[0] instanceof Object[]
          ? (Object[]) args[0] : args;
    }
    boolean enterDirectoryCallerScope(String path) { return true; }
    void exitCallerScope() {}
    int mediaDirectoryCallerUid() { return CALLER_UID; }
    String resolveMediaStoreDirectPathForValues(String path, int uid) {
      return path.equals(PUBLIC_PARENT) ? PRIVATE_PARENT : path;
    }
    int firstFileArgumentIndex(Object[] args) {
      for (int i = 0; i < args.length; i++) if (args[i] instanceof File) return i;
      return -1;
    }
    void restoreMediaFileBuildValues(Object[] args) {}
    Object providerMediaFileUtilsDirectoryCallback(Object[] args, String method) { return null; }
    void logInfo(String message) {}
    void check(boolean condition) { if (!condition) throw new AssertionError(); }
    /* 注入生产方法 */
  }
  public static File buildUniqueFile(File parent, String mimeType, String displayName) {
    return null;
  }
  public static File buildUniqueFile(File parent, String displayName) { return null; }
  public static File buildFile(File parent, String name, String extension) { return null; }
  static void check(boolean condition, String message) {
    if (!condition) throw new AssertionError(message);
  }
  public static void main(String[] args) throws Throwable {
    HookerFixture fixture = new HookerFixture();
    fixture.target = MediaFileUtilsRoutingFixture.class.getMethod(
        "buildUniqueFile", File.class, String.class, String.class);
    String mimeType = "text/plain";
    String pendingName = ".pending-123-owner-note.txt";
    Object[] call = {new File(PUBLIC_PARENT), mimeType, pendingName};
    Object returned = fixture.providerMediaFileUtilsCallback(call);
    check(fixture.received[0].equals(new File(PRIVATE_PARENT)), "父目录未重定向");
    check(Objects.equals(fixture.received[1], mimeType), "MIME 类型被改写");
    check(Objects.equals(fixture.received[2], pendingName), "显示名称被改写");
    check(returned.equals(new File(PRIVATE_PARENT, "created (1).txt")), "后端文件路径错误");
    check(MEDIA_FILE_BUILD_CONTEXT.get().publicPath.equals(
        new File(PUBLIC_PARENT, "created (1).txt").getPath()), "公共路径没有采用实际返回的唯一名称");

    fixture.target = MediaFileUtilsRoutingFixture.class.getMethod(
        "buildFile", File.class, String.class, String.class);
    String name = "photo";
    String extension = "jpg";
    Object[] nestedCall = {new Object[] {new File(PUBLIC_PARENT), name, extension}};
    fixture.providerMediaFileUtilsCallback(nestedCall);
    check(fixture.received[0].equals(new File(PRIVATE_PARENT)), "nested 父目录未重定向");
    check(Objects.equals(fixture.received[1], name), "文件名被改写");
    check(Objects.equals(fixture.received[2], extension), "扩展名被改写");

    fixture.target = MediaFileUtilsRoutingFixture.class.getMethod(
        "buildUniqueFile", File.class, String.class);
    String twoArgumentName = ".pending-456-report.pdf";
    fixture.providerMediaFileUtilsCallback(new Object[] {new File(PUBLIC_PARENT), twoArgumentName});
    check(Objects.equals(fixture.received[1], twoArgumentName), "双参数方法的文件名被改写");
  }
}
'''
        fixture = fixture.replace("/* 注入生产方法 */", "\n".join(methods))
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory) / "MediaFileUtilsRoutingFixture.java"
            source.write_text(fixture, encoding="utf-8")
            subprocess.run(["javac", "-d", directory, str(source)], check=True, timeout=45)
            subprocess.run(["java", "-ea", "-cp", directory, "MediaFileUtilsRoutingFixture"], check=True, timeout=30)


if __name__ == "__main__":
    unittest.main()
