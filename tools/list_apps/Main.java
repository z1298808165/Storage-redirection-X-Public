import android.content.Context;
import android.content.pm.ApplicationInfo;
import android.content.pm.PackageManager;
import android.os.Looper;
import java.lang.reflect.Method;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;

public final class Main {
  /** 系统应用标记行前缀；读取端遇到 `#` 开头的行会整体跳过，因此旧版本仍可解析新输出。 */
  private static final String SYSTEM_MARKER = "#system=";

  private Main() {}

  public static void main(String[] args) throws Exception {
    int userId = parseUserId(args);
    tryPrepareMainLooper();
    Context context = getSystemContext();
    PackageManager pm = context.getPackageManager();
    List<ApplicationInfo> apps = getInstalledApplications(pm, userId);
    Collections.sort(
        apps,
        new Comparator<ApplicationInfo>() {
          @Override
          public int compare(ApplicationInfo a, ApplicationInfo b) {
            return safe(a.packageName).compareTo(safe(b.packageName));
          }
        });
    StringBuilder out = new StringBuilder();
    StringBuilder systemOut = new StringBuilder();
    for (ApplicationInfo app : apps) {
      if (app == null || app.packageName == null || app.packageName.length() == 0) continue;
      CharSequence label = null;
      try {
        label = pm.getApplicationLabel(app);
      } catch (Throwable ignored) {
      }
      out.append(app.packageName)
          .append('=')
          .append(cleanLabel(label == null ? app.packageName : label.toString()))
          .append('\n');
      // 系统标记必须来自该用户的 PackageManager 结果：APK 路径无法区分更新后的
      // 系统应用与 APEX 应用，按路径猜测会把它们误判为用户应用。
      if (isSystemApp(app)) {
        if (systemOut.length() > 0) systemOut.append(' ');
        systemOut.append(app.packageName);
      }
    }
    if (systemOut.length() > 0) {
      out.append(SYSTEM_MARKER).append(systemOut).append('\n');
    }
    System.out.print(out.toString());
  }

  private static boolean isSystemApp(ApplicationInfo app) {
    // 与 `pm list packages -s` 一致：预装应用与「已更新的预装应用」都算系统应用。
    return (app.flags & ApplicationInfo.FLAG_SYSTEM) != 0
        || (app.flags & ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) != 0;
  }

  private static int parseUserId(String[] args) {
    int userId = 0;
    if (args == null) return userId;
    for (int i = 0; i < args.length; i++) {
      String arg = args[i];
      if (arg == null) continue;
      if (("--user".equals(arg) || "-u".equals(arg)) && i + 1 < args.length) {
        userId = parseInt(args[++i], userId);
      } else if (arg.startsWith("--user=")) {
        userId = parseInt(arg.substring("--user=".length()), userId);
      }
    }
    return userId;
  }

  private static int parseInt(String value, int fallback) {
    try {
      return Integer.parseInt(value);
    } catch (Throwable ignored) {
      return fallback;
    }
  }

  private static void tryPrepareMainLooper() {
    try {
      Looper.prepareMainLooper();
    } catch (Throwable ignored) {
    }
  }

  private static Context getSystemContext() throws Exception {
    Class<?> activityThreadClass = Class.forName("android.app.ActivityThread");
    Object thread = null;
    try {
      Method currentActivityThread = activityThreadClass.getDeclaredMethod("currentActivityThread");
      currentActivityThread.setAccessible(true);
      thread = currentActivityThread.invoke(null);
    } catch (Throwable ignored) {
    }
    if (thread == null) {
      Method systemMain = activityThreadClass.getDeclaredMethod("systemMain");
      systemMain.setAccessible(true);
      thread = systemMain.invoke(null);
    }
    Method getSystemContext = findMethod(activityThreadClass, "getSystemContext");
    if (getSystemContext == null)
      throw new NoSuchMethodException("ActivityThread.getSystemContext");
    getSystemContext.setAccessible(true);
    return (Context) getSystemContext.invoke(thread);
  }

  @SuppressWarnings("unchecked")
  private static List<ApplicationInfo> getInstalledApplications(PackageManager pm, int userId) {
    if (userId >= 0) {
      try {
        Method asUser =
            findMethod(pm.getClass(), "getInstalledApplicationsAsUser", int.class, int.class);
        if (asUser != null) {
          asUser.setAccessible(true);
          Object result = asUser.invoke(pm, 0, userId);
          if (result instanceof List) return (List<ApplicationInfo>) result;
        }
      } catch (Throwable ignored) {
      }
      try {
        Class<?> flagsClass =
            Class.forName("android.content.pm.PackageManager$ApplicationInfoFlags");
        Method of = flagsClass.getDeclaredMethod("of", long.class);
        Object flags = of.invoke(null, 0L);
        Method asUser =
            findMethod(pm.getClass(), "getInstalledApplicationsAsUser", flagsClass, int.class);
        if (asUser != null) {
          asUser.setAccessible(true);
          Object result = asUser.invoke(pm, flags, userId);
          if (result instanceof List) return (List<ApplicationInfo>) result;
        }
      } catch (Throwable ignored) {
      }
    }
    try {
      List<ApplicationInfo> apps = pm.getInstalledApplications(0);
      return apps == null ? new ArrayList<ApplicationInfo>() : apps;
    } catch (Throwable ignored) {
      return new ArrayList<ApplicationInfo>();
    }
  }

  private static Method findMethod(Class<?> cls, String name, Class<?>... params) {
    for (Class<?> current = cls; current != null; current = current.getSuperclass()) {
      try {
        return current.getDeclaredMethod(name, params);
      } catch (NoSuchMethodException ignored) {
      }
    }
    return null;
  }

  private static String cleanLabel(String value) {
    return safe(value).replace('\n', ' ').replace('\r', ' ').trim();
  }

  private static String safe(String value) {
    return value == null ? "" : value;
  }
}
