"""执行真实媒体登记/提交方法，覆盖深层重定向、放行和 pending 发布边界。"""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from test_mount_metadata_scope import function_body

ROOT = Path(__file__).resolve().parents[2]
JAVA = shutil.which("java")
JAVAC = shutil.which("javac")


@unittest.skipUnless(JAVA and JAVAC, "需要 JDK 执行媒体提交回归")
class MediaPendingLifecycleTest(unittest.TestCase):
    def test_registration_and_publish_boundaries(self):
        source = (ROOT / "java_src/org/srx/hook/Hooker.java").read_text(encoding="utf-8")
        signatures = [
            "private static void rememberRedirectedMediaTarget(",
            "private static void commitRedirectedPendingFile(",
            "private static ContentValues findContentValues(",
            "private static boolean isPendingFileOf(",
        ]
        methods = []
        for signature in signatures:
            start = source.index(signature)
            brace = source.index("{", start)
            methods.append(source[start:brace] + function_body(source, signature))
        fixture = r'''
import java.util.*;
import java.io.File;
import java.nio.file.*;
public class PendingFixture {
  static final int ANDROID_APP_UID_START = 10000;
  static final int REDIRECTED_MEDIA_TARGET_LIMIT = 256;
  static final LinkedHashMap<String,String> REDIRECTED_MEDIA_TARGETS = new LinkedHashMap<>();
  static class ContentValues extends HashMap<String,Object> {
    String getAsString(String k) { return (String)get(k); }
    Integer getAsInteger(String k) { return (Integer)get(k); }
  }
  static String mapping;
  static String buildMediaStoreProbePath(String r,String n,int uid) { return "/storage/emulated/0/"+r+n; }
  static String rewriteMediaStorePath(String p,int uid) { return mapping; }
  static String relativePathFromDirectoryColumns(ContentValues v) { return null; }
  static String firstString(ContentValues v,String a,String b) { return v.getAsString(a); }
  static String normalizeStorageDisplayPath(String p,int uid) { return p; }
  static void logInfo(String s) {}
  static void logWarn(String s,Throwable t) { throw new AssertionError(t); }
  static int findMutationUriIndex(Object[] args) {
    for(int i=0;i<args.length;i++) if(args[i] instanceof android.net.Uri) return i;
    return -1;
  }
  static void check(boolean value) { if(!value) throw new AssertionError(); }
  public static void main(String[] args) throws Exception {
    Path root=Paths.get(args[0]);
    Path target=root.resolve("photo.jpg");
    android.net.Uri uri=new android.net.Uri("content://media/external/file/1");
    ContentValues insert=new ContentValues();
    insert.put("relative_path","Pictures/"); insert.put("_display_name","photo.jpg");
    mapping=target.toString();
    rememberRedirectedMediaTarget(new Object[]{insert},uri,10123,"insert",true,false);
    check(mapping.equals(REDIRECTED_MEDIA_TARGETS.get(uri.toString())));
    REDIRECTED_MEDIA_TARGETS.clear();
    rememberRedirectedMediaTarget(new Object[]{insert},uri,10123,"insert",false,true);
    check(REDIRECTED_MEDIA_TARGETS.isEmpty());
    mapping=null; insert.put("_data",target.toString());
    rememberRedirectedMediaTarget(new Object[]{insert},uri,10123,"insert",true,false);
    check(REDIRECTED_MEDIA_TARGETS.isEmpty());
    mapping="/storage/emulated/0/Pictures/photo.jpg";
    rememberRedirectedMediaTarget(new Object[]{insert},uri,10123,"insert",true,false);
    check(REDIRECTED_MEDIA_TARGETS.isEmpty());
    mapping=null;
    insert.put("_data",root.resolve(".pending-123-photo.jpg").toString());
    rememberRedirectedMediaTarget(new Object[]{insert},uri,10123,"insert",true,true);
    check(target.toString().equals(REDIRECTED_MEDIA_TARGETS.get(uri.toString())));
    Path pending=root.resolve(".pending-123-photo.jpg");
    Path unrelated=root.resolve(".pending-124-other-photo.jpg");
    Files.writeString(pending,"payload"); Files.writeString(unrelated,"other");
    ContentValues update=new ContentValues();
    Object[] mutation={uri,update};
    commitRedirectedPendingFile(mutation,1,"update");
    check(REDIRECTED_MEDIA_TARGETS.containsKey(uri.toString()) && Files.exists(pending));
    update.put("is_pending",1);
    commitRedirectedPendingFile(mutation,1,"update");
    check(Files.exists(pending));
    update.put("is_pending",0);
    commitRedirectedPendingFile(mutation,0,"update");
    check(REDIRECTED_MEDIA_TARGETS.containsKey(uri.toString()) && Files.exists(pending));
    commitRedirectedPendingFile(mutation,1,"update");
    check(Files.readString(target).equals("payload") && !Files.exists(pending));
    check(Files.exists(unrelated) && REDIRECTED_MEDIA_TARGETS.isEmpty());
  }
'''
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            uri_dir = root / "android/net"
            uri_dir.mkdir(parents=True)
            (uri_dir / "Uri.java").write_text(
                'package android.net; public class Uri { private final String v; public Uri(String s){v=s;} public String toString(){return v;} }',
                encoding="utf-8",
            )
            (root / "PendingFixture.java").write_text(fixture + "\n".join(methods) + "\n}", encoding="utf-8")
            compile_result = subprocess.run([JAVAC, "-encoding", "UTF-8", "PendingFixture.java", "android/net/Uri.java"], cwd=root, capture_output=True, text=True, encoding="utf-8", timeout=60)
            self.assertEqual(0, compile_result.returncode, compile_result.stderr)
            result = subprocess.run([JAVA, "-cp", str(root), "PendingFixture", str(root)], cwd=root, capture_output=True, text=True, encoding="utf-8", timeout=30)
            self.assertEqual(0, result.returncode, result.stderr)


@unittest.skipUnless(JAVA and JAVAC, "需要 JDK 执行媒体提交回归")
class MediaPublicPhysicalFallbackTest(unittest.TestCase):
    """执行真实的 mediaStorePublicPhysicalFallback，覆盖父跟随限制与放行边界。

    mediaStorePublicPhysicalFallback 是本次改动的方法：只有 pending 名称才跟随父目录
    的 native 沙箱目标，普通放行路径即便父目录落在沙箱内也必须保留公共物理落点。
    这里抽取 Hooker.java 中的真实方法执行，并 stub 掉 native 重写、safe/enabled/physical
    四个依赖；pending 判据抽取 Hooker.java 内嵌 FilteringCursor 类的真实方法。
    """

    def test_fallback_follows_sandbox_parent_only_for_pending(self):
        source = (ROOT / "java_src/org/srx/hook/Hooker.java").read_text(encoding="utf-8")
        fallback_sig = "private static String mediaStorePublicPhysicalFallback("
        fallback_start = source.index(fallback_sig)
        fallback = source[fallback_start : source.index("{", fallback_start)] + function_body(
            source, fallback_sig
        )
        pending_sig = "private static boolean isMediaStorePendingPath("
        pending_start = source.index(pending_sig)
        pending = source[pending_start : source.index("{", pending_start)] + function_body(
            source, pending_sig
        )
        # 保留内嵌类的方法实现，仅开放测试夹具中的方法可见性。
        pending = pending.replace(
            "private static boolean isMediaStorePendingPath",
            "public static boolean isMediaStorePendingPath",
            1,
        )
        prefix = r'''
import java.util.*;
public class PublicFallbackFixture {
  // stub 依赖：native 重写只对被显式登记（parent -> target）的路径返回目标。
  static boolean redirectEnabled = true;
  static boolean safePublic = true;
  static final Map<String,String> nativeTargets = new HashMap<>();
  static String resolveMediaStoreDirectPath(String path, int uid) {
    return nativeTargets.get(path);
  }
  static boolean isRedirectEnabledForCallerUid(int uid) { return redirectEnabled; }
  static boolean isSafePublicMediaValuePath(String path) { return safePublic; }
  static int userIdFromUid(int uid) { return uid < 0 ? -1 : uid / 100000; }
  static boolean isSrxSandboxFallbackPath(String path, int uid) {
    return path != null && path.indexOf("/Android/data/") >= 0;
  }
  // mediaStorePhysicalPath 用与源码一致的 /storage/emulated -> /data/media 变换实现。
  static String mediaStorePhysicalPath(String path, int callerUid) {
    if (path == null || path.length() == 0) return null;
    boolean hasFileScheme = path.startsWith("file://");
    String value = hasFileScheme ? path.substring(6) : path;
    int userId = userIdFromUid(callerUid);
    if (userId < 0) return null;
    String displayRoot = "/storage/emulated/" + userId + "/";
    if (value.startsWith(displayRoot)) {
      String physical = "/data/media/" + userId + "/" + value.substring(displayRoot.length());
      return hasFileScheme ? "file://" + physical : physical;
    }
    String physicalRoot = "/data/media/" + userId + "/";
    if (value.startsWith(physicalRoot)) return path;
    return null;
  }
  static void check(boolean value) { if(!value) throw new AssertionError("check failed"); }
  static class FilteringCursor {
'''
        main = r'''
  public static void main(String[] args) throws Exception {
    final int UID = 10123;
    String sandboxParent = "/storage/emulated/0/Pictures";
    String sandboxTarget = "/storage/emulated/0/Android/data/com.example.app/sdcard/Pictures";
    String publicRoot = "/storage/emulated/0/";

    // A: 放行的普通目录，其父 native 落在沙箱内，也必须保留公共物理落点（不跟随父）。
    redirectEnabled = true; safePublic = true;
    nativeTargets.clear();
    nativeTargets.put(sandboxParent, sandboxTarget);
    String dir = publicRoot + "Pictures/Albums";
    String a = mediaStorePublicPhysicalFallback(dir, UID);
    check(a != null);
    check(a.equals("/data/media/0/Pictures/Albums"));
    check(!a.contains("Android/data"));

    // B: 放行的普通文件保持公共物理落点。
    nativeTargets.clear();
    String file = publicRoot + "Pictures/photo.jpg";
    String b = mediaStorePublicPhysicalFallback(file, UID);
    check(b != null);
    check(b.equals("/data/media/0/Pictures/photo.jpg"));
    check(!b.contains("Android/data"));

    // C: pending 名称且父 native 落在沙箱内，跟随父目标。
    nativeTargets.clear();
    nativeTargets.put(sandboxParent, sandboxTarget);
    String pendingSandbox = publicRoot + "Pictures/.pending-123-photo.jpg";
    String c = mediaStorePublicPhysicalFallback(pendingSandbox, UID);
    check(c != null);
    check(c.equals("/data/media/0/Android/data/com.example.app/sdcard/Pictures/.pending-123-photo.jpg"));
    check(c.contains("Android/data"));

    // D: pending 名称但父 native 为 null，保留公共物理落点。
    nativeTargets.clear();
    String pendingNull = publicRoot + "Pictures/.pending-456-photo.jpg";
    String d = mediaStorePublicPhysicalFallback(pendingNull, UID);
    check(d != null);
    check(d.equals("/data/media/0/Pictures/.pending-456-photo.jpg"));
    check(!d.contains("Android/data"));

    // E: 重定向未启用直接拒绝。
    redirectEnabled = false;
    check(mediaStorePublicPhysicalFallback(file, UID) == null);
    redirectEnabled = true;

    // F: 不安全路径直接拒绝。
    safePublic = false;
    check(mediaStorePublicPhysicalFallback(file, UID) == null);
    safePublic = true;

    System.out.println("OK");
  }
'''
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            java_text = prefix + pending + "  }\n" + main + fallback + "}\n"
            (root / "PublicFallbackFixture.java").write_text(java_text, encoding="utf-8")
            compile_result = subprocess.run(
                [JAVAC, "-encoding", "UTF-8", "PublicFallbackFixture.java"],
                cwd=root, capture_output=True, text=True, encoding="utf-8", timeout=60,
            )
            self.assertEqual(0, compile_result.returncode, compile_result.stderr)
            result = subprocess.run(
                [JAVA, "-cp", str(root), "PublicFallbackFixture"],
                cwd=root, capture_output=True, text=True, encoding="utf-8", timeout=30,
            )
            self.assertEqual(0, result.returncode, result.stderr)


class MediaDirectoryScopeTest(unittest.TestCase):
    def test_column_redirect_registers_existing_scoped_cleanup(self):
        source = (ROOT / "java_src/org/srx/hook/Hooker.java").read_text(encoding="utf-8")
        body = function_body(source, "public Object providerMediaFileColumnCallback(")
        self.assertIn("rememberProviderRedirectSourceDirectory(publicParent.getPath(), directParent.getPath())", body)
        self.assertLess(body.index("finally"), body.index("rememberProviderRedirectSourceDirectory"))
        native = (ROOT / "src/hook/ops/mutation/dir.rs").read_text(encoding="utf-8")
        cleanup = function_body(native, "pub(crate) fn cleanup_provider_redirect_source_directory(")
        self.assertIn("is_public_default_sandbox_redirect", cleanup)
        self.assertIn("libc::rmdir", function_body(native, "fn cleanup_empty_redirect_source_dir("))


class FuseBackingLifetimeTest(unittest.TestCase):
    def test_both_reply_paths_retain_registration_until_release(self):
        source = (ROOT / "src/fuse_redirect/mod.rs").read_text(encoding="utf-8")
        self.assertIn("_backing: Option<Arc<fuser::BackingId>>", source)
        for signature, reply in (("fn open(", "reply.opened_passthrough"), ("fn create(", "reply.created_passthrough")):
            body = function_body(source, signature)
            self.assertIn("_backing: None", body)
            self.assertIn("open_file._backing = Some(Arc::clone(&backing))", body)
            self.assertLess(body.index("open_file._backing ="), body.index(reply))
        self.assertIn("state.files.remove", function_body(source, "fn release("))


if __name__ == "__main__":
    unittest.main()
