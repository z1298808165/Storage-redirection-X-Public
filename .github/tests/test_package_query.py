"""执行包查询的反射与请求解析，覆盖 Android 接口签名差异和畸形输入。"""

from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from test_mount_metadata_scope import function_body

ROOT = Path(__file__).resolve().parents[2]


@unittest.skipUnless(shutil.which("java") and shutil.which("javac"), "需要 JDK")
class PackageQueryTest(unittest.TestCase):
    def test_package_manager_generation_variants(self):
        source = (ROOT / "java_src/org/srx/hook/PackageQuery.java").read_text(encoding="utf-8")
        signatures = (
            "private static void resolvePackageFields(",
            "private static Object generation(",
            "private static boolean sameGeneration(",
        )
        methods = []
        for signature in signatures:
            start = source.index(signature)
            methods.append(source[start:source.index("{", start)] + function_body(source, signature))
        for fields, expected in (
            (
                "public static java.util.concurrent.atomic.AtomicInteger sSnapshotPendingVersion = new java.util.concurrent.atomic.AtomicInteger(7);",
                "MODERN",
            ),
            (
                "public static java.util.concurrent.atomic.AtomicBoolean sSnapshotInvalid = new java.util.concurrent.atomic.AtomicBoolean(true); public volatile Object mSnapshotComputer;",
                "LEGACY",
            ),
            ("", "UNSUPPORTED"),
        ):
            with self.subTest(api=expected), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                service = root / "com/android/server/pm/PackageManagerService.java"
                service.parent.mkdir(parents=True)
                service.write_text(
                    "package com.android.server.pm; public class PackageManagerService { "
                    + fields
                    + " }",
                    encoding="utf-8",
                )
                if expected == "MODERN":
                    scenario = """
      Object before = generation(pm);
      check(before.equals(Integer.valueOf(7)));
      com.android.server.pm.PackageManagerService.sSnapshotPendingVersion.incrementAndGet();
      check(!sameGeneration(before, generation(pm)));
                    """
                elif expected == "LEGACY":
                    scenario = """
      check(generation(pm) == null);
      com.android.server.pm.PackageManagerService.sSnapshotInvalid.set(false);
      Object first = new Object();
      ((com.android.server.pm.PackageManagerService) pm).mSnapshotComputer = first;
      check(generation(pm) == first);
      check(sameGeneration(first, generation(pm)));
      Object next = new Object();
      ((com.android.server.pm.PackageManagerService) pm).mSnapshotComputer = next;
      check(!sameGeneration(first, generation(pm)));
      com.android.server.pm.PackageManagerService.sSnapshotInvalid.set(true);
      check(generation(pm) == null);
                    """
                else:
                    scenario = "      check(generation(pm) == null);\n"
                fixture = """
import java.lang.reflect.Field;
import java.util.concurrent.atomic.*;
public class GenerationFixture {
  static Field packageVersionField, snapshotInvalidField, snapshotField;
  static boolean packageFieldsResolved;
  static class Log { static void i(String a,String b) {} static void w(String a,String b,Throwable t) {} }
""" + "\n".join(methods) + """
  static void check(boolean value) { if(!value) throw new AssertionError(); }
  public static void main(String[] args) throws Exception {
    Object pm = new com.android.server.pm.PackageManagerService();
""" + scenario + """
    System.out.println("PASS " + args[0]);
  }
}
                """
                target = root / "GenerationFixture.java"
                target.write_text(fixture, encoding="utf-8")
                compiled = subprocess.run(
                    ["javac", "--release", "11", str(service), str(target)],
                    cwd=root,
                    check=False,
                    capture_output=True,
                    text=True,
                    timeout=30,
                )
                self.assertEqual(compiled.returncode, 0, compiled.stderr)
                result = subprocess.run(
                    ["java", "-cp", str(root), "GenerationFixture", expected],
                    cwd=root,
                    check=True,
                    capture_output=True,
                    text=True,
                    timeout=10,
                )
                self.assertEqual(result.stdout.strip(), f"PASS {expected}")

    def test_cache_invalidates_before_broadcast_and_never_caches_query_failures(self):
        source = (ROOT / "java_src/org/srx/hook/PackageQuery.java").read_text(encoding="utf-8")
        signature = "private static String cachedQuery("
        start = source.index(signature)
        method = source[start:source.index("{", start)] + function_body(source, signature)
        fixture = r'''
import java.util.*;
import java.util.concurrent.atomic.*;
public class CacheFixture {
  static final int MAX_REPLY = 4 * 1024 * 1024;
  static final AtomicLong EVENTS = new AtomicLong();
  static final Map<String,String> CACHE = new HashMap<>();
  static Object cachedGeneration;
  static int cachedBytes, calls;
  static long cachedEvents;
  static AtomicInteger epoch = new AtomicInteger(1);
  static String live = "";
  static boolean fail, changeDuringQuery;
  static Object generation(Object pm) { return epoch == null ? null : Integer.valueOf(epoch.get()); }
  static boolean sameGeneration(Object a, Object b) { return a instanceof Integer && a.equals(b); }
  static void clearCache(Object generation, long events) {
    CACHE.clear(); cachedBytes=0; cachedGeneration=generation; cachedEvents=events;
  }
  static String queryPackageManager(String request, Object pm) throws Exception {
    calls++;
    if (fail) throw new IllegalStateException("backend unavailable");
    if (changeDuringQuery) { epoch.incrementAndGet(); changeDuringQuery=false; }
    return live;
  }
  static void check(boolean value) { if (!value) throw new AssertionError(); }
  static String query(String request) throws Exception { return cachedQuery(request, null); }
METHOD
  public static void main(String[] args) throws Exception {
    check(query("P\t0\tcom.new.app").isEmpty());
    for (int i=0;i<100;i++) check(query("P\t0\tcom.new.app").isEmpty());
    check(calls == 1);
    live="com.new.app:10123\n";
    epoch.incrementAndGet(); // 安装已提交，广播尚未收到。
    check(EVENTS.get() == 0);
    check(query("P\t0\tcom.new.app").equals(live));
    check(calls == 2);
    epoch.incrementAndGet(); // 卸载与 UID 复用不能继续命中旧包。
    live="";
    check(query("P\t0\tcom.new.app").isEmpty());
    live="com.other.app:10123\n";
    check(query("U\t10123").equals(live));
    EVENTS.incrementAndGet();
    live="com.other.app:10123\ncom.shared.app:10123\n";
    check(query("U\t10123").equals(live));
    epoch.incrementAndGet();
    fail=true;
    try { query("U\t10123"); throw new AssertionError(); }
    catch (IllegalStateException expected) {}
    fail=false;
    int before=calls;
    check(query("U\t10123").equals(live));
    check(calls == before+1);
    epoch.incrementAndGet();
    changeDuringQuery=true;
    query("L");
    check(!CACHE.containsKey("L"));
    query("L");
    check(CACHE.containsKey("L"));
    epoch=null; // 平台不暴露版本时必须实时查询。
    before=calls;
    query("L"); query("L");
    check(calls == before+2);
    epoch=new AtomicInteger(100);
    live="x".repeat(MAX_REPLY);
    query("oversized");
    check(!CACHE.containsKey("oversized"));
    live="x";
    for(int i=0;i<1025;i++) query("P"+i);
    check(CACHE.size() <= 1024 && cachedBytes <= MAX_REPLY);
    live="x".repeat(MAX_REPLY/2+1);
    query("large1"); query("large2");
    check(cachedBytes <= MAX_REPLY);
    System.out.println("PASS package cache generation, failure and size boundaries");
  }
}
'''.replace("METHOD", method)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "CacheFixture.java").write_text(fixture, encoding="utf-8")
            subprocess.run(["javac", "--release", "11", "CacheFixture.java"], cwd=root,
                           check=True, capture_output=True, text=True, timeout=30)
            result = subprocess.run(["java", "-cp", str(root), "CacheFixture"],
                                    check=True, capture_output=True, text=True, timeout=10)
            self.assertEqual(result.stdout.strip(), "PASS package cache generation, failure and size boundaries")

    def test_interface_variants_and_request_boundaries(self):
        source = (ROOT / "java_src/org/srx/hook/PackageQuery.java").read_text(encoding="utf-8")
        methods = []
        for signature in (
            "private static Object call(",
            "private static Object packageCall(",
            "private static List<?> users(",
            "private static String readRequest(",
            "private static int nonnegative(",
            "private static String queryPackageManager(",
        ):
            start = source.index(signature)
            brace = source.index("{", start)
            methods.append(source[start:brace] + function_body(source, signature))
        for flags_type, user_args in (("int", "boolean a, boolean b, boolean c"), ("long", "boolean a")):
            with self.subTest(flags=flags_type, users=user_args), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                sources = {
                    "android/content/pm/IPackageManager.java": f"""
package android.content.pm;
public interface IPackageManager {{
  int getPackageUid(String name, {flags_type} flags, int user);
  Object getInstalledApplications({flags_type} flags, int user);
  String[] getPackagesForUid(int uid);
}}
""",
                    "android/content/pm/ApplicationInfo.java": """
package android.content.pm;
public class ApplicationInfo {
  public String packageName;
  public int uid;
  public ApplicationInfo(String name, int value) { packageName=name; uid=value; }
}
""",
                    "android/os/IUserManager.java": f"""
package android.os;
public interface IUserManager {{ java.util.List<?> getUsers({user_args}); }}
""",
                    "QueryFixture.java": """
import java.io.*;
import java.lang.reflect.*;
import java.util.*;
import android.content.pm.ApplicationInfo;
public class QueryFixture {
""" + "\n".join(methods) + f"""
  static boolean fail, empty;
  static Object service(String name, String stub) {{
    if (!name.equals("user")) throw new AssertionError();
    return new Users();
  }}
  public static final class Pm implements android.content.pm.IPackageManager {{
    public int getPackageUid(String name, {flags_type} flags, int user) {{
      if (flags != 0 || user != 10) throw new AssertionError();
      if (fail) throw new IllegalStateException("backend unavailable");
      return name.equals("com.example.app") ? 1010123 : -1;
    }}
    public Object getInstalledApplications({flags_type} flags, int user) {{
      if (flags != 0 || (user != 0 && user != 10)) throw new AssertionError();
      if (fail) throw new IllegalStateException("backend unavailable");
      return new Slice(user);
    }}
    public String[] getPackagesForUid(int uid) {{
      if (fail) throw new IllegalStateException("backend unavailable");
      return uid == 1010123 ? new String[] {{"com.example.sibling", "com.example.app", "com.example.app"}} : null;
    }}
  }}
  public static final class Slice {{
    final int uid;
    Slice(int user) {{ uid=user*100000+10123; }}
    public List<?> getList() {{
      return empty ? Collections.emptyList() : Arrays.asList(
          new ApplicationInfo("com.example.sibling",uid),
          new ApplicationInfo("com.example.app",uid));
    }}
  }}
  public static final class User {{ public int id; User(int value) {{ id=value; }} }}
  public static final class Users implements android.os.IUserManager {{
    public List<?> getUsers({user_args}) {{ return Arrays.asList(new User(0), new User(10)); }}
  }}
  interface Action {{ void run() throws Exception; }}
  static void rejects(Action action) throws Exception {{
    try {{ action.run(); }} catch (IllegalArgumentException expected) {{ return; }}
    throw new AssertionError("invalid request accepted");
  }}
  static void check(boolean value) {{ if(!value) throw new AssertionError(); }}
  static String query(String request) throws Exception {{ return queryPackageManager(request, new Pm()); }}
  public static void main(String[] args) throws Exception {{
    if (!packageCall(new Pm(), "getPackageUid", "com.example.app", 10).equals(1010123)) throw new AssertionError();
    check(packageCall(new Pm(), "getInstalledApplications", null, 10) instanceof Slice);
    check(users(new Users()).size() == 2);
    check(query("P\\t10\\tcom.example.app").equals("com.example.app:1010123\\n"));
    check(query("P\\t10\\tcom.missing.app").isEmpty());
    check(query("U\\t1010123").equals("com.example.app:1010123\\ncom.example.sibling:1010123\\n"));
    check(query("U\\t12345").isEmpty());
    check(query("L").equals("com.example.app:1010123\\ncom.example.app:10123\\ncom.example.sibling:1010123\\ncom.example.sibling:10123\\n"));
    for (String request : new String[] {{"", "L\\t", "P\\t10", "U\\t", "X", "U\\t-1", "U\\t4294967296", "P\\t-1\\tcom.example.app", "P\\t10\\t../bad", "P\\t10\\tcom..app"}})
      rejects(() -> query(request));
    empty=true;
    try {{ query("L"); throw new AssertionError(); }} catch (IllegalStateException expected) {{}}
    empty=false;
    fail=true;
    for(String request : new String[] {{"L", "P\\t10\\tcom.example.app", "U\\t1010123"}}) {{
      try {{ query(request); throw new AssertionError(); }} catch (InvocationTargetException expected) {{
        check(expected.getCause() instanceof IllegalStateException);
      }}
    }}
    fail=false;
    try {{ packageCall(new Pm(), "missing", null, 10); throw new AssertionError(); }}
    catch (NoSuchMethodException expected) {{}}
    if (!readRequest(new ByteArrayInputStream("P\\t10\\tcom.example.app\\n".getBytes("UTF-8"))).equals("P\\t10\\tcom.example.app")) throw new AssertionError();
    if (nonnegative("10") != 10) throw new AssertionError();
    for (String value : new String[] {{"-1", "overflow", "4294967296"}}) rejects(() -> nonnegative(value));
    for (String value : new String[] {{"no-newline", "bad\\0request\\n", "é\\n", "x".repeat(512) + "\\n"}}) {{
      rejects(() -> readRequest(new ByteArrayInputStream(value.getBytes("UTF-8"))));
    }}
    System.out.println("PASS package query interface and request boundaries");
  }}
}}
""",
                }
                for name, text in sources.items():
                    target = root / name
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text(text, encoding="utf-8")
                subprocess.run(["javac", "--release", "11", *sources], cwd=root, check=True,
                               capture_output=True, text=True, timeout=30)
                result = subprocess.run(["java", "-cp", str(root), "QueryFixture"],
                                        check=True, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.stdout.strip(), "PASS package query interface and request boundaries")

    def test_same_uid_cache_comparison_is_scoped_to_android_user(self):
        source = (ROOT / "src/fuse_redirect/policy.rs").read_text(encoding="utf-8")
        signature = "fn is_same_uid_private_write_path("
        start = source.index(signature)
        body = source[start:source.index("\n}\n", start) + 2]
        self.assertIn("extract_user_id_from_storage_path(storage_path) != session_uid / 100000", body)
        self.assertIn("app_id % 100000 == session_uid % 100000", body)
