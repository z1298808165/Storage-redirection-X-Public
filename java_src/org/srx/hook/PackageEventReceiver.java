// SPDX-License-Identifier: Apache-2.0

package org.srx.hook;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;
import android.util.Log;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileReader;
import java.io.FileWriter;
import java.lang.reflect.Field;
import java.lang.reflect.Method;
import java.util.Locale;
import java.util.concurrent.atomic.AtomicBoolean;

public final class PackageEventReceiver extends BroadcastReceiver {
  private static final String TAG = "SrxPackageEvent";
  private static final String EVENT_PREFIX = "srx-package-event-v1";
  private static final int ANDROID_USER_ID_OFFSET = 100000;
  private static final int RECEIVER_EXPORTED = 0x2;
  private static final int MAX_INSTALL_RETRY_COUNT = 120;
  private static final long INSTALL_RETRY_DELAY_MS = 1000L;
  private static final AtomicBoolean INSTALLED = new AtomicBoolean(false);
  private static final AtomicBoolean RETRY_SCHEDULED = new AtomicBoolean(false);
  private static volatile Handler mainHandler;
  private static volatile File eventFile;
  private static volatile File readyFile;
  private static volatile PackageEventReceiver receiver;

  private PackageEventReceiver() {}

  public static boolean install(String moduleDir) {
    if (moduleDir == null || moduleDir.length() == 0) {
      Log.w(TAG, "install failed: empty module dir");
      return false;
    }
    File logsDir = new File(moduleDir, "logs");
    eventFile = new File(logsDir, "package_events.log");
    readyFile = new File(logsDir, ".package_event_receiver_ready");

    if (tryInstall(true)) {
      return true;
    }
    scheduleInstallRetry();
    return true;
  }

  private static boolean tryInstall(boolean logFailure) {
    if (!INSTALLED.compareAndSet(false, true)) {
      writeReadySignal();
      Log.i(TAG, "receiver already installed");
      return true;
    }

    try {
      Context context = getSystemContext();
      if (context == null) {
        INSTALLED.set(false);
        if (logFailure) {
          Log.w(TAG, "install failed: system context unavailable");
        }
        return false;
      }

      IntentFilter filter = new IntentFilter();
      filter.addAction(Intent.ACTION_PACKAGE_ADDED);
      filter.addAction(Intent.ACTION_PACKAGE_REPLACED);
      filter.addAction(Intent.ACTION_PACKAGE_REMOVED);
      filter.addAction(Intent.ACTION_PACKAGE_FULLY_REMOVED);
      filter.addDataScheme("package");

      PackageEventReceiver newReceiver = new PackageEventReceiver();
      if (!registerForAllUsers(context, newReceiver, filter)) {
        registerForCurrentUser(context, newReceiver, filter);
      }
      receiver = newReceiver;
      writeReadySignal();
      Log.i(TAG, "receiver installed");
      return true;
    } catch (Throwable t) {
      INSTALLED.set(false);
      if (logFailure) {
        Log.w(TAG, "install failed", t);
      }
      return false;
    }
  }

  private static void scheduleInstallRetry() {
    if (!RETRY_SCHEDULED.compareAndSet(false, true)) {
      return;
    }
    Thread thread =
        new Thread(
            new Runnable() {
              @Override
              public void run() {
                for (int attempt = 1; attempt <= MAX_INSTALL_RETRY_COUNT; attempt++) {
                  try {
                    Thread.sleep(INSTALL_RETRY_DELAY_MS);
                  } catch (InterruptedException ignored) {
                    Thread.currentThread().interrupt();
                    RETRY_SCHEDULED.set(false);
                    return;
                  }
                  if (INSTALLED.get()) {
                    RETRY_SCHEDULED.set(false);
                    return;
                  }
                  boolean logFailure = attempt == 1 || attempt % 15 == 0;
                  if (tryInstall(logFailure)) {
                    RETRY_SCHEDULED.set(false);
                    Log.i(TAG, "receiver installed after retry attempt=" + attempt);
                    return;
                  }
                }
                RETRY_SCHEDULED.set(false);
                Log.w(TAG, "install retry exhausted");
              }
            },
            "SrxPackageEventRetry");
    thread.setDaemon(true);
    thread.start();
  }

  @Override
  public void onReceive(Context context, Intent intent) {
    if (intent == null) {
      return;
    }
    String action = normalizeAction(intent.getAction());
    if (action.length() == 0) {
      return;
    }

    Uri data = intent.getData();
    String packageName = data == null ? null : data.getSchemeSpecificPart();
    if (!isSafePackageName(packageName)) {
      return;
    }

    boolean replacing = intent.getBooleanExtra(Intent.EXTRA_REPLACING, false);
    int uid = intent.getIntExtra(Intent.EXTRA_UID, -1);
    int userId = resolveUserId(uid);
    String line = buildLine(action, userId, uid, replacing, packageName);
    appendEventAsync(line);

    // 为 ADD/REPLACE 保留第二次延迟事件，使 shell 侧验证在较慢设备上也能
    // 读取到稳定后的 PackageManager 状态。
    if ("added".equals(action) || "replaced".equals(action)) {
      Runnable delayedAppend =
          new Runnable() {
            @Override
            public void run() {
              appendEventAsync(buildLine(action, userId, uid, replacing, packageName));
            }
          };
      Handler handler = mainHandler();
      if (handler != null) {
        handler.postDelayed(delayedAppend, 2500L);
      } else {
        new Thread(
                new Runnable() {
                  @Override
                  public void run() {
                    try {
                      Thread.sleep(2500L);
                    } catch (InterruptedException ignored) {
                      Thread.currentThread().interrupt();
                      return;
                    }
                    delayedAppend.run();
                  }
                },
                "SrxPackageEventDelay")
            .start();
      }
    }
  }

  private static void appendEventAsync(final String line) {
    Thread thread =
        new Thread(
            new Runnable() {
              @Override
              public void run() {
                appendEvent(line);
              }
            },
            "SrxPackageEventWriter");
    thread.setDaemon(true);
    thread.start();
  }

  private static Handler mainHandler() {
    Handler handler = mainHandler;
    if (handler != null) {
      return handler;
    }
    Looper looper = Looper.getMainLooper();
    if (looper == null) {
      return null;
    }
    handler = new Handler(looper);
    mainHandler = handler;
    return handler;
  }

  private static boolean registerForAllUsers(
      Context context, BroadcastReceiver receiver, IntentFilter filter) {
    try {
      Class<?> userHandleClass = Class.forName("android.os.UserHandle");
      Field allField = userHandleClass.getDeclaredField("ALL");
      allField.setAccessible(true);
      Object allUsers = allField.get(null);
      Method method =
          Context.class.getMethod(
              "registerReceiverAsUser",
              BroadcastReceiver.class,
              userHandleClass,
              IntentFilter.class,
              String.class,
              Handler.class,
              int.class);
      method.invoke(context, receiver, allUsers, filter, null, mainHandler(), RECEIVER_EXPORTED);
      return true;
    } catch (Throwable ignored) {
    }

    try {
      Class<?> userHandleClass = Class.forName("android.os.UserHandle");
      Field allField = userHandleClass.getDeclaredField("ALL");
      allField.setAccessible(true);
      Object allUsers = allField.get(null);
      Method method =
          Context.class.getMethod(
              "registerReceiverAsUser",
              BroadcastReceiver.class,
              userHandleClass,
              IntentFilter.class,
              String.class,
              Handler.class);
      method.invoke(context, receiver, allUsers, filter, null, mainHandler());
      return true;
    } catch (Throwable t) {
      Log.w(TAG, "registerReceiverAsUser unavailable", t);
      return false;
    }
  }

  private static void registerForCurrentUser(
      Context context, BroadcastReceiver receiver, IntentFilter filter) {
    try {
      Method method =
          Context.class.getMethod(
              "registerReceiver", BroadcastReceiver.class, IntentFilter.class, int.class);
      method.invoke(context, receiver, filter, RECEIVER_EXPORTED);
      return;
    } catch (Throwable ignored) {
    }
    context.registerReceiver(receiver, filter);
  }

  private static Context getSystemContext() {
    try {
      Class<?> activityThreadClass = Class.forName("android.app.ActivityThread");
      Object thread = invokeStatic(activityThreadClass, "currentActivityThread");
      if (thread == null) {
        thread = invokeStatic(activityThreadClass, "systemMain");
      }
      if (thread != null) {
        Method getSystemContext = activityThreadClass.getDeclaredMethod("getSystemContext");
        getSystemContext.setAccessible(true);
        Object context = getSystemContext.invoke(thread);
        if (context instanceof Context) {
          return (Context) context;
        }
      }

      Object app = invokeStatic(activityThreadClass, "currentApplication");
      if (app instanceof Context) {
        return (Context) app;
      }
    } catch (Throwable t) {
      Log.w(TAG, "getSystemContext failed", t);
    }
    return null;
  }

  private static Object invokeStatic(Class<?> clazz, String name) {
    try {
      Method method = clazz.getDeclaredMethod(name);
      method.setAccessible(true);
      return method.invoke(null);
    } catch (Throwable ignored) {
      return null;
    }
  }

  private int resolveUserId(int uid) {
    try {
      Method method = BroadcastReceiver.class.getDeclaredMethod("getSendingUserId");
      method.setAccessible(true);
      Object value = method.invoke(this);
      if (value instanceof Integer) {
        int id = ((Integer) value).intValue();
        if (id >= 0) {
          return id;
        }
      }
    } catch (Throwable ignored) {
    }

    if (uid >= ANDROID_USER_ID_OFFSET) {
      return uid / ANDROID_USER_ID_OFFSET;
    }
    return 0;
  }

  private static String normalizeAction(String action) {
    if (Intent.ACTION_PACKAGE_ADDED.equals(action)) {
      return "added";
    }
    if (Intent.ACTION_PACKAGE_REPLACED.equals(action)) {
      return "replaced";
    }
    if (Intent.ACTION_PACKAGE_REMOVED.equals(action)) {
      return "removed";
    }
    if (Intent.ACTION_PACKAGE_FULLY_REMOVED.equals(action)) {
      return "fully_removed";
    }
    return "";
  }

  private static boolean isSafePackageName(String packageName) {
    if (packageName == null || packageName.length() == 0 || packageName.length() > 255) {
      return false;
    }
    for (int i = 0; i < packageName.length(); i++) {
      char c = packageName.charAt(i);
      boolean ok =
          (c >= 'a' && c <= 'z')
              || (c >= 'A' && c <= 'Z')
              || (c >= '0' && c <= '9')
              || c == '_'
              || c == '.'
              || c == '-';
      if (!ok) {
        return false;
      }
    }
    return true;
  }

  private static String buildLine(
      String action, int userId, int uid, boolean replacing, String packageName) {
    return EVENT_PREFIX
        + "|"
        + action.toLowerCase(Locale.ROOT)
        + "|"
        + userId
        + "|"
        + uid
        + "|"
        + (replacing ? "1" : "0")
        + "|"
        + packageName
        + "|"
        + System.currentTimeMillis();
  }

  private static void appendEvent(String line) {
    Log.i(TAG, line);
    File target = eventFile;
    if (target == null) {
      return;
    }
    try {
      File parent = target.getParentFile();
      if (parent != null) {
        parent.mkdirs();
      }
      FileWriter writer = new FileWriter(target, true);
      try {
        writer.write(line);
        writer.write('\n');
      } finally {
        writer.close();
      }
    } catch (Throwable t) {
      Log.w(TAG, "append event failed", t);
    }
  }

  private static void writeReadySignal() {
    File target = readyFile;
    if (target == null) {
      return;
    }
    try {
      File parent = target.getParentFile();
      if (parent != null) {
        parent.mkdirs();
      }
      FileWriter writer = new FileWriter(target, false);
      try {
        writer.write("srx-package-receiver-ready-v1|");
        writer.write(Long.toString(System.currentTimeMillis()));
        writer.write('|');
        writer.write(readBootId());
        writer.write('|');
        writer.write(Integer.toString(Process.myPid()));
        writer.write('|');
        writer.write(sanitizeField(readProcessName()));
        writer.write('\n');
      } finally {
        writer.close();
      }
    } catch (Throwable t) {
      Log.w(TAG, "write ready signal failed", t);
    }
  }

  private static String readBootId() {
    BufferedReader reader = null;
    try {
      reader = new BufferedReader(new FileReader("/proc/sys/kernel/random/boot_id"));
      String value = reader.readLine();
      if (value != null && value.length() > 0) {
        return sanitizeField(value);
      }
    } catch (Throwable ignored) {
    } finally {
      if (reader != null) {
        try {
          reader.close();
        } catch (Throwable ignored) {
        }
      }
    }
    return "unknown";
  }

  private static String readProcessName() {
    BufferedReader reader = null;
    try {
      reader = new BufferedReader(new FileReader("/proc/self/cmdline"));
      String value = reader.readLine();
      if (value != null && value.length() > 0) {
        int nul = value.indexOf(0);
        if (nul >= 0) {
          value = value.substring(0, nul);
        }
        if (value.length() > 0) {
          return value;
        }
      }
    } catch (Throwable ignored) {
    } finally {
      if (reader != null) {
        try {
          reader.close();
        } catch (Throwable ignored) {
        }
      }
    }
    return "unknown";
  }

  private static String sanitizeField(String value) {
    StringBuilder builder = new StringBuilder(value.length());
    for (int i = 0; i < value.length(); i++) {
      char c = value.charAt(i);
      if (c == '|' || Character.isISOControl(c)) {
        builder.append('_');
      } else {
        builder.append(c);
      }
    }
    return builder.toString();
  }
}
