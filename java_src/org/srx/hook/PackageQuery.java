package org.srx.hook;

import android.content.pm.ApplicationInfo;
import android.content.pm.PackageInfo;
import android.net.LocalServerSocket;
import android.net.LocalSocket;
import android.net.LocalSocketAddress;
import android.os.IBinder;
import android.os.Process;
import android.system.Os;
import android.util.Log;
import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.io.File;
import java.io.InputStream;
import java.lang.reflect.Method;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.TreeSet;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/** system_server 内的包身份查询端；仅接受 root 请求，包查询缓存随 PM 快照变化失效。 */
public final class PackageQuery {
  private static final int MAX_REPLY = 4 * 1024 * 1024;
  private static final AtomicBoolean STARTED = new AtomicBoolean();
  private static final AtomicLong EVENTS = new AtomicLong();
  private static final Map<String, String> CACHE = new HashMap<>();
  private static java.lang.reflect.Field packageVersionField;
  private static java.lang.reflect.Field snapshotInvalidField;
  private static java.lang.reflect.Field snapshotField;
  private static boolean packageFieldsResolved;
  private static Object cachedGeneration;
  private static long cachedEvents;
  private static int cachedBytes;

  private PackageQuery() {}

  public static void invalidate() {
    EVENTS.incrementAndGet();
  }

  private static void resolvePackageFields(Object pm) {
    if (!packageFieldsResolved) {
      packageFieldsResolved = true;
      try {
        Class<?> pms =
            Class.forName(
                "com.android.server.pm.PackageManagerService",
                false,
                pm.getClass().getClassLoader());
        try {
          packageVersionField = pms.getDeclaredField("sSnapshotPendingVersion");
          packageVersionField.setAccessible(true);
          Log.i("SrxPackageQuery", "package cache generation available");
          return;
        } catch (NoSuchFieldException oldApi) {
          snapshotInvalidField = pms.getDeclaredField("sSnapshotInvalid");
          snapshotInvalidField.setAccessible(true);
          snapshotField = pms.getDeclaredField("mSnapshotComputer");
          snapshotField.setAccessible(true);
          Log.i("SrxPackageQuery", "package snapshot identity available");
        }
      } catch (Exception error) {
        Log.w("SrxPackageQuery", "package cache generation unavailable; querying live", error);
      }
    }
  }

  private static Object generation(Object pm) throws Exception {
    resolvePackageFields(pm);
    if (packageVersionField != null) {
      return Integer.valueOf(((AtomicInteger) packageVersionField.get(null)).get());
    }
    if (snapshotInvalidField == null
        || snapshotField == null
        || ((AtomicBoolean) snapshotInvalidField.get(null)).get()) return null;
    return snapshotField.get(pm);
  }

  private static boolean sameGeneration(Object first, Object second) {
    return first instanceof Integer && second instanceof Integer
        ? first.equals(second)
        : first == second;
  }

  private static void clearCache(Object generation, long events) {
    CACHE.clear();
    cachedBytes = 0;
    cachedGeneration = generation;
    cachedEvents = events;
  }

  // CACHE 只由查询线程读写；广播线程仅递增 EVENTS，不持有查询或 PM 的锁。
  private static String cachedQuery(String request, Object pm) throws Exception {
    // 用户增删不保证推进 PM 代次或触发包广播，全用户清单必须实时查询。
    if ("T".equals(request) || "L".equals(request)) return queryPackageManager(request, pm);
    Object before = generation(pm);
    long events = EVENTS.get();
    if (before == null) {
      clearCache(null, events);
      return queryPackageManager(request, pm);
    }
    if (!sameGeneration(before, cachedGeneration) || events != cachedEvents) {
      clearCache(before, events);
    }
    String reply = CACHE.get(request);
    if (reply != null && sameGeneration(before, generation(pm)) && EVENTS.get() == events)
      return reply;
    reply = queryPackageManager(request, pm);
    int replyBytes = reply.getBytes(StandardCharsets.UTF_8).length;
    if (sameGeneration(before, generation(pm))
        && EVENTS.get() == events
        && replyBytes <= MAX_REPLY - 3) {
      if (CACHE.size() >= 1024 || cachedBytes + replyBytes > MAX_REPLY - 3) {
        CACHE.clear();
        cachedBytes = 0;
      }
      String previous = CACHE.put(request, reply);
      cachedBytes +=
          replyBytes - (previous == null ? 0 : previous.getBytes(StandardCharsets.UTF_8).length);
    }
    return reply;
  }

  public static void install(String moduleDir) {
    if (Process.myUid() != Process.SYSTEM_UID || !STARTED.compareAndSet(false, true)) return;
    Thread thread =
        new Thread(
            () -> {
              File socket = new File(moduleDir, "tmp/package_query/socket");
              while (true) {
                try {
                  if (socket.exists() && !socket.delete())
                    throw new IllegalStateException("stale socket");
                  serve(socket.getPath());
                } catch (Exception error) {
                  Log.w("SrxPackageQuery", "query endpoint unavailable", error);
                  try {
                    Thread.sleep(1000);
                  } catch (InterruptedException stop) {
                    Thread.currentThread().interrupt();
                    return;
                  }
                }
              }
            },
            "SrxPackageQuery");
    thread.setDaemon(true);
    thread.start();
  }

  private static void serve(String path) throws Exception {
    try (LocalSocket bound = new LocalSocket()) {
      bound.bind(new LocalSocketAddress(path, LocalSocketAddress.Namespace.FILESYSTEM));
      Os.chmod(path, 0600);
      try (LocalServerSocket server = new LocalServerSocket(bound.getFileDescriptor())) {
        Log.i(
            "SrxPackageQuery",
            "query endpoint listening uptime_ms=" + android.os.SystemClock.uptimeMillis());
        while (true) {
          try (LocalSocket client = server.accept()) {
            if (client.getPeerCredentials().getUid() != 0) continue;
            client.setSoTimeout(2000);
            byte[] response;
            try {
              response =
                  ("OK\n" + query(readRequest(client.getInputStream())))
                      .getBytes(StandardCharsets.UTF_8);
              if (response.length > MAX_REPLY) throw new IllegalStateException("reply too large");
            } catch (Exception error) {
              Log.w("SrxPackageQuery", "package lookup failed", error);
              response =
                  "ERR\nPackageManager unavailable or invalid request\n"
                      .getBytes(StandardCharsets.UTF_8);
            }
            DataOutputStream out = new DataOutputStream(client.getOutputStream());
            out.writeInt(response.length);
            out.write(response);
            out.flush();
          } catch (Exception error) {
            Log.w("SrxPackageQuery", "query client failed", error);
          }
        }
      }
    }
  }

  private static String readRequest(InputStream in) throws Exception {
    ByteArrayOutputStream bytes = new ByteArrayOutputStream();
    for (int i = 0; i < 512; i++) {
      int b = in.read();
      if (b == '\n') return bytes.toString("UTF-8");
      if (b < 0 || b == 0 || b > 127) break;
      bytes.write(b);
    }
    throw new IllegalArgumentException("invalid request");
  }

  private static Object service(String name, String stub) throws Exception {
    Class<?> manager = Class.forName("android.os.ServiceManager");
    IBinder binder = (IBinder) manager.getMethod("checkService", String.class).invoke(null, name);
    if (binder == null || !binder.isBinderAlive())
      throw new IllegalStateException(name + " not ready");
    return Class.forName(stub).getMethod("asInterface", IBinder.class).invoke(null, binder);
  }

  private static Object call(
      Object target, String api, String name, Class<?>[] types, Object... args) throws Exception {
    return Class.forName(api).getMethod(name, types).invoke(target, args);
  }

  // Android 13 将包查询的 flags 从 int 扩展为 long；使用接口声明匹配两种签名。
  private static Object packageCall(Object pm, String name, String packageName, int user)
      throws Exception {
    Class<?> api = Class.forName("android.content.pm.IPackageManager");
    for (Method method : api.getMethods()) {
      Class<?>[] types = method.getParameterTypes();
      if (!method.getName().equals(name)) continue;
      if (packageName == null && types.length == 2 && types[1] == int.class) {
        Object flags = types[0] == long.class ? (Object) Long.valueOf(0) : Integer.valueOf(0);
        return method.invoke(pm, flags, user);
      }
      if (packageName != null
          && types.length == 3
          && types[0] == String.class
          && types[2] == int.class) {
        Object flags = types[1] == long.class ? (Object) Long.valueOf(0) : Integer.valueOf(0);
        return method.invoke(pm, packageName, flags, user);
      }
    }
    throw new NoSuchMethodException(name);
  }

  private static int nonnegative(String value) {
    int number = Integer.parseInt(value);
    if (number < 0) throw new IllegalArgumentException();
    return number;
  }

  private static List<?> users(Object manager) throws Exception {
    Class<?> api = Class.forName("android.os.IUserManager");
    try {
      return (List<?>) api.getMethod("getUsers", boolean.class).invoke(manager, true);
    } catch (NoSuchMethodException oldApi) {
      return (List<?>)
          api.getMethod("getUsers", boolean.class, boolean.class, boolean.class)
              .invoke(manager, true, true, true);
    }
  }

  private static Object systemPackageManager() throws Exception {
    Class<?> activityThread = Class.forName("android.app.ActivityThread");
    Object thread = null;
    try {
      Method current = activityThread.getDeclaredMethod("currentActivityThread");
      current.setAccessible(true);
      thread = current.invoke(null);
    } catch (NoSuchMethodException ignored) {
    }
    if (thread == null) {
      Method systemMain = activityThread.getDeclaredMethod("systemMain");
      systemMain.setAccessible(true);
      thread = systemMain.invoke(null);
    }
    Method getSystemContext = activityThread.getDeclaredMethod("getSystemContext");
    getSystemContext.setAccessible(true);
    Object context = getSystemContext.invoke(thread);
    return context.getClass().getMethod("getPackageManager").invoke(context);
  }

  private static String applicationLabel(ApplicationInfo app, Object packageManager) {
    try {
      Class<?> api = Class.forName("android.content.pm.PackageManager");
      Object label = app.getClass().getMethod("loadLabel", api).invoke(app, packageManager);
      if (label != null && label.toString().length() > 0) return label.toString();
    } catch (Exception ignored) {
    }
    return app.packageName;
  }

  private static String encodeField(String value) {
    byte[] bytes = value.getBytes(StandardCharsets.UTF_8);
    StringBuilder encoded = new StringBuilder(bytes.length);
    final char[] hex = "0123456789ABCDEF".toCharArray();
    for (byte raw : bytes) {
      int valueByte = raw & 0xff;
      if ((valueByte >= 'a' && valueByte <= 'z')
          || (valueByte >= 'A' && valueByte <= 'Z')
          || (valueByte >= '0' && valueByte <= '9')
          || valueByte == '.'
          || valueByte == '_'
          || valueByte == '-'
          || valueByte == '~') {
        encoded.append((char) valueByte);
      } else {
        encoded.append('%').append(hex[valueByte >>> 4]).append(hex[valueByte & 0xf]);
      }
    }
    return encoded.toString();
  }

  private static String query(String request) throws Exception {
    Object pm = service("package", "android.content.pm.IPackageManager$Stub");
    return cachedQuery(request, pm);
  }

  private static String queryPackageManager(String request, Object pm) throws Exception {
    String[] fields = request.split("\t", -1);
    TreeSet<String> rows = new TreeSet<>();
    if (fields.length == 3 && fields[0].equals("P")) {
      int user = nonnegative(fields[1]);
      if (!fields[2].matches("[A-Za-z0-9_]+(\\.[A-Za-z0-9_]+)*")) {
        throw new IllegalArgumentException();
      }
      int uid = (Integer) packageCall(pm, "getPackageUid", fields[2], user);
      if (uid >= 0) rows.add(fields[2] + ":" + uid);
    } else if (fields.length == 2 && fields[0].equals("A")) {
      int user = nonnegative(fields[1]);
      Object slice = packageCall(pm, "getInstalledApplications", null, user);
      List<?> apps = (List<?>) slice.getClass().getMethod("getList").invoke(slice);
      ArrayList<ApplicationInfo> sorted = new ArrayList<>();
      for (Object entry : apps) sorted.add((ApplicationInfo) entry);
      Collections.sort(
          sorted,
          new Comparator<ApplicationInfo>() {
            @Override
            public int compare(ApplicationInfo first, ApplicationInfo second) {
              return first.packageName.compareTo(second.packageName);
            }
          });
      Object packageManager = null;
      try {
        packageManager = systemPackageManager();
      } catch (Exception ignored) {
      }
      StringBuilder result = new StringBuilder();
      for (ApplicationInfo app : sorted) {
        if (app.packageName == null || app.packageName.length() == 0) continue;
        if (app.uid < 0 || app.uid / 100000 != user)
          throw new IllegalStateException("PackageManager returned mismatched user UID");
        boolean isSystem =
            (app.flags & ApplicationInfo.FLAG_SYSTEM) != 0
                || (app.flags & ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) != 0;
        result
            .append(user)
            .append('\t')
            .append(app.uid)
            .append('\t')
            .append(isSystem ? '1' : '0')
            .append('\t')
            .append(app.packageName)
            .append('\t')
            .append(encodeField(applicationLabel(app, packageManager)))
            .append('\t')
            .append(encodeField(app.sourceDir == null ? "" : app.sourceDir))
            .append('\n');
      }
      return result.toString();
    } else if (fields.length == 1 && fields[0].equals("T")) {
      Object manager = service("user", "android.os.IUserManager$Stub");
      TreeSet<Integer> ids = new TreeSet<>();
      for (Object user : users(manager)) ids.add(user.getClass().getField("id").getInt(user));
      StringBuilder result = new StringBuilder();
      for (Integer id : ids) result.append(id).append('\n');
      if (result.length() == 0) throw new IllegalStateException("empty user inventory");
      return result.toString();
    } else if (fields.length == 2 && (fields[0].equals("U") || fields[0].equals("S"))) {
      int uid = nonnegative(fields[1]);
      String[] packages =
          (String[])
              call(
                  pm,
                  "android.content.pm.IPackageManager",
                  "getPackagesForUid",
                  new Class<?>[] {int.class},
                  uid);
      if (fields[0].equals("S")) {
        if (packages == null || packages.length < 2) return "";
        String sharedUserId = "";
        for (String name : packages) {
          PackageInfo info = (PackageInfo) packageCall(pm, "getPackageInfo", name, uid / 100000);
          if (info == null
              || info.applicationInfo == null
              || info.applicationInfo.uid != uid
              || !name.equals(info.packageName)
              || info.sharedUserId == null
              || !info.sharedUserId.matches("[A-Za-z0-9_]+(\\.[A-Za-z0-9_]+)+")) return "";
          if (!sharedUserId.isEmpty() && !sharedUserId.equals(info.sharedUserId)) return "";
          sharedUserId = info.sharedUserId;
        }
        return sharedUserId + "\n";
      }
      if (packages != null) for (String name : packages) rows.add(name + ":" + uid);
    } else if (fields.length == 1 && fields[0].equals("L")) {
      Object users = service("user", "android.os.IUserManager$Stub");
      List<?> list = users(users);
      for (Object user : list) {
        int id = user.getClass().getField("id").getInt(user);
        Object slice = packageCall(pm, "getInstalledApplications", null, id);
        List<?> apps = (List<?>) slice.getClass().getMethod("getList").invoke(slice);
        for (Object entry : apps) {
          ApplicationInfo app = (ApplicationInfo) entry;
          rows.add(app.packageName + ":" + app.uid);
        }
      }
      if (rows.isEmpty()) throw new IllegalStateException("empty package inventory");
    } else {
      throw new IllegalArgumentException("unknown operation");
    }
    return rows.isEmpty() ? "" : String.join("\n", rows) + "\n";
  }
}
