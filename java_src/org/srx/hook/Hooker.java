// SPDX-License-Identifier: Apache-2.0
// SRX Hooker 类：LSPlant Java method hook 的 Java 侧上下文。

package org.srx.hook;

import android.content.ContentValues;
import android.content.res.AssetFileDescriptor;
import android.database.AbstractCursor;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.os.ParcelFileDescriptor;
import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Member;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.lang.reflect.Constructor;
import java.lang.reflect.Field;
import java.io.File;
import java.io.FileNotFoundException;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashMap;
import java.util.HashSet;
import java.util.Locale;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

public class Hooker {
  private static final String HIDDEN_ROW_SENTINEL = "\u001FSRX_HIDDEN_ROW";
  private static final String READ_ONLY_DENIED_SENTINEL_PREFIX =
      "__SRX_READ_ONLY_DENIED__:";
  private static final int ANDROID_USER_ID_OFFSET = 100000;
  private static final int ANDROID_APP_UID_START = 10000;
  private static final String[] PUBLIC_MEDIA_ROOTS = {
      "Alarms", "Audiobooks", "DCIM", "Documents", "Download", "Movies",
      "Music", "Notifications", "Pictures", "Podcasts", "Recordings",
      "Ringtones"
  };
  private static final ArrayList<Hooker> HOOKS = new ArrayList<>();
  private static final HashSet<String> HOOKED_QUERY_CLASSES = new HashSet<>();
  private static final HashSet<String> HOOKED_OPEN_CLASSES = new HashSet<>();
  private static final HashSet<String> HOOKED_MUTATION_CLASSES = new HashSet<>();
  private static final HashSet<String> HOOKED_MUTATION_METHODS = new HashSet<>();
  private static final HashSet<String> HOOKED_FUSE_CLASSES = new HashSet<>();
  private static volatile boolean QUERY_HOOK_PENDING = true;
  private static int QUERY_LOG_COUNT;
  private static int QUERY_NULL_EMPTY_LOG_COUNT;
  private static int OPEN_LOG_COUNT;
  private static int MUTATION_LOG_COUNT;
  private static int CURSOR_LOG_COUNT;
  private static int FILTER_LOG_COUNT;
  private static int INTERNAL_QUERY_LOG_COUNT;
  private static int OPEN_RESULT_LOG_COUNT;
  private static int OPEN_DELEGATE_LOG_COUNT;
  private static int MUTATION_RESULT_LOG_COUNT;
  private static int FUSE_READ_ONLY_LOG_COUNT;
  private static int FUSE_FILE_OPEN_ALLOW_LOG_COUNT;
  private static int FUSE_REDIRECT_LOG_COUNT;
  private static int FUSE_FILE_OPEN_MODE_LOG_COUNT;
  private static int QUERY_KEEP_LOG_COUNT;
  private static int KEPT_ROW_LOG_COUNT;
  private static int BUCKET_PROBE_LOG_COUNT;
  private static int DOWNLOAD_MEDIA_PATCH_LOG_COUNT;
  private static int MEDIA_SOURCE_CAPTURE_LOG_COUNT;
  private static int DIRECT_MEDIA_WRITE_LOG_COUNT;
  private static int BUCKET_ID_REWRITE_LOG_COUNT;
  private static final HashMap<Integer, SourceFdCapture> RECENT_MEDIA_SOURCE_FDS =
      new HashMap<>();
  private static final HashMap<String, PendingDirectMediaWrite> PENDING_DIRECT_MEDIA_WRITES =
      new HashMap<>();
  private static final HashMap<String, String> RECENT_BUCKET_ID_REWRITES =
      new HashMap<>();
  private static final int MAX_RECENT_BUCKET_ID_REWRITES = 512;
  private static final Pattern INLINE_BUCKET_ID_EQUALS =
      Pattern.compile("(\\bbucket_id\\s*=\\s*)(')?(-?\\d+)(')?", Pattern.CASE_INSENSITIVE);
  private static final Pattern INLINE_BUCKET_ID_IN =
      Pattern.compile("(\\bbucket_id\\s+in\\s*\\()([^)]*)(\\))", Pattern.CASE_INSENSITIVE);
  private static final int MAX_QUERY_ACCESS_RECORDS_PER_CURSOR = 128;
  private static final int FUSE_DIR_ACCESS_FOR_READ = 1;
  private static final int FUSE_DIR_ACCESS_FOR_WRITE = 2;
  private static final int FUSE_DIR_ACCESS_FOR_CREATE = 3;
  private static final int FUSE_DIR_ACCESS_FOR_DELETE = 4;
  private static final int FUSE_KIND_OPEN = 1;
  private static final int FUSE_KIND_MKDIR = 2;
  private static final int FUSE_KIND_MKNOD = 3;
  private static final int FUSE_KIND_RENAME = 4;
  private static final int FUSE_KIND_UNLINK = 5;
  private static final int FUSE_KIND_RMDIR = 6;
  private static final ThreadLocal<Integer> PROVIDER_INTERNAL_DEPTH =
      new ThreadLocal<>();
  public Method backup;
  private Member target;

  private native Method doHook(Member target, Method callback);
  private native boolean doUnhook(Member target);

  public Object callback(Object[] args) throws Throwable {
    if (isInsideProviderInternalCall()) {
      logInternalQueryBypass(this);
      return callBackupWithProviderPassthrough(args);
    }
    int callerUid = android.os.Binder.getCallingUid();
    int callerPid = android.os.Binder.getCallingPid();
    captureBinderCaller(callerUid, callerPid);
    enterCallerScope(callerUid, callerPid);
    try {
      if (!isRedirectEnabledForCallerUid(callerUid)) {
        return callBackupPassthrough(args);
      }
      Object[] actualArgs = unwrapArgs(args);
      logQueryArgs(this, actualArgs, callerUid);
      SelectionPatch.apply(args, actualArgs, callerUid);
      ProjectionPatch projectionPatch = ProjectionPatch.apply(args, actualArgs);
      boolean allowMissingMappedTarget = isSingleItemQuery(actualArgs);
      Object result;
      enterProviderInternalCall();
      try {
        result = onMediaProviderQuery(this, args);
      } finally {
        exitProviderInternalCall();
      }
      logQueryInvocationShape(actualArgs, callerUid, result);
      if (result instanceof Cursor) {
        return FilteringCursor.wrap((Cursor)result, callerUid,
                                    projectionPatch.visibleColumns,
                                    allowMissingMappedTarget, actualArgs);
      }
      if (result == null) {
        Cursor empty = emptyQueryCursorIfSafe(actualArgs, projectionPatch,
                                             callerUid);
        if (empty != null)
          return empty;
      }
      logQueryResult(result);
      return result;
    } finally {
      exitCallerScope();
    }
  }

  public Object providerOpenCallback(Object[] args) throws Throwable {
    if (isInsideProviderInternalCall())
      return callBackupWithProviderPassthrough(args);
    int callerUid = android.os.Binder.getCallingUid();
    int callerPid = android.os.Binder.getCallingPid();
    captureBinderCaller(callerUid, callerPid);
    enterCallerScope(callerUid, callerPid);
    try {
      Object[] actualArgs = unwrapArgs(args);
      logOpenArgs(this, actualArgs, callerUid, callerPid);
      if (!isMediaProviderHookerOrReceiver(this, args) &&
          !hasMediaStoreUriArg(actualArgs)) {
        return callBackupPassthrough(args);
      }
      boolean redirectEnabled = isRedirectEnabledForCallerUid(callerUid);
      if (!redirectEnabled) {
        enterProviderInternalCall();
        try {
          Object mappedResult = tryOpenMappedMediaFile(args, callerUid);
          if (mappedResult != null) {
            logOpenResult("owner_mapped", mappedResult);
            return mappedResult;
          }
        } finally {
          exitProviderInternalCall();
        }
        return callBackupPassthrough(args);
      }
      captureMediaSourceFileDescriptor(this, actualArgs, callerUid);
      enterProviderInternalCall();
      try {
        FuseReadOnlyRequest readOnlyRequest =
            parseProviderOpenReadOnlyRequest(this, args, actualArgs,
                                             callerUid);
        if (recordReadOnlyProviderOpenIfNeeded(readOnlyRequest)) {
          logProviderOpenReadOnlyDeny(readOnlyRequest);
          closeQuietly(takeRecentMediaSourceFd(callerUid));
          throw readOnlyOpenException(readOnlyRequest.path);
        }
        Object mappedResult = tryOpenMappedMediaFile(args, callerUid);
        if (mappedResult != null) {
          logOpenResult("mapped", mappedResult);
          return mappedResult;
        }
        recordProviderOpenPath(this, args, actualArgs, callerUid);
        Object result = callBackup(args);
        completeDirectMediaWriteFromSource(this, args, actualArgs, result,
                                           callerUid);
        logOpenResult("backup", result);
        return result;
      } finally {
        exitProviderInternalCall();
      }
    } finally {
      exitCallerScope();
    }
  }

  public Object providerMutationCallback(Object[] args) throws Throwable {
    if (isInsideProviderInternalCall())
      return callBackupWithProviderPassthrough(args);
    int callerUid = android.os.Binder.getCallingUid();
    int callerPid = android.os.Binder.getCallingPid();
    captureBinderCaller(callerUid, callerPid);
    enterCallerScope(callerUid, callerPid);
    try {
      Object[] actualArgs = unwrapArgs(args);
      String mutationMethod = target instanceof Method
                                  ? ((Method)target).getName()
                                  : null;
      if (!isMediaProviderHookerOrReceiver(this, args) &&
          !hasMediaStoreUriArg(actualArgs)) {
        return callBackupPassthrough(args);
      }
      boolean redirectEnabled = isRedirectEnabledForCallerUid(callerUid);
      MutationPatchResult patch =
          redirectEnabled || shouldProbeMediaStoreMutationPatch(callerUid)
              ? patchMediaStoreValues(args, actualArgs, callerUid,
                                      mutationMethod)
              : new MutationPatchResult(false, false);
      logMutationArgs(this, actualArgs, callerUid, callerPid,
                      patch.patchedAny);
      enterProviderInternalCall();
      try {
        Object result = callBackup(args);
        registerDirectMediaWriteAfterInsert(args, result, callerUid,
                                            mutationMethod,
                                            patch.directWriteRequested);
        finishDirectMediaWriteAfterUpdate(actualArgs, result, mutationMethod);
        logMutationResult(this, result);
        return result;
      } finally {
        exitProviderInternalCall();
      }
    } finally {
      exitCallerScope();
    }
  }

  public Object providerFuseCallback(Object[] args) throws Throwable {
    boolean insideProviderInternalCall = isInsideProviderInternalCall();
    Object[] actualArgs = unwrapArgs(args);
    FuseReadOnlyRequest request = parseFuseReadOnlyRequest(this, actualArgs);
    if (request == null) {
      return insideProviderInternalCall ? callBackupWithProviderPassthrough(args)
                                        : callBackup(args);
    }
    captureBinderCaller(request.callerUid, -1);
    enterCallerScope(request.callerUid, -1);
    try {
      if (recordReadOnlyFuseOperation(request.kind, request.opName,
                                      request.opFilter, request.path,
                                      request.fromPath, request.callerUid,
                                      request.flags)) {
        logFuseReadOnlyDeny(request);
        return Integer.valueOf(android.system.OsConstants.EROFS);
      }
      FusePathPatch patch = patchFuseMutationArgs(args, actualArgs, request);
      Object result = callBackup(args);
      logFuseRedirect(patch, result);
      return result;
    } finally {
      exitCallerScope();
    }
  }

  public Object providerFileOpenCallback(Object[] args) throws Throwable {
    boolean insideProviderInternalCall = isInsideProviderInternalCall();
    Object[] actualArgs = unwrapArgs(args);
    FuseFileOpenRequest request = parseFuseFileOpenRequest(this, actualArgs);
    if (request == null) {
      return insideProviderInternalCall ? callBackupWithProviderPassthrough(args)
                                        : callBackup(args);
    }
    captureBinderCaller(request.callerUid, -1);
    enterCallerScope(request.callerUid, -1);
    try {
      FusePathPatch patch = patchFuseFileOpenArgs(args, actualArgs, request);
      Object result;
      try {
        result = callBackup(args);
      } catch (Throwable t) {
        int status = fileOpenThrowableStatus(t);
        if (status == Integer.MIN_VALUE || !shouldAllowFuseFileOpen(request))
          throw t;
        Object allowed = createAllowedFileOpenResult(null, request);
        if (allowed == null)
          throw t;
        logFuseFileOpenAllow(request, status);
        return allowed;
      }
      logFuseRedirect(patch, result);
      int status = fileOpenResultStatus(result);
      if (status == 0 || !shouldAllowFuseFileOpen(request))
        return result;
      Object allowed = createAllowedFileOpenResult(result, request);
      if (allowed == null)
        return result;
      logFuseFileOpenAllow(request, status);
      return allowed;
    } finally {
      exitCallerScope();
    }
  }

  // ContentProvider.attachInfo 的 hook 回调：原方法跑完后再尝试安装 query hook
  public Object attachInfoCallback(Object[] args) throws Throwable {
    Object result = callBackup(args);
    android.content.ContentProvider provider = providerReceiver(args);
    Class<?> clazz = provider != null
                         ? provider.getClass()
                         : args.length >= 1 && args[0] != null ? args[0].getClass()
                                                               : null;
    if (clazz != null) {
      if (QUERY_HOOK_PENDING) {
        tryInstallQueryHook(clazz);
      }
      tryInstallOpenHook(clazz);
      tryInstallMutationHook(clazz);
      tryInstallFuseHook(clazz);
      // 如果当前类不是 MediaProvider，尝试用它的 ClassLoader 加载 MediaProvider
      // Android 16+ MediaProvider 可能重写了 attachInfo 导致基类 hook 无法拦截
      if (QUERY_HOOK_PENDING) {
        tryLoadAndHookMediaProvider(clazz.getClassLoader());
      }
    }
    return result;
  }

  public Object callBackup(Object[] args) throws Throwable {
    if (backup == null)
      return null;
    if (args != null && args.length == 1 && args[0] instanceof Object[])
      args = (Object[])args[0];
    boolean isStatic = isStaticTarget();
    int parameterCount = target instanceof Method
                             ? ((Method)target).getParameterTypes().length
                             : Math.max(0, args.length - 1);
    boolean hasReceiverSlot = args.length > parameterCount;
    Object receiver = !isStatic && hasReceiverSlot ? args[0] : null;
    int firstArg = hasReceiverSlot ? 1 : 0;
    Object[] actualArgs = new Object[args.length > firstArg ? args.length - firstArg : 0];
    if (actualArgs.length > 0) {
      System.arraycopy(args, firstArg, actualArgs, 0, actualArgs.length);
    }
    try {
      return backup.invoke(receiver, actualArgs);
    } catch (InvocationTargetException e) {
      Throwable cause = e.getCause();
      if (cause != null)
        throw cause;
      throw e;
    }
  }

  private Object callBackupPassthrough(Object[] args) throws Throwable {
    enterProviderInternalCall();
    try {
      return callBackupWithProviderPassthrough(args);
    } finally {
      exitProviderInternalCall();
    }
  }

  private Object callBackupWithProviderPassthrough(Object[] args)
      throws Throwable {
    enterProviderPassthrough();
    try {
      return callBackup(args);
    } finally {
      exitProviderPassthrough();
    }
  }

  public boolean unhook() { return target != null && doUnhook(target); }

  private static void enterProviderInternalCall() {
    Integer depth = PROVIDER_INTERNAL_DEPTH.get();
    PROVIDER_INTERNAL_DEPTH.set(depth == null ? 1 : depth.intValue() + 1);
  }

  private static void exitProviderInternalCall() {
    Integer depth = PROVIDER_INTERNAL_DEPTH.get();
    if (depth == null || depth.intValue() <= 1) {
      PROVIDER_INTERNAL_DEPTH.remove();
    } else {
      PROVIDER_INTERNAL_DEPTH.set(depth.intValue() - 1);
    }
  }

  private static boolean isInsideProviderInternalCall() {
    Integer depth = PROVIDER_INTERNAL_DEPTH.get();
    return depth != null && depth.intValue() > 0;
  }

  // 注入入口：直接查找已知 Provider 类并 hook，同时保留 attachInfo 兜底。
  public static boolean installMediaProviderHook() {
    try {
      // 策略1: 直接通过 ClassLoader 查找当前进程已加载的 Provider 类。
      boolean directHooked = tryDirectProviderHook();
      // 策略2: 兜底 - hook attachInfo 等待 Provider 实例触发。
      Class<?> cpClass = Class.forName("android.content.ContentProvider");
      tryInstallContentProviderMutationFallback(cpClass);
      Method attachInfo = findAttachInfo(cpClass);
      if (attachInfo == null)
        return directHooked;
      Method callback =
          Hooker.class.getDeclaredMethod("attachInfoCallback", Object[].class);
      callback.setAccessible(true);
      Hooker hooker = new Hooker();
      Method backup = hooker.doHook(attachInfo, callback);
      if (backup == null)
        return directHooked;
      backup.setAccessible(true);
      hooker.backup = backup;
      hooker.target = attachInfo;
      HOOKS.add(hooker);
      return true;
    } catch (Throwable ignored) {
      return false;
    }
  }

  private static boolean tryDirectProviderHook() {
    String[] candidates = providerHookClassCandidates();
    boolean hookedAny = false;
    for (String name : candidates) {
      try {
        Class<?> clazz = Class.forName(name);
        logInfo("tryDirectProviderHook: found class " + name);
        tryInstallQueryHook(clazz);
        tryInstallOpenHook(clazz);
        tryInstallMutationHook(clazz);
        tryInstallFuseHook(clazz);
        hookedAny = true;
      } catch (ClassNotFoundException ignored) {
        // class not available in this process
      } catch (Throwable t) {
        logWarn("tryDirectProviderHook error for " + name, t);
      }
    }
    return hookedAny || !QUERY_HOOK_PENDING;
  }

  private static synchronized void tryLoadAndHookMediaProvider(ClassLoader loader) {
    if (loader == null)
      return;
    String[] candidates = providerHookClassCandidates();
    for (String name : candidates) {
      try {
        Class<?> clazz = loader.loadClass(name);
        if (clazz != null) {
          tryInstallQueryHook(clazz);
          tryInstallOpenHook(clazz);
          tryInstallMutationHook(clazz);
          tryInstallFuseHook(clazz);
        }
      } catch (ClassNotFoundException ignored) {
      } catch (Throwable ignored) {
      }
    }
  }

  private static String[] providerHookClassCandidates() {
    return new String[] {
        "com.android.providers.media.MediaProvider",
        "com.android.providers.media.module.MediaProvider",
        "com.android.providers.downloads.DownloadProvider",
        "com.android.providers.downloads.DownloadStorageProvider",
        "com.android.externalstorage.ExternalStorageProvider",
        "com.android.documentsui.archives.ArchivesProvider"
    };
  }

  private static Method findAttachInfo(Class<?> cpClass) {
    for (Method m : cpClass.getDeclaredMethods()) {
      if (!"attachInfo".equals(m.getName()))
        continue;
      Class<?>[] params = m.getParameterTypes();
      if (params.length == 2 &&
          "android.content.Context".equals(params[0].getName()) &&
          "android.content.pm.ProviderInfo".equals(params[1].getName())) {
        return m;
      }
    }
    return null;
  }

  private static Object currentActivityThread() {
    try {
      Class<?> clazz = Class.forName("android.app.ActivityThread");
      Method method = clazz.getDeclaredMethod("currentActivityThread");
      method.setAccessible(true);
      return method.invoke(null);
    } catch (Throwable ignored) {
      return null;
    }
  }

  private static Object fieldValue(Object target, String name) {
    if (target == null || name == null)
      return null;
    for (Class<?> c = target.getClass(); c != null; c = c.getSuperclass()) {
      try {
        Field field = c.getDeclaredField(name);
        field.setAccessible(true);
        return field.get(target);
      } catch (NoSuchFieldException ignored) {
      } catch (Throwable ignored) {
        return null;
      }
    }
    return null;
  }

  private static synchronized void tryInstallQueryHook(Class<?> clazz) {
    if (!QUERY_HOOK_PENDING)
      return;
    for (Class<?> c = clazz; c != null; c = c.getSuperclass()) {
      String name = c.getName();
      if (!isMediaProviderClass(name))
        continue;
      if (!HOOKED_QUERY_CLASSES.add(name))
        continue;
      installQueryOn(c);
    }
  }

  private static synchronized void tryInstallOpenHook(Class<?> clazz) {
    for (Class<?> c = clazz; c != null; c = c.getSuperclass()) {
      String name = c.getName();
      if (!isProviderOpenHookClass(name))
        continue;
      if (!HOOKED_OPEN_CLASSES.add(name))
        continue;
      installOpenOn(c);
    }
  }

  private static synchronized void tryInstallMutationHook(Class<?> clazz) {
    for (Class<?> c = clazz; c != null; c = c.getSuperclass()) {
      String name = c.getName();
      if (!isProviderMutationHookClass(name))
        continue;
      if (!HOOKED_MUTATION_CLASSES.add(name))
        continue;
      installMutationOn(c);
    }
  }

  private static synchronized void tryInstallContentProviderMutationFallback(
      Class<?> clazz) {
    if (clazz == null)
      return;
    installMutationOn(clazz);
  }

  private static synchronized void tryInstallFuseHook(Class<?> clazz) {
    for (Class<?> c = clazz; c != null; c = c.getSuperclass()) {
      String name = c.getName();
      if (!isMediaProviderClass(name))
        continue;
      if (!HOOKED_FUSE_CLASSES.add(name))
        continue;
      installFuseOn(c);
    }
  }

  private static boolean isMediaProviderClass(String name) {
    return "com.android.providers.media.MediaProvider".equals(name) ||
        "com.android.providers.media.module.MediaProvider".equals(name);
  }

  private static boolean isProviderOpenHookClass(String name) {
    return isMediaProviderClass(name) || isSafWriteProviderClass(name);
  }

  private static boolean isProviderOpenMethodName(String name) {
    return "openFile".equals(name) || "openAssetFile".equals(name) ||
        "openTypedAssetFile".equals(name);
  }

  private static boolean isProviderMutationHookClass(String name) {
    return isMediaProviderClass(name) || isSafWriteProviderClass(name);
  }

  private static boolean isSafWriteProviderClass(String name) {
    return "com.android.providers.downloads.DownloadProvider".equals(name) ||
        "com.android.providers.downloads.DownloadStorageProvider".equals(name) ||
        "com.android.externalstorage.ExternalStorageProvider".equals(name) ||
        "com.android.documentsui.archives.ArchivesProvider".equals(name);
  }

  private static boolean isMediaProviderHooker(Hooker hooker) {
    if (hooker == null || !(hooker.target instanceof Method))
      return false;
    return isMediaProviderClass(((Method)hooker.target).getDeclaringClass().getName());
  }

  private static boolean isMediaProviderHookerOrReceiver(Hooker hooker,
                                                         Object[] rawArgs) {
    if (isMediaProviderHooker(hooker))
      return true;
    android.content.ContentProvider receiver = providerReceiver(rawArgs);
    if (receiver == null)
      return false;
    for (Class<?> c = receiver.getClass(); c != null; c = c.getSuperclass()) {
      if (isMediaProviderClass(c.getName()))
        return true;
    }
    return false;
  }

  private static boolean hasMediaStoreUriArg(Object[] args) {
    if (args == null)
      return false;
    for (Object arg : args) {
      if (arg instanceof android.net.Uri &&
          String.valueOf(arg).startsWith("content://media/")) {
        return true;
      }
    }
    return false;
  }

  private static boolean shouldProbeMediaStoreMutationPatch(int callerUid) {
    return callerUid >= 0;
  }

  private static android.content.ContentProvider providerReceiver(Object[] rawArgs) {
    Object receiver = null;
    if (rawArgs != null && rawArgs.length > 0) {
      receiver = rawArgs[0];
      if (rawArgs.length == 1 && rawArgs[0] instanceof Object[]) {
        Object[] nested = (Object[])rawArgs[0];
        if (nested.length > 0)
          receiver = nested[0];
      }
    }
    if (receiver instanceof android.content.ContentProvider)
      return (android.content.ContentProvider)receiver;
    return null;
  }

  private static void installQueryOn(Class<?> clazz) {
    try {
      Method callback =
          Hooker.class.getDeclaredMethod("callback", Object[].class);
      callback.setAccessible(true);
      for (Class<?> c = clazz; c != null; c = c.getSuperclass()) {
        for (Method method : c.getDeclaredMethods()) {
          if (!"query".equals(method.getName()))
            continue;
          String sig = describeMethod(method);
          Hooker hooker = new Hooker();
          Method backup = hooker.doHook(method, callback);
          if (backup == null) {
            logWarn("java hook query failed " + sig);
            continue;
          }
          backup.setAccessible(true);
          hooker.backup = backup;
          hooker.target = method;
          HOOKS.add(hooker);
          QUERY_HOOK_PENDING = false;
          logInfo("java hook query ok " + sig);
        }
      }
    } catch (Throwable ignored) {
    }
  }

  private static void installOpenOn(Class<?> clazz) {
    try {
      Method callback = Hooker.class.getDeclaredMethod("providerOpenCallback",
                                                       Object[].class);
      callback.setAccessible(true);
      for (Class<?> c = clazz; c != null; c = c.getSuperclass()) {
        for (Method method : c.getDeclaredMethods()) {
          String name = method.getName();
          if (!isProviderOpenMethodName(name))
            continue;
          String sig = describeMethod(method);
          Hooker hooker = new Hooker();
          Method backup = hooker.doHook(method, callback);
          if (backup == null) {
            logWarn("java hook open failed " + sig);
            continue;
          }
          backup.setAccessible(true);
          hooker.backup = backup;
          hooker.target = method;
          HOOKS.add(hooker);
          logInfo("java hook open ok " + sig);
        }
      }
    } catch (Throwable ignored) {
    }
  }

  private static void installMutationOn(Class<?> clazz) {
    try {
      Method callback = Hooker.class.getDeclaredMethod(
          "providerMutationCallback", Object[].class);
      callback.setAccessible(true);
      for (Class<?> c = clazz; c != null; c = c.getSuperclass()) {
        for (Method method : c.getDeclaredMethods()) {
          String name = method.getName();
          if (!"insert".equals(name) && !"update".equals(name) &&
              !"bulkInsert".equals(name))
            continue;
          if (!hasContentValuesParameter(method))
            continue;
          String sig = describeMethod(method);
          if (!HOOKED_MUTATION_METHODS.add(sig))
            continue;
          Hooker hooker = new Hooker();
          Method backup = hooker.doHook(method, callback);
          if (backup == null) {
            HOOKED_MUTATION_METHODS.remove(sig);
            logWarn("java hook media mutation failed " + sig);
            continue;
          }
          backup.setAccessible(true);
          hooker.backup = backup;
          hooker.target = method;
          HOOKS.add(hooker);
          logInfo("java hook media mutation ok " + sig);
        }
      }
    } catch (Throwable t) {
      logWarn("java hook media mutation installer failed " + clazz, t);
    }
  }

  private static boolean hasContentValuesParameter(Method method) {
    Class<?>[] params = method.getParameterTypes();
    for (Class<?> param : params) {
      if (ContentValues.class.isAssignableFrom(param) ||
          ContentValues[].class.isAssignableFrom(param))
        return true;
    }
    return false;
  }

  private static void installFuseOn(Class<?> clazz) {
    try {
      Method fuseCallback = Hooker.class.getDeclaredMethod("providerFuseCallback",
                                                           Object[].class);
      Method fileOpenCallback =
          Hooker.class.getDeclaredMethod("providerFileOpenCallback",
                                         Object[].class);
      fuseCallback.setAccessible(true);
      fileOpenCallback.setAccessible(true);
      for (Class<?> c = clazz; c != null; c = c.getSuperclass()) {
        for (Method method : c.getDeclaredMethods()) {
          boolean isFileOpen = isFuseFileOpenMethod(method);
          if (!isFileOpen && !isFuseMutationMethod(method))
            continue;
          String sig = describeMethod(method);
          Hooker hooker = new Hooker();
          Method backup = hooker.doHook(method,
                                        isFileOpen ? fileOpenCallback
                                                   : fuseCallback);
          if (backup == null) {
            logWarn("java hook fuse failed " + sig);
            continue;
          }
          backup.setAccessible(true);
          hooker.backup = backup;
          hooker.target = method;
          HOOKS.add(hooker);
          logInfo("java hook fuse ok " + sig);
        }
      }
    } catch (Throwable ignored) {
    }
  }

  private static boolean isFuseMutationMethod(Method method) {
    if (method == null)
      return false;
    Class<?> returnType = method.getReturnType();
    if (returnType != Integer.TYPE && returnType != Integer.class)
      return false;
    String name = method.getName();
    Class<?>[] params = method.getParameterTypes();
    if ("insertFileIfNecessaryForFuse".equals(name)) {
      return params.length == 2 && params[0] == String.class &&
          params[1] == Integer.TYPE;
    }
    if ("deleteFileForFuse".equals(name)) {
      return (params.length == 2 || params.length == 3) &&
          params[0] == String.class && params[1] == Integer.TYPE;
    }
    if ("renameForFuse".equals(name)) {
      return params.length == 3 && params[0] == String.class &&
          params[1] == String.class && params[2] == Integer.TYPE;
    }
    if ("isDirAccessAllowedForFuse".equals(name)) {
      return params.length == 3 && params[0] == String.class &&
          params[1] == Integer.TYPE && params[2] == Integer.TYPE;
    }
    if ("isDirectoryCreationOrDeletionAllowedForFuse".equals(name)) {
      return params.length == 3 && params[0] == String.class &&
          params[1] == Integer.TYPE && params[2] == Boolean.TYPE;
    }
    return false;
  }

  private static boolean isFuseFileOpenMethod(Method method) {
    if (method == null)
      return false;
    String name = method.getName();
    if (!"onFileOpenForFuse".equals(name) && !"onFileOpen".equals(name))
      return false;
    Class<?> returnType = method.getReturnType();
    if (!"com.android.providers.media.FileOpenResult".equals(returnType.getName()))
      return false;
    Class<?>[] params = method.getParameterTypes();
    return params.length >= 6 && params[0] == String.class &&
        params[1] == String.class && params[2] == Integer.TYPE &&
        params[3] == Integer.TYPE;
  }

  private static String describeMethod(Method method) {
    StringBuilder sb = new StringBuilder();
    sb.append(method.getDeclaringClass().getName())
        .append('#')
        .append(method.getName())
        .append('(');
    Class<?>[] params = method.getParameterTypes();
    for (int i = 0; i < params.length; i++) {
      if (i > 0)
        sb.append(',');
      sb.append(params[i].getName());
    }
    sb.append(')');
    return sb.toString();
  }

  private static native Object onMediaProviderQuery(Hooker hooker,
                                                    Object[] args)
      throws Throwable;
  private static native String filterPath(String path, int callerUid);
  private static native boolean shouldHideCursorPath(String path, int callerUid);
  private static native String resolveOpenPath(String path, int callerUid);
  private static native boolean storagePathExistsBySyscall(String path);
  private static native String rewriteMediaStorePath(String path, int callerUid);
  private static native String resolveDownloadMediaPlaceholderPath(
      String originalPath, String relativePath, String displayName,
      boolean video, int callerUid);
  private static native String rewriteMediaStoreBucketId(String bucketId,
                                                        int callerUid);
  private static native void recordQueryAccessPath(String path, int callerUid);
  private static native void recordProviderOpenPath(String path, int callerUid,
                                                    String callerPackage);
  private static native boolean recordReadOnlyFuseOperation(
      int kind, String opName, String opFilter, String path, String fromPath,
      int callerUid, int flags);
  private static native boolean shouldAllowPublicMappingTargetAccess(
      String path, int callerUid);
  private static native boolean isRedirectEnabledForCallerUid(int callerUid);
  private static native void captureBinderCaller(int callerUid, int callerPid);
  private static native void enterCallerScope(int callerUid, int callerPid);
  private static native void exitCallerScope();
  private static native void enterProviderPassthrough();
  private static native void exitProviderPassthrough();
  private static native boolean isDebugLoggingEnabled();

  private static boolean shouldLog() {
    try {
      return isDebugLoggingEnabled();
    } catch (Throwable ignored) {
      return false;
    }
  }

  private static void logInfo(String message) {
    if (shouldLog())
      android.util.Log.i("SRX", message);
  }

  private static void logWarn(String message) {
    if (shouldLog())
      android.util.Log.w("SRX", message);
  }

  private static void logWarn(String message, Throwable t) {
    if (shouldLog())
      android.util.Log.w("SRX", message, t);
  }

  private static void logDebug(String message) {
    if (shouldLog())
      android.util.Log.d("SRX", message);
  }

  private static FuseReadOnlyRequest parseFuseReadOnlyRequest(
      Hooker hooker, Object[] args) {
    if (hooker == null || !(hooker.target instanceof Method) || args == null)
      return null;
    String methodName = ((Method)hooker.target).getName();
    if ("insertFileIfNecessaryForFuse".equals(methodName)) {
      if (args.length < 2 || !(args[0] instanceof String))
        return null;
      int uid = intArg(args[1], -1);
      return new FuseReadOnlyRequest(FUSE_KIND_MKNOD, methodName, "open:create",
                                     (String)args[0], "", uid, -1);
    }
    if ("deleteFileForFuse".equals(methodName)) {
      if (args.length < 2 || !(args[0] instanceof String))
        return null;
      int uid = intArg(args[1], -1);
      return new FuseReadOnlyRequest(FUSE_KIND_UNLINK, methodName, "delete",
                                     (String)args[0], "", uid, -1);
    }
    if ("renameForFuse".equals(methodName)) {
      if (args.length < 3 || !(args[0] instanceof String) ||
          !(args[1] instanceof String))
        return null;
      int uid = intArg(args[2], -1);
      return new FuseReadOnlyRequest(FUSE_KIND_RENAME, methodName, "rename",
                                     (String)args[1], (String)args[0], uid,
                                     -1);
    }
    if ("isDirAccessAllowedForFuse".equals(methodName)) {
      if (args.length < 3 || !(args[0] instanceof String))
        return null;
      int uid = intArg(args[1], -1);
      int accessType = intArg(args[2], FUSE_DIR_ACCESS_FOR_READ);
      if (accessType == FUSE_DIR_ACCESS_FOR_READ)
        return null;
      if (accessType == FUSE_DIR_ACCESS_FOR_CREATE) {
        return new FuseReadOnlyRequest(FUSE_KIND_MKDIR, methodName, "mkdir",
                                       (String)args[0], "", uid, accessType);
      }
      if (accessType == FUSE_DIR_ACCESS_FOR_DELETE) {
        return new FuseReadOnlyRequest(FUSE_KIND_RMDIR, methodName, "rmdir",
                                       (String)args[0], "", uid, accessType);
      }
      if (accessType == FUSE_DIR_ACCESS_FOR_WRITE) {
        return new FuseReadOnlyRequest(FUSE_KIND_OPEN, methodName,
                                       "open:write", (String)args[0], "",
                                       uid, accessType);
      }
      return new FuseReadOnlyRequest(FUSE_KIND_OPEN, methodName,
                                     "fuse_dir_access", (String)args[0], "",
                                     uid, accessType);
    }
    if ("isDirectoryCreationOrDeletionAllowedForFuse".equals(methodName)) {
      if (args.length < 3 || !(args[0] instanceof String))
        return null;
      int uid = intArg(args[1], -1);
      boolean forCreate = args[2] instanceof Boolean &&
          ((Boolean)args[2]).booleanValue();
      return new FuseReadOnlyRequest(forCreate ? FUSE_KIND_MKDIR
                                               : FUSE_KIND_RMDIR,
                                     methodName, forCreate ? "mkdir" : "rmdir",
                                     (String)args[0], "", uid,
                                     forCreate ? FUSE_DIR_ACCESS_FOR_CREATE
                                               : FUSE_DIR_ACCESS_FOR_DELETE);
    }
    return null;
  }

  private static FusePathPatch patchFuseMutationArgs(Object[] rawArgs,
                                                     Object[] actualArgs,
                                                     FuseReadOnlyRequest request) {
    if (actualArgs == null || request == null ||
        !shouldRewriteFusePath(request))
      return null;
    FusePathPatch patch = null;
    if (request.kind == FUSE_KIND_RENAME) {
      patch = rewriteFusePathArg(rawArgs, actualArgs, request, 0, patch);
      patch = rewriteFusePathArg(rawArgs, actualArgs, request, 1, patch);
      return patch;
    }
    return rewriteFusePathArg(rawArgs, actualArgs, request, 0, null);
  }

  private static FusePathPatch patchFuseFileOpenArgs(Object[] rawArgs,
                                                     Object[] actualArgs,
                                                     FuseFileOpenRequest request) {
    if (actualArgs == null || request == null || !request.forWrite ||
        request.callerUid < ANDROID_APP_UID_START)
      return null;
    FuseReadOnlyRequest rewriteRequest =
        new FuseReadOnlyRequest(FUSE_KIND_OPEN, request.method.getName(),
                                "open:write", request.path, request.ioPath,
                                request.callerUid, -1);
    FusePathPatch patch =
        rewriteFusePathArg(rawArgs, actualArgs, rewriteRequest, 0, null);
    return rewriteFusePathArg(rawArgs, actualArgs, rewriteRequest, 1, patch);
  }

  private static boolean shouldRewriteFusePath(FuseReadOnlyRequest request) {
    if (request.callerUid < ANDROID_APP_UID_START)
      return false;
    if (request.kind == FUSE_KIND_MKDIR || request.kind == FUSE_KIND_MKNOD ||
        request.kind == FUSE_KIND_RENAME || request.kind == FUSE_KIND_UNLINK ||
        request.kind == FUSE_KIND_RMDIR)
      return true;
    return request.kind == FUSE_KIND_OPEN &&
        request.opFilter != null &&
        (request.opFilter.indexOf("write") >= 0 ||
         request.opFilter.indexOf("create") >= 0);
  }

  private static FusePathPatch rewriteFusePathArg(Object[] rawArgs,
                                                  Object[] actualArgs,
                                                  FuseReadOnlyRequest request,
                                                  int index,
                                                  FusePathPatch previous) {
    if (actualArgs == null || index < 0 || index >= actualArgs.length ||
        !(actualArgs[index] instanceof String))
      return previous;
    String original = (String)actualArgs[index];
    String mapped = resolveOpenPathSafe(original, request.callerUid);
    if (mapped == null || mapped.length() == 0 || mapped.equals(original))
      return previous;
    replaceActualArg(rawArgs, actualArgs, index, mapped);
    return new FusePathPatch(request.opName, request.kind, request.callerUid,
                             index, original, mapped);
  }

  private static String resolveOpenPathSafe(String path, int callerUid) {
    if (path == null || path.length() == 0)
      return null;
    try {
      return resolveOpenPath(path, callerUid);
    } catch (Throwable ignored) {
      return null;
    }
  }

  private static FuseFileOpenRequest parseFuseFileOpenRequest(
      Hooker hooker, Object[] args) {
    if (hooker == null || !(hooker.target instanceof Method) || args == null)
      return null;
    Method method = (Method)hooker.target;
    if (!isFuseFileOpenMethod(method))
      return null;
    if (args.length < 6 || !(args[0] instanceof String) ||
        !(args[1] instanceof String))
      return null;
    int uid = intArg(args[2], -1);
    boolean forWrite = isFuseFileOpenForWrite(args);
    return new FuseFileOpenRequest(method, (String)args[0], (String)args[1],
                                   uid, forWrite);
  }

  private static boolean isFuseFileOpenForWrite(Object[] args) {
    if (args == null)
      return false;
    if (args.length > 4 && args[4] instanceof Integer) {
      int mode = ((Integer)args[4]).intValue();
      boolean write = fileOpenModeHasWriteIntent(mode);
      logFuseFileOpenMode(mode, write);
      return write;
    }
    for (int i = 4; i < args.length; i++) {
      if (args[i] instanceof Boolean && ((Boolean)args[i]).booleanValue())
        return true;
    }
    return false;
  }

  private static boolean fileOpenModeHasWriteIntent(int mode) {
    int access = mode & android.system.OsConstants.O_ACCMODE;
    if (access == android.system.OsConstants.O_WRONLY ||
        access == android.system.OsConstants.O_RDWR)
      return true;
    if ((mode & (android.system.OsConstants.O_CREAT |
                 android.system.OsConstants.O_TRUNC |
                 android.system.OsConstants.O_APPEND)) != 0)
      return true;
    return (mode & 0x20000000) != 0 || // ParcelFileDescriptor write bit.
        (mode & (0x08000000 | 0x04000000 | 0x02000000)) != 0;
  }

  private static int intArg(Object value, int fallback) {
    return value instanceof Integer ? ((Integer)value).intValue() : fallback;
  }

  private static boolean booleanArg(Object value, boolean fallback) {
    return value instanceof Boolean ? ((Boolean)value).booleanValue() : fallback;
  }

  private static boolean shouldAllowFuseFileOpen(FuseFileOpenRequest request) {
    if (request == null || request.forWrite ||
        request.callerUid < ANDROID_APP_UID_START)
      return false;
    return shouldAllowPublicMappingTargetAccessSafe(request.path,
                                                    request.callerUid) ||
        shouldAllowPublicMappingTargetAccessSafe(request.ioPath,
                                                request.callerUid);
  }

  private static boolean shouldAllowPublicMappingTargetAccessSafe(
      String path, int callerUid) {
    if (path == null || path.length() == 0)
      return false;
    try {
      return shouldAllowPublicMappingTargetAccess(path, callerUid);
    } catch (Throwable ignored) {
      return false;
    }
  }

  private static int fileOpenResultStatus(Object result) {
    if (result == null)
      return Integer.MIN_VALUE;
    try {
      Field field;
      try {
        field = result.getClass().getField("status");
      } catch (NoSuchFieldException ignored) {
        field = result.getClass().getDeclaredField("status");
        field.setAccessible(true);
      }
      return field.getInt(result);
    } catch (Throwable ignored) {
      return 0;
    }
  }

  private static int fileOpenThrowableStatus(Throwable t) {
    if (t instanceof FileNotFoundException) {
      String message = t.getMessage();
      if (message != null &&
          (message.indexOf("EACCES") >= 0 ||
           message.indexOf("Permission denied") >= 0)) {
        return -android.system.OsConstants.EACCES;
      }
    }
    return Integer.MIN_VALUE;
  }

  private static Object createAllowedFileOpenResult(Object original,
                                                    FuseFileOpenRequest request) {
    try {
      Class<?> resultClass = original != null
                                 ? original.getClass()
                                 : request.method.getReturnType();
      Constructor<?> ctor =
          resultClass.getDeclaredConstructor(Integer.TYPE, Integer.TYPE,
                                             Integer.TYPE, long[].class);
      ctor.setAccessible(true);
      return ctor.newInstance(Integer.valueOf(0),
                              Integer.valueOf(request.callerUid),
                              Integer.valueOf(0), new long[0]);
    } catch (Throwable t) {
      logWarn("fuse file open allow result failed path=" +
                  (request == null ? "" : request.path),
              t);
      return null;
    }
  }

  private static void logFuseReadOnlyDeny(FuseReadOnlyRequest request) {
    logReadOnlyDeny("fuse", request);
  }

  private static void logProviderOpenReadOnlyDeny(FuseReadOnlyRequest request) {
    logReadOnlyDeny("provider_open", request);
  }

  private static void logFuseFileOpenAllow(FuseFileOpenRequest request,
                                           int originalStatus) {
    if (request == null)
      return;
    if (!shouldLog())
      return;
    if (FUSE_FILE_OPEN_ALLOW_LOG_COUNT >= 96)
      return;
    FUSE_FILE_OPEN_ALLOW_LOG_COUNT++;
    try {
      android.util.Log.i("SRX",
                         "java fuse file open allow uid=" +
                             request.callerUid + " status=" +
                             originalStatus + " path=" + request.path +
                             " io=" + request.ioPath + " n=" +
                             FUSE_FILE_OPEN_ALLOW_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static void logFuseFileOpenMode(int mode, boolean write) {
    if (!shouldLog())
      return;
    if (FUSE_FILE_OPEN_MODE_LOG_COUNT >= 64)
      return;
    FUSE_FILE_OPEN_MODE_LOG_COUNT++;
    try {
      android.util.Log.i("SRX", "java fuse file open mode=0x" +
                                    Integer.toHexString(mode) +
                                    " write=" + write +
                                    " n=" + FUSE_FILE_OPEN_MODE_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static void logFuseRedirect(FusePathPatch patch, Object result) {
    if (patch == null || !shouldLog())
      return;
    if (FUSE_REDIRECT_LOG_COUNT >= 96)
      return;
    FUSE_REDIRECT_LOG_COUNT++;
    try {
      android.util.Log.i("SRX", "java fuse redirect op=" + patch.opName +
                                    " kind=" + patch.kind +
                                    " uid=" + patch.callerUid +
                                    " arg=" + patch.argIndex +
                                    " from=" + patch.fromPath +
                                    " to=" + patch.toPath +
                                    " result=" + String.valueOf(result) +
                                    " n=" + FUSE_REDIRECT_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static void logReadOnlyDeny(String source,
                                      FuseReadOnlyRequest request) {
    if (request == null)
      return;
    if (!shouldLog())
      return;
    if (FUSE_READ_ONLY_LOG_COUNT >= 96)
      return;
    FUSE_READ_ONLY_LOG_COUNT++;
    try {
      android.util.Log.i("SRX", "java " + source + " readonly deny op=" +
                                    request.opName + " filter=" +
                                    request.opFilter + " uid=" +
                                    request.callerUid + " path=" +
                                    request.path + " from=" +
                                    request.fromPath + " n=" +
                                    FUSE_READ_ONLY_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static FuseReadOnlyRequest parseProviderOpenReadOnlyRequest(
      Hooker hooker, Object[] rawArgs, Object[] actualArgs, int callerUid) {
    try {
      if (hooker == null || !(hooker.target instanceof Method))
        return null;
      Method method = (Method)hooker.target;
      if (!isProviderOpenMethodName(method.getName()))
        return null;
      OpenRequest request = parseOpenRequest(method.getName(), actualArgs);
      if (request == null || !isWriteMode(request.mode))
        return null;
      String uriText = String.valueOf(request.uri);
      if (!uriText.startsWith("content://media/"))
        return null;

      String path = null;
      PendingDirectMediaWrite pending =
          peekPendingDirectMediaWrite(request.uri);
      if (pending != null && pending.callerUid == callerUid)
        path = pending.path;
      if (path == null || path.length() == 0) {
        android.content.ContentProvider receiver = providerReceiver(rawArgs);
        if (receiver != null)
          path = queryDataPath(receiver, request.uri);
      }
      return buildProviderOpenReadOnlyRequest(method.getName(), request, path,
                                              callerUid);
    } catch (Throwable ignored) {
      return null;
    }
  }

  private static FuseReadOnlyRequest buildProviderOpenReadOnlyRequest(
      String methodName, OpenRequest request, String path, int callerUid) {
    if (request == null || path == null || path.length() == 0)
      return null;
    String mappedPath = null;
    try {
      mappedPath = resolveOpenPath(path, callerUid);
    } catch (Throwable ignored) {
    }
    if (mappedPath == null || mappedPath.length() == 0 ||
        mappedPath.equals(path)) {
      mappedPath = "";
    }
    return new FuseReadOnlyRequest(FUSE_KIND_OPEN, methodName,
                                   openOperationFilterForMode(request.mode),
                                   path, mappedPath, callerUid,
                                   openFlagsForMode(request.mode));
  }

  private static boolean recordReadOnlyProviderOpenIfNeeded(
      FuseReadOnlyRequest request) {
    return request != null &&
        recordReadOnlyFuseOperation(request.kind, request.opName,
                                    request.opFilter, request.path,
                                    request.fromPath, request.callerUid,
                                    request.flags);
  }

  private static String openOperationFilterForMode(String mode) {
    return hasCreateIntentMode(mode) ? "open:create" : "open:write";
  }

  private static boolean hasCreateIntentMode(String mode) {
    if (mode == null)
      return false;
    String lower = mode.toLowerCase(Locale.ROOT);
    return lower.indexOf('w') >= 0 || lower.indexOf('a') >= 0 ||
        lower.indexOf('t') >= 0;
  }

  private static int openFlagsForMode(String mode) {
    if (mode == null)
      return -1;
    String lower = mode.toLowerCase(Locale.ROOT);
    int flags = 0;
    boolean readable = lower.indexOf('r') >= 0;
    boolean writable = lower.indexOf('w') >= 0 || lower.indexOf('a') >= 0 ||
        lower.indexOf('t') >= 0;
    if (writable) {
      flags |= readable ? android.system.OsConstants.O_RDWR
                        : android.system.OsConstants.O_WRONLY;
      flags |= android.system.OsConstants.O_CREAT;
      if (lower.indexOf('a') >= 0)
        flags |= android.system.OsConstants.O_APPEND;
      if (lower.indexOf('t') >= 0 || "w".equals(lower))
        flags |= android.system.OsConstants.O_TRUNC;
      return flags;
    }
    if (readable)
      return android.system.OsConstants.O_RDONLY;
    return -1;
  }

  private static FileNotFoundException readOnlyOpenException(String path) {
    return new FileNotFoundException(
        (path == null || path.length() == 0 ? "media file" : path) +
        ": Read-only file system");
  }

  private static final class FuseReadOnlyRequest {
    final int kind;
    final String opName;
    final String opFilter;
    final String path;
    final String fromPath;
    final int callerUid;
    final int flags;

    FuseReadOnlyRequest(int kind, String opName, String opFilter, String path,
                        String fromPath, int callerUid, int flags) {
      this.kind = kind;
      this.opName = opName == null ? "" : opName;
      this.opFilter = opFilter == null ? "" : opFilter;
      this.path = path == null ? "" : path;
      this.fromPath = fromPath == null ? "" : fromPath;
      this.callerUid = callerUid;
      this.flags = flags;
    }
  }

  private static final class FuseFileOpenRequest {
    final Method method;
    final String path;
    final String ioPath;
    final int callerUid;
    final boolean forWrite;

    FuseFileOpenRequest(Method method, String path, String ioPath,
                        int callerUid, boolean forWrite) {
      this.method = method;
      this.path = path == null ? "" : path;
      this.ioPath = ioPath == null ? "" : ioPath;
      this.callerUid = callerUid;
      this.forWrite = forWrite;
    }
  }

  private static final class FusePathPatch {
    final String opName;
    final int kind;
    final int callerUid;
    final int argIndex;
    final String fromPath;
    final String toPath;

    FusePathPatch(String opName, int kind, int callerUid, int argIndex,
                  String fromPath, String toPath) {
      this.opName = opName == null ? "" : opName;
      this.kind = kind;
      this.callerUid = callerUid;
      this.argIndex = argIndex;
      this.fromPath = fromPath == null ? "" : fromPath;
      this.toPath = toPath == null ? "" : toPath;
    }
  }

  private Object tryOpenMappedMediaFile(Object[] rawArgs, int callerUid)
      throws Throwable {
    String path = null;
    String mappedPath = null;
    OpenRequest request = null;
    Method method = null;
    android.content.ContentProvider receiver = null;
    try {
      if (!(target instanceof Method))
        return null;
      method = (Method)target;
      if (!isProviderOpenMethodName(method.getName()))
        return null;
      receiver = providerReceiver(rawArgs);
      Object[] args = unwrapArgs(rawArgs);
      request = parseOpenRequest(method.getName(), args);
      if (request == null)
        return null;
      if (!String.valueOf(request.uri).startsWith("content://media/"))
        return null;
      path = queryDataPath(receiver, request.uri);
      if (path == null || path.length() == 0) {
        logOpenDelegate("mapped_query_miss", String.valueOf(request.uri), "");
        return null;
      }
      mappedPath = path == null ? null : resolveOpenPath(path, callerUid);
      if (mappedPath == null || mappedPath.length() == 0 || mappedPath.equals(path)) {
        logOpenDelegate("mapped_resolve_miss", path,
                        mappedPath == null ? "" : mappedPath);
        return null;
      }
      rememberProviderOpenPath(mappedPath, callerUid);
      if (!isWriteMode(request.mode) && shouldDenyMissingMappedRead(mappedPath)) {
        logOpenDelegate("mapped_read_missing_denied", path, mappedPath);
        throw new FileNotFoundException(mappedPath);
      }
    } catch (FileNotFoundException denied) {
      throw denied;
    } catch (Throwable ignored) {
      return null;
    }

    try {
      File file = new File(mappedPath);
      File parent = file.getParentFile();
      if (parent != null)
        parent.mkdirs();
      ParcelFileDescriptor pfd =
          ParcelFileDescriptor.open(file, ParcelFileDescriptor.parseMode(request.mode));
      return wrapOpenResult(method, pfd);
    } catch (Throwable t) {
      logWarn("mapped open failed from=" + path + " to=" + mappedPath, t);
      if (request != null && isWriteMode(request.mode)) {
        // ParcelFileDescriptor.open may fail with EXDEV on bind mounts.
        // Fallback: create file directly via FileOutputStream (pure open, no rename).
        try {
          File file = new File(mappedPath);
          File parent = file.getParentFile();
          if (parent != null && !parent.exists())
            parent.mkdirs();
          java.io.FileOutputStream fos = new java.io.FileOutputStream(file);
          try {
            ParcelFileDescriptor pfd = ParcelFileDescriptor.dup(fos.getFD());
            return wrapOpenResult(method, pfd);
          } finally {
            fos.close();
          }
        } catch (Throwable t2) {
          logWarn("fallback write also failed from=" + path + " to=" + mappedPath, t2);
          return null;
        }
      }
      if (request != null && !isWriteMode(request.mode))
        throw new FileNotFoundException(mappedPath);
      return null;
    }
  }

  private static OpenRequest parseOpenRequest(String methodName, Object[] args) {
    if (args == null || args.length == 0 ||
        !(args[0] instanceof android.net.Uri))
      return null;
    android.net.Uri uri = (android.net.Uri)args[0];
    if ("openTypedAssetFile".equals(methodName))
      return new OpenRequest(uri, "r");
    if (args.length < 2 || !(args[1] instanceof String))
      return null;
    return new OpenRequest(uri, (String)args[1]);
  }

  private static Object wrapOpenResult(Method method, ParcelFileDescriptor pfd) {
    if (AssetFileDescriptor.class.isAssignableFrom(method.getReturnType())) {
      return new AssetFileDescriptor(pfd, 0, AssetFileDescriptor.UNKNOWN_LENGTH);
    }
    return pfd;
  }

  private static final class OpenRequest {
    final android.net.Uri uri;
    final String mode;

    OpenRequest(android.net.Uri uri, String mode) {
      this.uri = uri;
      this.mode = mode == null || mode.length() == 0 ? "r" : mode;
    }
  }

  private static boolean isWriteMode(String mode) {
    return mode != null &&
        (mode.indexOf('w') >= 0 || mode.indexOf('a') >= 0 || mode.indexOf('t') >= 0);
  }

  private static boolean shouldDenyMissingMappedRead(String path) {
    return !pathExistsBySyscall(path);
  }

  private static boolean pathExistsBySyscall(String path) {
    if (path == null || path.length() == 0)
      return false;
    try {
      return storagePathExistsBySyscall(path);
    } catch (Throwable ignored) {
      return false;
    }
  }

  private static boolean isSrxSandboxFallbackPath(String path, int callerUid) {
    if (path == null || path.length() == 0)
      return false;
    int userId = userIdFromUid(callerUid);
    if (userId < 0)
      return false;
    String value = path.startsWith("file://")
                       ? path.substring("file://".length())
                       : path;
    String dataPrefix = "/data/media/" + userId + "/Android/data/";
    String storagePrefix = "/storage/emulated/" + userId + "/Android/data/";
    return isSrxSandboxFallbackPathWithPrefix(value, dataPrefix) ||
        isSrxSandboxFallbackPathWithPrefix(value, storagePrefix);
  }

  private static boolean isSrxSandboxFallbackPathWithPrefix(String path,
                                                            String prefix) {
    if (!path.startsWith(prefix))
      return false;
    int packageEnd = path.indexOf('/', prefix.length());
    if (packageEnd < 0)
      return false;
    String rest = path.substring(packageEnd + 1);
    return "sdcard".equals(rest) || rest.startsWith("sdcard/");
  }

  private static String queryDataPath(android.content.ContentProvider provider,
                                      android.net.Uri uri) {
    Cursor cursor = null;
    try {
      if (provider != null) {
        cursor = provider.query(uri, new String[] {"_data"}, null, null, null);
      } else {
        Object app = activityThreadApplication(currentActivityThread());
        if (app instanceof android.content.Context) {
          cursor = ((android.content.Context)app).getContentResolver()
              .query(uri, new String[] {"_data"}, null, null, null);
        }
      }
      if (cursor == null || !cursor.moveToFirst())
        return null;
      return cursor.getString(0);
    } catch (Throwable ignored) {
      return null;
    } finally {
      if (cursor != null) {
        try {
          cursor.close();
        } catch (Throwable ignored) {
        }
      }
    }
  }

  private static void recordProviderOpenPath(Hooker hooker, Object[] rawArgs,
                                             Object[] actualArgs,
                                             int callerUid) {
    try {
      if (!(hooker != null && hooker.target instanceof Method))
        return;
      Method method = (Method)hooker.target;
      if (!isProviderOpenMethodName(method.getName()))
        return;
      OpenRequest request = parseOpenRequest(method.getName(), actualArgs);
      if (request == null)
        return;
      String uriText = String.valueOf(request.uri);
      if (!uriText.startsWith("content://media/"))
        return;
      android.content.ContentProvider receiver = providerReceiver(rawArgs);
      if (receiver == null)
        return;
      String path = queryDataPath(receiver, request.uri);
      rememberProviderOpenPath(path, callerUid);
      String mappedPath = null;
      try {
        mappedPath = path == null ? null : resolveOpenPath(path, callerUid);
      } catch (Throwable ignored) {
      }
      if (mappedPath != null && mappedPath.length() > 0 &&
          !mappedPath.equals(path)) {
        rememberProviderOpenPath(mappedPath, callerUid);
      }
    } catch (Throwable ignored) {
    }
  }

  private static void rememberProviderOpenPath(String path, int callerUid) {
    rememberProviderOpenPath(path, callerUid, packageNameForUid(callerUid));
  }

  private static void rememberProviderOpenPath(String path, int callerUid,
                                               String callerPackage) {
    if (path == null || path.length() == 0 ||
        callerUid < ANDROID_APP_UID_START)
      return;
    try {
      recordProviderOpenPath(path, callerUid, callerPackage);
    } catch (Throwable ignored) {
    }
  }

  private static String packageNameForUid(int uid) {
    if (uid < ANDROID_APP_UID_START)
      return "";
    try {
      Object thread = currentActivityThread();
      if (thread == null)
        return "";
      Object app = activityThreadApplication(thread);
      if (app == null)
        return "";
      if (!(app instanceof android.content.Context))
        return "";
      android.content.pm.PackageManager pm =
          ((android.content.Context)app).getPackageManager();
      if (pm == null)
        return "";
      String[] packages = pm.getPackagesForUid(uid);
      if (packages != null) {
        for (String candidate : packages) {
          if (candidate != null && candidate.length() > 0 &&
              !isIntermediateAttributionPackage(candidate)) {
            return candidate.trim();
          }
        }
        if (packages.length > 0 && packages[0] != null) {
          return packages[0].trim();
        }
      }
      return normalizePackageManagerUidName(pm.getNameForUid(uid));
    } catch (Throwable ignored) {
      return "";
    }
  }

  private static Object activityThreadApplication(Object activityThread) {
    if (activityThread == null)
      return null;
    try {
      Method method = activityThread.getClass().getDeclaredMethod("getApplication");
      method.setAccessible(true);
      return method.invoke(activityThread);
    } catch (Throwable ignored) {
      return fieldValue(activityThread, "mInitialApplication");
    }
  }

  private static String normalizePackageManagerUidName(String name) {
    if (name == null || name.length() == 0)
      return "";
    int colon = name.indexOf(':');
    if (colon >= 0)
      name = name.substring(0, colon);
    int comma = name.indexOf(',');
    if (comma >= 0)
      name = name.substring(0, comma);
    return name.trim();
  }

  private static boolean isIntermediateAttributionPackage(String value) {
    if (value == null || value.length() == 0)
      return true;
    String name = value.toLowerCase(Locale.ROOT);
    return name.equals("com.android.providers.media") ||
        name.equals("com.android.providers.media.module") ||
        name.equals("com.google.android.providers.media.module") ||
        name.equals("com.android.providers.downloads") ||
        name.equals("com.android.providers.downloads.ui") ||
        name.equals("com.android.externalstorage") ||
        name.contains(".documentsui") ||
        name.contains(".photopicker") ||
        name.contains(".filemanager") ||
        name.contains("fileexplorer") ||
        name.endsWith(".myfiles");
  }

  private static MutationPatchResult patchMediaStoreValues(Object[] rawArgs,
                                                          Object[] actualArgs,
                                                          int callerUid,
                                                          String mutationMethod)
      throws FileNotFoundException {
    if (actualArgs == null || actualArgs.length == 0)
      return new MutationPatchResult(false, false);
    boolean patchedAny = false;
    boolean directWriteRequested = false;
    int uriIndex = findMutationUriIndex(actualArgs);
    android.net.Uri mutationUri =
        uriIndex >= 0 ? (android.net.Uri)actualArgs[uriIndex] : null;
    String requestedCollection = null;
    boolean insertLike = isInsertLikeMutation(mutationMethod);
    for (int i = 0; i < actualArgs.length; i++) {
      Object arg = actualArgs[i];
      if (arg instanceof ContentValues) {
        ContentValuesPatch patch =
            patchContentValues((ContentValues)arg, callerUid, mutationUri,
                               insertLike);
        requestedCollection =
            mergeRequestedMediaCollection(requestedCollection,
                                          patch.mediaCollection);
        directWriteRequested |= patch.directWriteRequested;
        if (patch.values != arg) {
          replaceActualArg(rawArgs, actualArgs, i, patch.values);
          patchedAny = true;
        }
      } else if (arg instanceof ContentValues[]) {
        ContentValues[] values = (ContentValues[])arg;
        ContentValues[] patchedValues = null;
        for (int j = 0; j < values.length; j++) {
          ContentValuesPatch patch =
              patchContentValues(values[j], callerUid, mutationUri,
                                 insertLike);
          requestedCollection =
              mergeRequestedMediaCollection(requestedCollection,
                                            patch.mediaCollection);
          directWriteRequested |= patch.directWriteRequested;
          if (patch.values != values[j]) {
            if (patchedValues == null)
              patchedValues = values.clone();
            patchedValues[j] = patch.values;
          }
        }
        if (patchedValues != null) {
          replaceActualArg(rawArgs, actualArgs, i, patchedValues);
          patchedAny = true;
        }
      }
    }
    if (insertLike && uriIndex >= 0 && requestedCollection != null &&
        requestedCollection.length() > 0) {
      android.net.Uri patchedUri =
          rewriteMutationCollectionUri(mutationUri, requestedCollection);
      if (patchedUri != null && !patchedUri.equals(mutationUri)) {
        replaceActualArg(rawArgs, actualArgs, uriIndex, patchedUri);
        patchedAny = true;
      }
    }
    return new MutationPatchResult(patchedAny, directWriteRequested);
  }

  private static ContentValuesPatch patchContentValues(ContentValues values,
                                                       int callerUid,
                                                       android.net.Uri mutationUri,
                                                       boolean insertLike)
      throws FileNotFoundException {
    if (values == null || values.size() == 0)
      return new ContentValuesPatch(values, null, false);

    ContentValues patched = null;
    String requestedCollection = null;
    boolean directWriteRequested = false;
    String dataKey = values.containsKey("_data") ? "_data"
                                                 : values.containsKey("data") ? "data" : null;
    if (dataKey != null) {
      String originalPath = values.getAsString(dataKey);
      DownloadMediaPathPatch mediaPatch = insertLike
                                              ? rewriteDownloadMediaPlaceholderPath(
                                                    originalPath, values,
                                                    callerUid, mutationUri)
                                              : null;
      if (mediaPatch != null) {
        patched = copyIfNeeded(patched, values);
        patched.put(dataKey, mediaPatch.mappedPath);
        patched.put("relative_path", mediaPatch.relativePath);
        patchDirectoryColumns(patched, mediaPatch.relativePath);
        if (mediaPatch.mimeType != null)
          patched.put("mime_type", mediaPatch.mimeType);
        forceDirectMediaStoreWrite(patched);
        requestedCollection = mediaPatch.mediaCollection;
        directWriteRequested = true;
      } else {
        String mappedPath = rewriteStoragePathForValues(originalPath, callerUid);
        if (mappedPath != null && !mappedPath.equals(originalPath)) {
          patched = copyIfNeeded(patched, values);
          patched.put(dataKey, mappedPath);
        }
      }
    }

    ContentValues relativeSource = patched != null ? patched : values;
    String relativePath = relativeSource.getAsString("relative_path");
    if (relativePath == null || relativePath.length() == 0)
      relativePath = relativePathFromDirectoryColumns(relativeSource);
    if (insertLike && relativePath != null && relativePath.length() > 0) {
      String displayName =
          firstString(relativeSource, "_display_name", "display_name");
      DownloadMediaPathPatch mediaPatch =
          rewriteDownloadMediaPlaceholderRelativePath(relativePath, displayName,
                                                      values, callerUid,
                                                      mutationUri);
      if (mediaPatch != null) {
        patched = copyIfNeeded(patched, values);
        patched.put("relative_path", mediaPatch.relativePath);
        patchDirectoryColumns(patched, mediaPatch.relativePath);
        if (mediaPatch.mimeType != null)
          patched.put("mime_type", mediaPatch.mimeType);
        forceDirectMediaStoreWrite(patched);
        requestedCollection = mediaPatch.mediaCollection;
        directWriteRequested = true;
        relativeSource = patched;
        relativePath = relativeSource.getAsString("relative_path");
      }
    }
    if (relativePath != null && relativePath.length() > 0) {
      String displayName =
          firstString(relativeSource, "_display_name", "display_name");
      String probePath = buildMediaStoreProbePath(relativePath, displayName, callerUid);
      String mappedPath = rewriteStoragePathForValues(probePath, callerUid);
      String mappedRelative = relativePathFromStoragePath(mappedPath, callerUid);
      if (mappedRelative != null &&
          !normalizeRelativePathValue(mappedRelative).equals(
              normalizeRelativePathValue(relativePath))) {
        patched = copyIfNeeded(patched, values);
        patched.put("relative_path", mappedRelative);
        patchDirectoryColumns(patched, mappedRelative);
      }
    }
    if (!insertLike && mutationUri != null) {
      ContentValues sizePatched =
          patchPendingDirectMediaUpdate(patched, values, mutationUri);
      if (sizePatched != null)
        patched = sizePatched;
    }

    return new ContentValuesPatch(patched == null ? values : patched,
                                  requestedCollection,
                                  directWriteRequested);
  }

  private static int findMutationUriIndex(Object[] actualArgs) {
    if (actualArgs == null)
      return -1;
    for (int i = 0; i < actualArgs.length; i++) {
      if (actualArgs[i] instanceof android.net.Uri)
        return i;
    }
    return -1;
  }

  private static boolean isInsertLikeMutation(String methodName) {
    return "insert".equals(methodName);
  }

  private static String mergeRequestedMediaCollection(String current,
                                                      String next) {
    if (next == null || next.length() == 0)
      return current;
    if (current == null || current.length() == 0 || current.equals(next))
      return next;
    return null;
  }

  private static android.net.Uri rewriteMutationCollectionUri(android.net.Uri uri,
                                                              String mediaCollection) {
    if (uri == null || mediaCollection == null || mediaCollection.length() == 0)
      return null;
    try {
      String value = uri.toString();
      if (!isDownloadsCollectionUri(uri))
        return null;
      String replacement = "/" + mediaCollectionUriPath(mediaCollection);
      int query = value.indexOf('?');
      String base = query >= 0 ? value.substring(0, query) : value;
      String suffix = query >= 0 ? value.substring(query) : "";
      if (base.endsWith(replacement))
        return null;
      if (base.endsWith("/downloads")) {
        return android.net.Uri.parse(
            base.substring(0, base.length() - "/downloads".length()) +
            replacement + suffix);
      }
    } catch (Throwable ignored) {
    }
    return null;
  }

  private static String mediaCollectionUriPath(String mediaCollection) {
    if ("video".equals(mediaCollection))
      return "video/media";
    return "images/media";
  }

  private static void forceDirectMediaStoreWrite(ContentValues values) {
    if (values == null)
      return;
    values.put("is_pending", Integer.valueOf(0));
    values.remove("date_expires");
  }

  private static ContentValues patchPendingDirectMediaUpdate(
      ContentValues current, ContentValues original, android.net.Uri uri) {
    PendingDirectMediaWrite pending = peekPendingDirectMediaWrite(uri);
    if (pending == null || pending.bytes <= 0)
      return null;
    boolean hasSize = original.containsKey("size");
    boolean hasDataSize = original.containsKey("_size");
    if (!hasSize && !hasDataSize)
      return null;
    ContentValues patched = copyIfNeeded(current, original);
    Long size = Long.valueOf(pending.bytes);
    if (hasSize)
      patched.put("size", size);
    if (hasDataSize)
      patched.put("_size", size);
    return patched;
  }

  private static DownloadMediaPathPatch rewriteDownloadMediaPlaceholderPath(
      String originalPath, ContentValues values, int callerUid,
      android.net.Uri mutationUri) throws FileNotFoundException {
    if (!isDownloadsCollectionUri(mutationUri) ||
        !isLikelyDownloadMediaPlaceholder(values))
      return null;
    DownloadPathParts parts = parseDownloadPathParts(originalPath, callerUid);
    if (parts == null || parts.bucket.length() == 0 ||
        parts.fileName.length() == 0)
      return null;
    String collection = preferredMediaCollection(values);
    String mappedPath =
        resolveDownloadMediaPlaceholderPathSafe(originalPath, null,
                                                parts.fileName,
                                                "video".equals(collection),
                                                callerUid);
    throwIfReadOnlyDeniedMediaPlaceholder(mappedPath);
    if (mappedPath == null || mappedPath.length() == 0)
      return null;
    String mappedRelative = relativePathFromStoragePath(mappedPath, callerUid);
    if (mappedRelative == null || mappedRelative.length() == 0)
      return null;
    String mimeType =
        normalizedMediaMimeType(values, "video".equals(collection));
    logDownloadMediaPathPatch(originalPath, mappedPath,
                              mappedRelative, collection, callerUid);
    return new DownloadMediaPathPatch(mappedPath, mappedRelative, mimeType,
                                      collection);
  }

  private static DownloadMediaPathPatch rewriteDownloadMediaPlaceholderRelativePath(
      String relativePath, String displayName, ContentValues values,
      int callerUid, android.net.Uri mutationUri)
      throws FileNotFoundException {
    if (!isDownloadsCollectionUri(mutationUri) ||
        !isLikelyDownloadMediaPlaceholder(values))
      return null;
    DownloadPathParts parts =
        parseDownloadRelativePathParts(relativePath, displayName, callerUid);
    if (parts == null || parts.bucket.length() == 0 ||
        parts.fileName.length() == 0)
      return null;
    String collection = preferredMediaCollection(values);
    String mappedPath =
        resolveDownloadMediaPlaceholderPathSafe(null, relativePath,
                                                parts.fileName,
                                                "video".equals(collection),
                                                callerUid);
    throwIfReadOnlyDeniedMediaPlaceholder(mappedPath);
    if (mappedPath == null || mappedPath.length() == 0)
      return null;
    String mappedRelative = relativePathFromStoragePath(mappedPath, callerUid);
    if (mappedRelative == null || mappedRelative.length() == 0)
      return null;
    String mimeType =
        normalizedMediaMimeType(values, "video".equals(collection));
    logDownloadMediaPathPatch("relative:" + relativePath, mappedPath,
                              mappedRelative, collection,
                              callerUid);
    return new DownloadMediaPathPatch(mappedPath, mappedRelative, mimeType,
                                      collection);
  }

  private static String resolveDownloadMediaPlaceholderPathSafe(
      String originalPath, String relativePath, String displayName,
      boolean video, int callerUid) {
    try {
      return resolveDownloadMediaPlaceholderPath(originalPath, relativePath,
                                                displayName, video,
                                                callerUid);
    } catch (Throwable ignored) {
      return null;
    }
  }

  private static void throwIfReadOnlyDeniedMediaPlaceholder(String path)
      throws FileNotFoundException {
    if (path == null || !path.startsWith(READ_ONLY_DENIED_SENTINEL_PREFIX))
      return;
    String deniedPath = path.substring(READ_ONLY_DENIED_SENTINEL_PREFIX.length());
    throw readOnlyOpenException(deniedPath);
  }

  private static boolean isDownloadsCollectionUri(android.net.Uri uri) {
    if (uri == null)
      return false;
    try {
      String value = uri.toString().toLowerCase(Locale.ROOT);
      int query = value.indexOf('?');
      if (query >= 0)
        value = value.substring(0, query);
      return value.endsWith("/downloads") || value.indexOf("/downloads/") >= 0;
    } catch (Throwable ignored) {
      return false;
    }
  }

  private static boolean isLikelyDownloadMediaPlaceholder(ContentValues values) {
    if (values == null)
      return false;
    String mimeType = values.getAsString("mime_type");
    if (mimeType != null) {
      String lower = mimeType.toLowerCase(Locale.ROOT);
      if (lower.startsWith("image/") || lower.startsWith("video/") ||
          "application/octet-stream".equals(lower))
        return true;
      return false;
    }
    String displayName = firstString(values, "_display_name", "display_name");
    if (displayName == null || displayName.length() == 0)
      return true;
    return !hasFileExtension(displayName);
  }

  private static String preferredMediaCollection(ContentValues values) {
    String mimeType = values == null ? null : values.getAsString("mime_type");
    if (mimeType != null &&
        mimeType.toLowerCase(Locale.ROOT).startsWith("video/"))
      return "video";
    return "images";
  }

  private static String normalizedMediaMimeType(ContentValues values,
                                                boolean video) {
    String mimeType = values == null ? null : values.getAsString("mime_type");
    if (mimeType != null) {
      String lower = mimeType.toLowerCase(Locale.ROOT);
      if (lower.startsWith("image/") || lower.startsWith("video/"))
        return mimeType;
    }
    return video ? "video/mp4" : "image/jpeg";
  }

  private static DownloadPathParts parseDownloadPathParts(String path,
                                                          int callerUid) {
    if (path == null || path.length() == 0)
      return null;
    String value = path.startsWith("file://")
                       ? path.substring("file://".length())
                       : path;
    int userId = userIdFromUid(callerUid);
    if (userId < 0)
      return null;
    String prefix = "/storage/emulated/" + userId + "/";
    String dataPrefix = "/data/media/" + userId + "/";
    if (value.startsWith(dataPrefix)) {
      value = prefix + value.substring(dataPrefix.length());
    }
    if (!value.startsWith(prefix))
      return null;
    String relative = value.substring(prefix.length());
    String downloadPrefix = "Download/";
    if (!relative.startsWith(downloadPrefix))
      return null;
    String rest = relative.substring(downloadPrefix.length());
    int slash = rest.lastIndexOf('/');
    if (slash <= 0 || slash >= rest.length() - 1)
      return null;
    String bucket = normalizeRelativePathValue(rest.substring(0, slash));
    String fileName = rest.substring(slash + 1);
    if (bucket.length() == 0 || hasUnsafeRelativePathSegment(bucket) ||
        fileName.length() == 0 || fileName.indexOf('\\') >= 0)
      return null;
    return new DownloadPathParts(prefix, bucket, fileName);
  }

  private static DownloadPathParts parseDownloadRelativePathParts(
      String relativePath, String displayName, int callerUid) {
    int userId = userIdFromUid(callerUid);
    if (userId < 0)
      return null;
    String relative = normalizeRelativePathValue(relativePath);
    String downloadPrefix = "Download/";
    if (!relative.startsWith(downloadPrefix))
      return null;
    String bucket = normalizeRelativePathValue(
        relative.substring(downloadPrefix.length()));
    if (bucket.length() == 0 || hasUnsafeRelativePathSegment(bucket))
      return null;
    String fileName = displayName;
    if (fileName == null || fileName.length() == 0)
      fileName = ".srx_media";
    if (fileName.indexOf('/') >= 0 || fileName.indexOf('\\') >= 0)
      return null;
    return new DownloadPathParts("/storage/emulated/" + userId + "/", bucket,
                                 fileName);
  }

  private static boolean hasUnsafeRelativePathSegment(String relativePath) {
    if (relativePath == null)
      return true;
    String normalized = normalizeRelativePathValue(relativePath);
    if (normalized.length() == 0)
      return true;
    String[] parts = normalized.split("/");
    for (String part : parts) {
      if (part.length() == 0 || ".".equals(part) || "..".equals(part))
        return true;
    }
    return false;
  }

  private static String normalizeStorageDisplayPath(String path,
                                                    int callerUid) {
    if (path == null || path.length() == 0)
      return null;
    String value = path.startsWith("file://")
                       ? path.substring("file://".length())
                       : path;
    int userId = userIdFromUid(callerUid);
    if (userId < 0)
      return value;
    String dataRoot = "/data/media/" + userId + "/";
    if (value.startsWith(dataRoot)) {
      value = "/storage/emulated/" + userId + "/" +
              value.substring(dataRoot.length());
    }
    while (value.endsWith("/") && value.length() > 1)
      value = value.substring(0, value.length() - 1);
    return value;
  }

  private static boolean hasFileExtension(String name) {
    if (name == null)
      return false;
    int slash = Math.max(name.lastIndexOf('/'), name.lastIndexOf('\\'));
    int dot = name.lastIndexOf('.');
    return dot > slash + 1 && dot < name.length() - 1;
  }

  private static void logDownloadMediaPathPatch(String originalPath,
                                                String mappedPath,
                                                String relativePath,
                                                String collection,
                                                int callerUid) {
    if (!shouldLog())
      return;
    if (DOWNLOAD_MEDIA_PATCH_LOG_COUNT >= 64)
      return;
    DOWNLOAD_MEDIA_PATCH_LOG_COUNT++;
    try {
      android.util.Log.i("SRX",
                         "java media downloads placeholder patch caller_uid=" +
                             callerUid + " from=" + originalPath +
                             " mapped=" + mappedPath +
                             " relative=" + relativePath +
                             " collection=" + collection + " n=" +
                             DOWNLOAD_MEDIA_PATCH_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static final class ContentValuesPatch {
    final ContentValues values;
    final String mediaCollection;
    final boolean directWriteRequested;

    ContentValuesPatch(ContentValues values, String mediaCollection,
                       boolean directWriteRequested) {
      this.values = values;
      this.mediaCollection = mediaCollection;
      this.directWriteRequested = directWriteRequested;
    }
  }

  private static final class MutationPatchResult {
    final boolean patchedAny;
    final boolean directWriteRequested;

    MutationPatchResult(boolean patchedAny, boolean directWriteRequested) {
      this.patchedAny = patchedAny;
      this.directWriteRequested = directWriteRequested;
    }
  }

  private static final class DownloadMediaPathPatch {
    final String mappedPath;
    final String relativePath;
    final String mimeType;
    final String mediaCollection;

    DownloadMediaPathPatch(String mappedPath, String relativePath,
                           String mimeType, String mediaCollection) {
      this.mappedPath = mappedPath;
      this.relativePath = relativePath;
      this.mimeType = mimeType;
      this.mediaCollection = mediaCollection;
    }
  }

  private static final class DownloadPathParts {
    final String prefix;
    final String bucket;
    final String fileName;

    DownloadPathParts(String prefix, String bucket, String fileName) {
      this.prefix = prefix;
      this.bucket = bucket;
      this.fileName = fileName;
    }
  }

  private static void captureMediaSourceFileDescriptor(Hooker hooker,
                                                       Object[] actualArgs,
                                                       int callerUid) {
    try {
      if (!(hooker != null && hooker.target instanceof Method))
        return;
      Method method = (Method)hooker.target;
      if (!"openTypedAssetFile".equals(method.getName()))
        return;
      if (actualArgs == null || actualArgs.length < 3 ||
          !(actualArgs[0] instanceof android.net.Uri))
        return;
      String uri = String.valueOf(actualArgs[0]);
      if (!uri.startsWith("content://media/") || !uri.endsWith("/file"))
        return;
      android.os.Bundle bundle = null;
      for (Object arg : actualArgs) {
        if (arg instanceof android.os.Bundle) {
          bundle = (android.os.Bundle)arg;
          break;
        }
      }
      if (bundle == null || !bundle.containsKey("file_descriptor"))
        return;
      Object fdValue = bundle.getParcelable("file_descriptor");
      if (!(fdValue instanceof ParcelFileDescriptor))
        return;
      ParcelFileDescriptor dup =
          ParcelFileDescriptor.dup(
              ((ParcelFileDescriptor)fdValue).getFileDescriptor());
      long size = statFileDescriptorSize(dup);
      if (size <= 0) {
        closeQuietly(dup);
        return;
      }
      SourceFdCapture capture = new SourceFdCapture(
          dup, sourceFileDescriptorPath(dup), size,
          android.os.SystemClock.elapsedRealtime());
      synchronized (RECENT_MEDIA_SOURCE_FDS) {
        SourceFdCapture old = RECENT_MEDIA_SOURCE_FDS.put(callerUid, capture);
        closeQuietly(old);
      }
      logMediaSourceCapture(callerUid, capture);
    } catch (Throwable ignored) {
    }
  }

  private static void registerDirectMediaWriteAfterInsert(
      Object[] rawArgs, Object result, int callerUid, String mutationMethod,
      boolean directWriteRequested) {
    try {
      if (!directWriteRequested || !"insert".equals(mutationMethod) ||
          !(result instanceof android.net.Uri))
        return;
      Object receiver =
          rawArgs != null && rawArgs.length > 0 ? rawArgs[0] : null;
      String path = null;
      if (receiver instanceof android.content.ContentProvider) {
        path = queryDataPath((android.content.ContentProvider)receiver,
                             (android.net.Uri)result);
      }
      PendingDirectMediaWrite pending =
          new PendingDirectMediaWrite(path, callerUid,
                                      android.os.SystemClock.elapsedRealtime());
      synchronized (PENDING_DIRECT_MEDIA_WRITES) {
        PENDING_DIRECT_MEDIA_WRITES.put(result.toString(), pending);
      }
      logDirectMediaWrite("register", callerUid, String.valueOf(result),
                          path, 0, null);
    } catch (Throwable ignored) {
    }
  }

  private static void completeDirectMediaWriteFromSource(
      Hooker hooker, Object[] rawArgs, Object[] actualArgs, Object result,
      int callerUid) throws FileNotFoundException {
    SourceFdCapture source = null;
    try {
      if (!(hooker != null && hooker.target instanceof Method))
        return;
      Method method = (Method)hooker.target;
      if (!isProviderOpenMethodName(method.getName()))
        return;
      OpenRequest request = parseOpenRequest(method.getName(), actualArgs);
      if (request == null || !isWriteMode(request.mode))
        return;
      PendingDirectMediaWrite pending =
          peekPendingDirectMediaWrite(request.uri);
      if (pending == null || pending.callerUid != callerUid)
        return;
      Object receiver =
          rawArgs != null && rawArgs.length > 0 ? rawArgs[0] : null;
      String path = pending.path;
      if ((path == null || path.length() == 0) &&
          receiver instanceof android.content.ContentProvider) {
        path = queryDataPath((android.content.ContentProvider)receiver,
                             request.uri);
        pending.path = path;
      }
      if (path == null || path.length() == 0) {
        logDirectMediaWrite("no_path", callerUid, String.valueOf(request.uri),
                            null, 0, null);
        return;
      }
      FuseReadOnlyRequest readOnlyRequest =
          buildProviderOpenReadOnlyRequest(method.getName(), request, path,
                                           callerUid);
      if (recordReadOnlyProviderOpenIfNeeded(readOnlyRequest)) {
        logProviderOpenReadOnlyDeny(readOnlyRequest);
        logDirectMediaWrite("readonly", callerUid,
                            String.valueOf(request.uri), path, 0, null);
        closeQuietly(result);
        throw readOnlyOpenException(path);
      }
      source = takeRecentMediaSourceFd(callerUid);
      if (source == null) {
        logDirectMediaWrite("no_source", callerUid,
                            String.valueOf(request.uri), path, 0, null);
        return;
      }
      long bytes = copySourceToPath(source, path);
      pending.bytes = bytes;
      pending.sourcePath = source.path;
      logDirectMediaWrite(bytes > 0 ? "copied" : "empty", callerUid,
                          String.valueOf(request.uri), path, bytes,
                          source.path);
    } catch (FileNotFoundException denied) {
      throw denied;
    } catch (Throwable t) {
      logDirectMediaWrite("failed:" + t.getClass().getName(), callerUid,
                          null, null, 0,
                          source == null ? null : source.path);
    } finally {
      closeQuietly(source);
    }
  }

  private static void finishDirectMediaWriteAfterUpdate(
      Object[] actualArgs, Object result, String mutationMethod) {
    try {
      if (!"update".equals(mutationMethod) || actualArgs == null)
        return;
      int uriIndex = findMutationUriIndex(actualArgs);
      if (uriIndex < 0)
        return;
      PendingDirectMediaWrite pending =
          peekPendingDirectMediaWrite((android.net.Uri)actualArgs[uriIndex]);
      if (pending == null || pending.bytes <= 0)
        return;
      synchronized (PENDING_DIRECT_MEDIA_WRITES) {
        PENDING_DIRECT_MEDIA_WRITES.remove(
            String.valueOf(actualArgs[uriIndex]));
      }
    } catch (Throwable ignored) {
    }
  }

  private static PendingDirectMediaWrite peekPendingDirectMediaWrite(
      android.net.Uri uri) {
    if (uri == null)
      return null;
    synchronized (PENDING_DIRECT_MEDIA_WRITES) {
      PendingDirectMediaWrite pending =
          PENDING_DIRECT_MEDIA_WRITES.get(uri.toString());
      if (pending == null)
        return null;
      long age = android.os.SystemClock.elapsedRealtime() - pending.elapsedMs;
      if (age > 60000L) {
        PENDING_DIRECT_MEDIA_WRITES.remove(uri.toString());
        return null;
      }
      return pending;
    }
  }

  private static SourceFdCapture takeRecentMediaSourceFd(int callerUid) {
    SourceFdCapture source;
    synchronized (RECENT_MEDIA_SOURCE_FDS) {
      source = RECENT_MEDIA_SOURCE_FDS.remove(callerUid);
    }
    if (source == null)
      return null;
    long age = android.os.SystemClock.elapsedRealtime() - source.elapsedMs;
    if (age > 30000L) {
      closeQuietly(source);
      return null;
    }
    return source;
  }

  private static long copySourceToPath(SourceFdCapture source,
                                       String destPath) throws Exception {
    if (source == null || source.pfd == null || destPath == null ||
        destPath.length() == 0)
      return 0;
    File dest = new File(destPath);
    if (dest.exists() && dest.length() > 0)
      return dest.length();
    File parent = dest.getParentFile();
    if (parent != null && !parent.exists())
      parent.mkdirs();
    try {
      android.system.Os.lseek(source.pfd.getFileDescriptor(), 0,
                              android.system.OsConstants.SEEK_SET);
    } catch (Throwable ignored) {
    }
    long total = 0;
    java.io.FileInputStream in =
        new java.io.FileInputStream(source.pfd.getFileDescriptor());
    java.io.FileOutputStream out = new java.io.FileOutputStream(dest, false);
    try {
      byte[] buffer = new byte[64 * 1024];
      while (true) {
        int n = in.read(buffer);
        if (n <= 0)
          break;
        out.write(buffer, 0, n);
        total += n;
      }
      out.getFD().sync();
    } finally {
      try {
        out.close();
      } catch (Throwable ignored) {
      }
      try {
        in.close();
      } catch (Throwable ignored) {
      }
    }
    return total;
  }

  private static long statFileDescriptorSize(ParcelFileDescriptor pfd) {
    if (pfd == null)
      return -1;
    try {
      long size = pfd.getStatSize();
      if (size > 0)
        return size;
    } catch (Throwable ignored) {
    }
    try {
      return android.system.Os.fstat(pfd.getFileDescriptor()).st_size;
    } catch (Throwable ignored) {
      return -1;
    }
  }

  private static String sourceFileDescriptorPath(ParcelFileDescriptor pfd) {
    if (pfd == null)
      return null;
    try {
      return new File("/proc/self/fd/" + pfd.getFd()).getCanonicalPath();
    } catch (Throwable ignored) {
      return null;
    }
  }

  private static void closeQuietly(SourceFdCapture capture) {
    if (capture != null)
      closeQuietly(capture.pfd);
  }

  private static void closeQuietly(Object value) {
    if (value instanceof ParcelFileDescriptor) {
      closeQuietly((ParcelFileDescriptor)value);
      return;
    }
    if (value instanceof AssetFileDescriptor) {
      try {
        ((AssetFileDescriptor)value).close();
      } catch (Throwable ignored) {
      }
    }
  }

  private static void closeQuietly(ParcelFileDescriptor pfd) {
    if (pfd == null)
      return;
    try {
      pfd.close();
    } catch (Throwable ignored) {
    }
  }

  private static void logMediaSourceCapture(int callerUid,
                                            SourceFdCapture capture) {
    if (!shouldLog())
      return;
    if (MEDIA_SOURCE_CAPTURE_LOG_COUNT >= 96)
      return;
    MEDIA_SOURCE_CAPTURE_LOG_COUNT++;
    try {
      android.util.Log.i("SRX", "java media source capture caller_uid=" +
                                    callerUid + " size=" +
                                    (capture == null ? 0 : capture.size) +
                                    " path=" +
                                    (capture == null ? null : capture.path) +
                                    " n=" + MEDIA_SOURCE_CAPTURE_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static void logDirectMediaWrite(String stage, int callerUid,
                                          String uri, String path,
                                          long bytes, String sourcePath) {
    if (!shouldLog())
      return;
    if (DIRECT_MEDIA_WRITE_LOG_COUNT >= 96)
      return;
    DIRECT_MEDIA_WRITE_LOG_COUNT++;
    try {
      android.util.Log.i("SRX", "java media direct write stage=" + stage +
                                    " caller_uid=" + callerUid +
                                    " uri=" + uri + " path=" + path +
                                    " bytes=" + bytes +
                                    " source=" + sourcePath + " n=" +
                                    DIRECT_MEDIA_WRITE_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static final class SourceFdCapture {
    final ParcelFileDescriptor pfd;
    final String path;
    final long size;
    final long elapsedMs;

    SourceFdCapture(ParcelFileDescriptor pfd, String path, long size,
                    long elapsedMs) {
      this.pfd = pfd;
      this.path = path;
      this.size = size;
      this.elapsedMs = elapsedMs;
    }
  }

  private static final class PendingDirectMediaWrite {
    String path;
    final int callerUid;
    final long elapsedMs;
    long bytes;
    String sourcePath;

    PendingDirectMediaWrite(String path, int callerUid, long elapsedMs) {
      this.path = path;
      this.callerUid = callerUid;
      this.elapsedMs = elapsedMs;
    }
  }

  private static ContentValues copyIfNeeded(ContentValues current,
                                            ContentValues original) {
    return current != null ? current : new ContentValues(original);
  }

  private static void replaceActualArg(Object[] rawArgs, Object[] actualArgs,
                                       int index, Object value) {
    actualArgs[index] = value;
    if (rawArgs == null)
      return;
    if (rawArgs.length == 1 && rawArgs[0] instanceof Object[]) {
      ((Object[])rawArgs[0])[index] = value;
      return;
    }
    int rawIndex = index + 1;
    if (rawIndex < rawArgs.length) {
      rawArgs[rawIndex] = value;
    } else if (index < rawArgs.length) {
      rawArgs[index] = value;
    }
  }

  private static String rewriteStoragePathForValues(String path, int callerUid) {
    if (path == null || path.length() == 0)
      return null;
    String rewritten = null;
    try {
      rewritten = rewriteMediaStorePath(path, callerUid);
    } catch (Throwable ignored) {
    }
    if (rewritten != null) {
      if (rewritten.equals(path)) {
        String unchangedFallback = mediaStoreDisplayPath(path, callerUid);
        if (unchangedFallback == null)
          unchangedFallback = normalizeMediaStoreRelativeValuePath(path, callerUid);
        if (unchangedFallback != null && !unchangedFallback.equals(path)) {
          logDebug("rwVals unchanged_native path=" + path + " fallback=" + unchangedFallback);
          return unchangedFallback;
        }
      }
      ensureSandboxParentDir(rewritten);
      String displayPath = mediaStoreDisplayPath(rewritten, callerUid);
      String result = displayPath != null ? displayPath : rewritten;
      logDebug("rwVals sandbox_in=" + path + " rewritten=" + rewritten + " displayPath=" + displayPath + " result=" + result);
      return result;
    }
    String fallback = mediaStoreDisplayPath(path, callerUid);
    if (fallback == null)
      fallback = normalizeMediaStoreRelativeValuePath(path, callerUid);
    logDebug("rwVals no_native path=" + path + " fallback=" + fallback);
    return fallback;
  }


  private static void ensureSandboxParentDir(String sandboxPath) {
    try {
      java.io.File file = new java.io.File(sandboxPath);
      java.io.File parent = file.getParentFile();
      if (parent != null) {
        boolean existed = parent.exists();
        logDebug("ensureSandbox parent=" + parent.getPath() + " exists=" + existed);
        if (!existed) {
          boolean created = parent.mkdirs();
          logDebug("ensureSandbox mkdirs result=" + created + " path=" + parent.getPath());
        }
      }
    } catch (Throwable ignored) {
      logDebug("ensureSandbox failed: " + ignored.getMessage());
    }
  }

  private static String mediaStoreDisplayPath(String path, int callerUid) {
    if (path == null || path.length() == 0)
      return null;
    boolean hasFileScheme = path.startsWith("file://");
    String value = hasFileScheme ? path.substring("file://".length()) : path;
    int userId = userIdFromUid(callerUid);
    if (userId < 0)
      return null;
    String dataRoot = "/data/media/" + userId + "/";
    if (!value.startsWith(dataRoot))
      return null;

    String relative = value.substring(dataRoot.length());
    String androidDataRoot = "Android/data/";
    if (relative.startsWith(androidDataRoot)) {
      int packageEnd = relative.indexOf('/', androidDataRoot.length());
      if (packageEnd >= 0) {
        String rest = relative.substring(packageEnd + 1);
        if ("sdcard".equals(rest)) {
          relative = "";
        } else if (rest.startsWith("sdcard/")) {
          relative = rest.substring("sdcard/".length());
        }
      }
    }

    String displayPath = "/storage/emulated/" + userId;
    if (relative.length() > 0)
      displayPath = displayPath + "/" + relative;
    return hasFileScheme ? "file://" + displayPath : displayPath;
  }

  private static String normalizeMediaStoreRelativeValuePath(String path,
                                                            int callerUid) {
    if (path == null || path.length() == 0 || path.startsWith("/") ||
        path.startsWith("file://") || path.indexOf('\\') >= 0)
      return null;
    String[] segments = path.split("/", -1);
    if (segments.length < 2 || !isPublicMediaRoot(segments[0]))
      return null;
    for (int i = 0; i < segments.length; i++) {
      String segment = segments[i];
      if (segment.length() == 0 || ".".equals(segment) || "..".equals(segment))
        return null;
    }
    int userId = userIdFromUid(callerUid);
    if (userId < 0)
      return null;
    return "/storage/emulated/" + userId + "/" + path;
  }

  private static boolean isPublicMediaRoot(String root) {
    if (root == null)
      return false;
    for (int i = 0; i < PUBLIC_MEDIA_ROOTS.length; i++) {
      if (PUBLIC_MEDIA_ROOTS[i].equals(root))
        return true;
    }
    return false;
  }

  private static String buildMediaStoreProbePath(String relativePath,
                                                 String displayName,
                                                 int callerUid) {
    int userId = userIdFromUid(callerUid);
    if (userId < 0)
      return null;
    String relative = normalizeRelativePathValue(relativePath);
    if (relative.length() == 0)
      return null;
    String absoluteRoot = "storage/emulated/" + userId + "/";
    if (relative.startsWith(absoluteRoot))
      relative = relative.substring(absoluteRoot.length());
    else if (relative.startsWith("/" + absoluteRoot))
      relative = relative.substring(absoluteRoot.length() + 1);
    String name = displayName != null && displayName.length() > 0 ? displayName
                                                                  : ".srx_probe";
    return "/storage/emulated/" + userId + "/" + relative + "/" + name;
  }

  private static String relativePathFromStoragePath(String path, int callerUid) {
    if (path == null || path.length() == 0)
      return null;
    if (path.startsWith("file://"))
      path = path.substring("file://".length());
    int userId = userIdFromUid(callerUid);
    if (userId < 0)
      return null;
    String root = "/storage/emulated/" + userId + "/";
    if (!path.startsWith(root))
      return null;
    int slash = path.lastIndexOf('/');
    if (slash < root.length())
      return null;
    String relative = normalizeRelativePathValue(path.substring(root.length(), slash));
    return relative.length() == 0 ? null : relative + "/";
  }

  private static void patchDirectoryColumns(ContentValues values,
                                            String relativePath) {
    String relative = normalizeRelativePathValue(relativePath);
    if (relative.length() == 0)
      return;
    int slash = relative.indexOf('/');
    String primary = slash >= 0 ? relative.substring(0, slash) : relative;
    String secondary = slash >= 0 ? relative.substring(slash + 1) : null;
    if (values.containsKey("primary_directory"))
      values.put("primary_directory", primary);
    if (values.containsKey("secondary_directory"))
      values.put("secondary_directory", secondary);
  }

  private static String relativePathFromDirectoryColumns(ContentValues values) {
    if (values == null)
      return null;
    String primary = normalizeRelativePathValue(
        values.getAsString("primary_directory"));
    if (primary.length() == 0 || hasUnsafeRelativePathSegment(primary))
      return null;
    String secondary = normalizeRelativePathValue(
        values.getAsString("secondary_directory"));
    if (secondary.length() > 0) {
      if (hasUnsafeRelativePathSegment(secondary))
        return null;
      primary = primary + "/" + secondary;
    }
    return primary + "/";
  }

  private static String firstString(ContentValues values, String firstKey,
                                    String secondKey) {
    String value = values.getAsString(firstKey);
    if (value != null)
      return value;
    return values.getAsString(secondKey);
  }

  private static String normalizeRelativePathValue(String relativePath) {
    String value = relativePath == null ? "" : relativePath.trim();
    while (value.startsWith("/"))
      value = value.substring(1);
    while (value.endsWith("/"))
      value = value.substring(0, value.length() - 1);
    return value;
  }

  private static int userIdFromUid(int uid) {
    if (uid < 0)
      return -1;
    return uid / ANDROID_USER_ID_OFFSET;
  }

  private boolean isStaticTarget() {
    return target instanceof Method &&
        Modifier.isStatic(((Method)target).getModifiers());
  }

  private static void logQueryArgs(Hooker hooker, Object[] args,
                                   int callerUid) {
    if (!shouldLog())
      return;
    if (callerUid == android.os.Process.myUid())
      return;
    if (QUERY_LOG_COUNT >= 64)
      return;
    QUERY_LOG_COUNT++;
    try {
      String targetSig = hooker != null && hooker.target instanceof Method
                             ? describeMethod((Method)hooker.target)
                             : "unknown";
      android.util.Log.i(
          "SRX", "java query"
                     + " target=" + targetSig + " caller_uid=" + callerUid +
                     " caller_pid=" + android.os.Binder.getCallingPid() +
                     " args=" + describeQueryArgs(args));
    } catch (Throwable ignored) {
    }
  }

  private static void logInternalQueryBypass(Hooker hooker) {
    if (!shouldLog())
      return;
    if (INTERNAL_QUERY_LOG_COUNT >= 16)
      return;
    INTERNAL_QUERY_LOG_COUNT++;
    try {
      String targetSig = hooker != null && hooker.target instanceof Method
                             ? describeMethod((Method)hooker.target)
                             : "unknown";
      android.util.Log.i("SRX", "java query internal passthrough target=" +
                                    targetSig + " n=" +
                                    INTERNAL_QUERY_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static void logQueryResult(Object result) {
    if (!shouldLog())
      return;
    if (CURSOR_LOG_COUNT >= 64)
      return;
    CURSOR_LOG_COUNT++;
    try {
      android.util.Log.i(
          "SRX", "java query result=" +
                     (result == null ? "null" : result.getClass().getName()));
    } catch (Throwable ignored) {
    }
  }

  private static void logQueryInvocationShape(Object[] actualArgs, int callerUid,
                                              Object result) {
    if (!shouldLog())
      return;
    if (QUERY_LOG_COUNT >= 64)
      return;
    try {
      StringBuilder shape = new StringBuilder();
      shape.append("java query shape caller_uid=").append(callerUid);
      shape.append(" result=");
      shape.append(result == null ? "null" : result.getClass().getName());
      shape.append(" args=").append(describeQueryArgs(actualArgs));
      if (actualArgs != null) {
        for (Object arg : actualArgs) {
          if (arg instanceof android.os.Bundle) {
            android.os.Bundle bundle = (android.os.Bundle)arg;
            shape.append(" bundle_keys=").append(describeBundle(bundle));
            break;
          }
        }
      }
      android.util.Log.i("SRX", shape.toString());
    } catch (Throwable ignored) {
    }
  }

  private static Cursor emptyQueryCursorIfSafe(Object[] actualArgs,
                                               ProjectionPatch projectionPatch,
                                               int callerUid) {
    NullQueryDecision decision =
        shouldConvertNullMediaQueryToEmpty(actualArgs, callerUid);
    if (!decision.shouldConvert) {
      logNullQuerySkip(decision.reason, callerUid, actualArgs);
      return null;
    }
    String[] columns = null;
    if (projectionPatch != null && projectionPatch.visibleColumns != null) {
      columns = projectionPatch.visibleColumns;
    } else if (actualArgs != null && actualArgs.length > 1 &&
               actualArgs[1] instanceof String[]) {
      columns = (String[])actualArgs[1];
    }
    if (columns == null || columns.length == 0)
      columns = new String[] {"_id"};
    logEmptyQueryCursor(columns, callerUid, actualArgs);
    return new MatrixCursor(columns, 0);
  }

  private static NullQueryDecision shouldConvertNullMediaQueryToEmpty(
      Object[] actualArgs, int callerUid) {
    if (callerUid < ANDROID_APP_UID_START || actualArgs == null ||
        actualArgs.length < 2 || !(actualArgs[0] instanceof android.net.Uri))
      return NullQueryDecision.skip("shape");
    if (!isRedirectEnabledForCallerUid(callerUid))
      return NullQueryDecision.skip("redirect");
    if (isSingleItemQuery(actualArgs))
      return NullQueryDecision.skip("single_item");
    String uri = String.valueOf(actualArgs[0]);
    if (!uri.startsWith("content://media/external"))
      return NullQueryDecision.skip("uri");
    if (!looksLikeMediaEnumerationProjection(actualArgs))
      return NullQueryDecision.skip("projection");
    return NullQueryDecision.convert();
  }

  private static boolean looksLikeMediaEnumerationProjection(Object[] actualArgs) {
    if (actualArgs == null || actualArgs.length < 2 ||
        !(actualArgs[1] instanceof String[]))
      return false;
    String[] projection = (String[])actualArgs[1];
    if (projection == null || projection.length == 0)
      return false;
    boolean hasId = false;
    boolean hasMediaColumn = false;
    for (String column : projection) {
      String normalized = normalizeQueryColumn(column);
      if (normalized.length() == 0)
        continue;
      if ("_id".equals(normalized)) {
        hasId = true;
        continue;
      }
      if (isMediaEnumerationProjectionColumn(normalized))
        hasMediaColumn = true;
    }
    return hasId && (hasMediaColumn || projection.length == 1);
  }

  private static boolean isMediaEnumerationProjectionColumn(String column) {
    return "_data".equals(column) || "data".equals(column) ||
        "relative_path".equals(column) ||
        "_display_name".equals(column) ||
        "display_name".equals(column) ||
        "mime_type".equals(column) ||
        "_size".equals(column) ||
        "size".equals(column) ||
        "bucket_id".equals(column) ||
        "bucket_display_name".equals(column) ||
        "date_added".equals(column) ||
        "date_modified".equals(column) ||
        "orientation".equals(column) ||
        "width".equals(column) ||
        "height".equals(column) ||
        "duration".equals(column) ||
        "media_type".equals(column) ||
        "is_pending".equals(column) ||
        "date_expires".equals(column) ||
        "volume_name".equals(column) ||
        "primary_directory".equals(column) ||
        "secondary_directory".equals(column) ||
        "artist".equals(column) ||
        "album".equals(column) ||
        "title".equals(column);
  }

  private static String normalizeQueryColumn(String column) {
    if (column == null)
      return "";
    return column.trim().toLowerCase(Locale.ROOT);
  }

  private static void logEmptyQueryCursor(String[] columns, int callerUid,
                                          Object[] actualArgs) {
    if (!shouldLog())
      return;
    if (QUERY_NULL_EMPTY_LOG_COUNT >= 32)
      return;
    QUERY_NULL_EMPTY_LOG_COUNT++;
    try {
      android.util.Log.i(
          "SRX", "java query null_to_empty caller_uid=" + callerUid +
                     " columns=" + Arrays.toString(columns) +
                     " args=" + describeQueryArgs(actualArgs) +
                     " n=" + QUERY_NULL_EMPTY_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static void logNullQuerySkip(String reason, int callerUid,
                                       Object[] actualArgs) {
    if (!shouldLog())
      return;
    if (QUERY_NULL_EMPTY_LOG_COUNT >= 32)
      return;
    QUERY_NULL_EMPTY_LOG_COUNT++;
    try {
      android.util.Log.i(
          "SRX", "java query null_to_empty skip reason=" + reason +
                     " caller_uid=" + callerUid +
                     " args=" + describeQueryArgs(actualArgs) +
                     " n=" + QUERY_NULL_EMPTY_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static final class NullQueryDecision {
    final boolean shouldConvert;
    final String reason;

    private NullQueryDecision(boolean shouldConvert, String reason) {
      this.shouldConvert = shouldConvert;
      this.reason = reason;
    }

    static NullQueryDecision convert() {
      return new NullQueryDecision(true, "");
    }

    static NullQueryDecision skip(String reason) {
      return new NullQueryDecision(false, reason);
    }
  }

  private static void logOpenArgs(Hooker hooker, Object[] args, int callerUid,
                                  int callerPid) {
    if (!shouldLog())
      return;
    if (OPEN_LOG_COUNT >= 96)
      return;
    OPEN_LOG_COUNT++;
    try {
      String targetSig = hooker != null && hooker.target instanceof Method
                             ? describeMethod((Method)hooker.target)
                             : "unknown";
      android.util.Log.i("SRX", "java open target=" + targetSig +
                                    " caller_uid=" + callerUid +
                                    " caller_pid=" + callerPid + " args=" +
                                    describeQueryArgs(args));
    } catch (Throwable ignored) {
    }
  }

  private static void logOpenResult(String source, Object result) {
    if (!shouldLog())
      return;
    if (OPEN_RESULT_LOG_COUNT >= 96)
      return;
    OPEN_RESULT_LOG_COUNT++;
    try {
      android.util.Log.i("SRX", "java open result source=" + source +
                                    " class=" +
                                    (result == null ? "null"
                                                    : result.getClass().getName()) +
                                    " n=" + OPEN_RESULT_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static void logOpenDelegate(String reason, String path,
                                      String mappedPath) {
    if (!shouldLog())
      return;
    if (OPEN_DELEGATE_LOG_COUNT >= 96)
      return;
    OPEN_DELEGATE_LOG_COUNT++;
    try {
      android.util.Log.i("SRX", "java open delegate reason=" + reason +
                                    " from=" + path + " to=" +
                                    mappedPath + " n=" +
                                    OPEN_DELEGATE_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static void logMutationArgs(Hooker hooker, Object[] args,
                                      int callerUid, int callerPid,
                                      boolean patched) {
    if (!shouldLog())
      return;
    if (callerUid == android.os.Process.myUid())
      return;
    if (MUTATION_LOG_COUNT >= 96)
      return;
    MUTATION_LOG_COUNT++;
    try {
      String targetSig = hooker != null && hooker.target instanceof Method
                             ? describeMethod((Method)hooker.target)
                             : "unknown";
      android.util.Log.i("SRX", "java media mutation target=" + targetSig +
                                    " caller_uid=" + callerUid +
                                    " caller_pid=" + callerPid +
                                    " patched=" + patched + " args=" +
                                    describeQueryArgs(args));
    } catch (Throwable ignored) {
    }
  }

  private static void logMutationResult(Hooker hooker, Object result) {
    if (!shouldLog())
      return;
    if (MUTATION_RESULT_LOG_COUNT >= 96)
      return;
    MUTATION_RESULT_LOG_COUNT++;
    try {
      String targetSig = hooker != null && hooker.target instanceof Method
                             ? describeMethod((Method)hooker.target)
                             : "unknown";
      android.util.Log.i("SRX", "java media mutation result target=" +
                                    targetSig + " result=" +
                                    String.valueOf(result) + " class=" +
                                    (result == null ? "null"
                                                    : result.getClass().getName()) +
                                    " n=" + MUTATION_RESULT_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static Object[] unwrapArgs(Object[] args) {
    if (args == null || args.length == 0)
      return args;
    if (args.length == 1 && args[0] instanceof Object[])
      return (Object[])args[0];
    Object[] actual = new Object[args.length - 1];
    System.arraycopy(args, 1, actual, 0, actual.length);
    return actual;
  }

  private static final class ProjectionPatch {
    static final ProjectionPatch NONE = new ProjectionPatch(null);
    final String[] visibleColumns;

    private ProjectionPatch(String[] visibleColumns) {
      this.visibleColumns = visibleColumns;
    }

    static ProjectionPatch apply(Object[] rawArgs, Object[] actualArgs) {
      try {
        if (actualArgs == null || actualArgs.length < 2 ||
            !(actualArgs[1] instanceof String[]))
          return NONE;
        String[] projection = (String[])actualArgs[1];
        if (projection == null || projection.length == 0 ||
            hasPathColumn(projection) || !hasIdColumn(projection)) {
          return NONE;
        }
        String[] patched = Arrays.copyOf(projection, projection.length + 1);
        patched[projection.length] = "_data";
        actualArgs[1] = patched;
        if (rawArgs != null && rawArgs.length == 1 &&
            rawArgs[0] instanceof Object[]) {
          ((Object[])rawArgs[0])[1] = patched;
        } else if (rawArgs != null && rawArgs.length > 2) {
          rawArgs[2] = patched;
        }
        return new ProjectionPatch(projection);
      } catch (Throwable ignored) {
        return NONE;
      }
    }

    private static boolean hasPathColumn(String[] columns) {
      for (String column : columns) {
        if ("_data".equals(column) || "data".equalsIgnoreCase(column))
          return true;
      }
      return false;
    }

    private static boolean hasIdColumn(String[] columns) {
      for (String column : columns) {
        if ("_id".equals(column))
          return true;
      }
      return false;
    }

  }

  private static final class SelectionPatch {
    static void apply(Object[] rawArgs, Object[] actualArgs, int callerUid) {
      if (actualArgs == null)
        return;
      patchLegacySelection(rawArgs, actualArgs, callerUid);
      patchBundleSelection(rawArgs, actualArgs, callerUid);
    }

    private static void patchLegacySelection(Object[] rawArgs,
                                             Object[] actualArgs,
                                             int callerUid) {
      if (actualArgs.length < 4 || !(actualArgs[2] instanceof String) ||
          !(actualArgs[3] instanceof String[]))
        return;
      String selection = (String)actualArgs[2];
      String[] selectionArgs = (String[])actualArgs[3];
      String[] patched = patchSelectionArgs(selection, selectionArgs, callerUid);
      String patchedSelection = patchInlineBucketIds(selection, callerUid);
      if (patched == selectionArgs && patchedSelection == selection)
        return;
      logSelectionPatch("legacy", callerUid, selection);
      if (patchedSelection != selection) {
        actualArgs[2] = patchedSelection;
        replaceActualArg(rawArgs, actualArgs, 2, patchedSelection);
      }
      if (patched != selectionArgs) {
        actualArgs[3] = patched;
        replaceActualArg(rawArgs, actualArgs, 3, patched);
      }
    }

    private static void patchBundleSelection(Object[] rawArgs,
                                             Object[] actualArgs,
                                             int callerUid) {
      for (int i = 0; i < actualArgs.length; i++) {
        Object arg = actualArgs[i];
        if (!(arg instanceof android.os.Bundle))
          continue;
        android.os.Bundle bundle = (android.os.Bundle)arg;
        String selection =
            bundle.getString("android:query-arg-sql-selection");
        String[] selectionArgs =
            bundle.getStringArray("android:query-arg-sql-selection-args");
        String[] patched = patchSelectionArgs(selection, selectionArgs, callerUid);
        String patchedSelection = patchInlineBucketIds(selection, callerUid);
        if (patched == selectionArgs && patchedSelection == selection)
          continue;
        logSelectionPatch("bundle", callerUid, selection);
        android.os.Bundle clone = new android.os.Bundle(bundle);
        if (patchedSelection != selection)
          clone.putString("android:query-arg-sql-selection", patchedSelection);
        if (patched != selectionArgs)
          clone.putStringArray("android:query-arg-sql-selection-args", patched);
        actualArgs[i] = clone;
        replaceActualArg(rawArgs, actualArgs, i, clone);
      }
    }

    private static String patchInlineBucketIds(String selection,
                                               int callerUid) {
      if (selection == null || selection.length() == 0 ||
          !containsColumn(selection.toLowerCase(Locale.ROOT), "bucket_id"))
        return selection;
      String patched = patchInlineBucketIdEquals(selection, callerUid);
      patched = patchInlineBucketIdIn(patched, callerUid);
      return patched.equals(selection) ? selection : patched;
    }

    private static String patchInlineBucketIdEquals(String selection,
                                                    int callerUid) {
      Matcher matcher = INLINE_BUCKET_ID_EQUALS.matcher(selection);
      StringBuffer out = null;
      while (matcher.find()) {
        String replacement = rewriteBucketIdValue(matcher.group(3), callerUid);
        if (replacement == null || replacement.equals(matcher.group(3)))
          continue;
        if (out == null)
          out = new StringBuffer(selection.length());
        matcher.appendReplacement(out,
                                  Matcher.quoteReplacement(matcher.group(1) +
                                                           nullToEmpty(matcher.group(2)) +
                                                           replacement +
                                                           nullToEmpty(matcher.group(4))));
      }
      if (out == null)
        return selection;
      matcher.appendTail(out);
      return out.toString();
    }

    private static String patchInlineBucketIdIn(String selection,
                                                int callerUid) {
      Matcher matcher = INLINE_BUCKET_ID_IN.matcher(selection);
      StringBuffer out = null;
      while (matcher.find()) {
        String values = matcher.group(2);
        String patchedValues = patchInlineBucketIdList(values, callerUid);
        if (patchedValues == null || patchedValues.equals(values))
          continue;
        if (out == null)
          out = new StringBuffer(selection.length());
        matcher.appendReplacement(
            out,
            Matcher.quoteReplacement(matcher.group(1) + patchedValues + matcher.group(3)));
      }
      if (out == null)
        return selection;
      matcher.appendTail(out);
      return out.toString();
    }

    private static String patchInlineBucketIdList(String values,
                                                  int callerUid) {
      String[] parts = values.split(",", -1);
      String[] patched = null;
      for (int i = 0; i < parts.length; i++) {
        String raw = parts[i];
        String trimmed = raw.trim();
        String quote = "";
        String numeric = trimmed;
        if (trimmed.length() >= 2 && trimmed.startsWith("'") &&
            trimmed.endsWith("'")) {
          quote = "'";
          numeric = trimmed.substring(1, trimmed.length() - 1);
        }
        if (!isInteger(numeric))
          continue;
        String replacement = rewriteBucketIdValue(numeric, callerUid);
        if (replacement == null || replacement.equals(numeric))
          continue;
        if (patched == null)
          patched = parts.clone();
        int start = raw.indexOf(trimmed);
        String prefix = start > 0 ? raw.substring(0, start) : "";
        String suffix = start >= 0 ? raw.substring(start + trimmed.length()) : "";
        patched[i] = prefix + quote + replacement + quote + suffix;
      }
      if (patched == null)
        return values;
      StringBuilder sb = new StringBuilder(values.length());
      for (int i = 0; i < patched.length; i++) {
        if (i > 0)
          sb.append(',');
        sb.append(patched[i]);
      }
      return sb.toString();
    }

    private static String nullToEmpty(String value) {
      return value == null ? "" : value;
    }

    private static String[] patchSelectionArgs(String selection,
                                               String[] selectionArgs,
                                               int callerUid) {
      if (selection == null || selectionArgs == null ||
          selectionArgs.length == 0)
        return selectionArgs;
      String normalized = selection.toLowerCase(Locale.ROOT);
      if (!selectionMayReferenceMappedMediaPath(normalized))
        return selectionArgs;
      ArrayList<String> columns = placeholderColumns(normalized,
                                                     selectionArgs.length);
      String[] patched = null;
      patched = patchDirectorySelectionArgs(selectionArgs, columns, callerUid,
                                            patched);
      for (int i = 0; i < selectionArgs.length; i++) {
        String value = selectionArgs[i];
        String column = i < columns.size() ? columns.get(i) : null;
        String replacement =
            patchSelectionArg(normalized, column, value, callerUid);
        if (replacement == null || replacement.equals(value))
          continue;
        if (patched == null)
          patched = selectionArgs.clone();
        patched[i] = replacement;
      }
      return patched == null ? selectionArgs : patched;
    }

    private static boolean selectionMayReferenceMappedMediaPath(String selection) {
      return containsColumn(selection, "_data") ||
          containsColumn(selection, "data") ||
          containsColumn(selection, "relative_path") ||
          containsColumn(selection, "primary_directory") ||
          containsColumn(selection, "secondary_directory") ||
          containsColumn(selection, "bucket_id");
    }

    private static String patchSelectionArg(String selection, String column,
                                            String value, int callerUid) {
      if (value == null || value.length() == 0)
        return null;
      if (("bucket_id".equals(column) ||
           (column == null && selection.indexOf("bucket_id") >= 0)) &&
          isInteger(value)) {
        String mappedBucket = rewriteBucketIdValue(value, callerUid);
        if (mappedBucket != null)
          return mappedBucket;
      }
      if ("relative_path".equals(column) ||
          (column == null && looksLikeRelativePath(value))) {
        String mappedRelative = rewriteRelativePathValue(value, callerUid);
        if (mappedRelative != null)
          return mappedRelative;
      }
      if (column != null && !"data".equals(column) && !"_data".equals(column))
        return null;
      String mappedPath = rewriteStoragePathForValues(value, callerUid);
      return mappedPath != null && !mappedPath.equals(value) ? mappedPath : null;
    }

    private static ArrayList<String> placeholderColumns(String selection,
                                                        int argCount) {
      ArrayList<String> columns = new ArrayList<>();
      int cursor = 0;
      while (columns.size() < argCount) {
        int question = selection.indexOf('?', cursor);
        if (question < 0)
          break;
        columns.add(nearestSelectionColumn(selection, question));
        cursor = question + 1;
      }
      return columns;
    }

    private static String nearestSelectionColumn(String selection,
                                                 int questionIndex) {
      String[] candidates = {
          "relative_path", "primary_directory", "secondary_directory",
          "bucket_id", "_data", "data"
      };
      int clauseStart = selectionClauseStart(selection, questionIndex);
      String best = null;
      int bestIndex = -1;
      for (String candidate : candidates) {
        int idx = lastColumnIndexBefore(selection, candidate, questionIndex);
        if (idx >= clauseStart && idx > bestIndex) {
          best = candidate;
          bestIndex = idx;
        }
      }
      return best;
    }

    private static int selectionClauseStart(String selection, int questionIndex) {
      int start = Math.max(
          Math.max(selection.lastIndexOf(" and ", questionIndex),
                   selection.lastIndexOf(" or ", questionIndex)),
          Math.max(selection.lastIndexOf('(', questionIndex),
                   selection.lastIndexOf(',', questionIndex)));
      return start < 0 ? 0 : start + 1;
    }

    private static boolean containsColumn(String selection, String column) {
      return lastColumnIndexBefore(selection, column, selection.length()) >= 0;
    }

    private static int lastColumnIndexBefore(String selection, String column,
                                             int beforeIndex) {
      int searchFrom = Math.min(beforeIndex, selection.length());
      while (searchFrom >= 0) {
        int idx = selection.lastIndexOf(column, searchFrom);
        if (idx < 0)
          return -1;
        int end = idx + column.length();
        if (isColumnBoundary(selection, idx - 1) &&
            isColumnBoundary(selection, end))
          return idx;
        searchFrom = idx - 1;
      }
      return -1;
    }

    private static boolean isColumnBoundary(String value, int index) {
      if (index < 0 || index >= value.length())
        return true;
      char c = value.charAt(index);
      return !(c == '_' || (c >= 'a' && c <= 'z') ||
               (c >= '0' && c <= '9'));
    }

    private static String[] patchDirectorySelectionArgs(String[] selectionArgs,
                                                        ArrayList<String> columns,
                                                        int callerUid,
                                                        String[] patched) {
      int primaryIndex = -1;
      int secondaryIndex = -1;
      for (int i = 0; i < columns.size() && i < selectionArgs.length; i++) {
        String column = columns.get(i);
        if ("primary_directory".equals(column))
          primaryIndex = i;
        else if ("secondary_directory".equals(column))
          secondaryIndex = i;
      }
      if (primaryIndex < 0 || secondaryIndex < 0)
        return patched;
      String primary = selectionArgs[primaryIndex];
      String secondary = selectionArgs[secondaryIndex];
      if (primary == null || primary.length() == 0 || secondary == null ||
          secondary.length() == 0)
        return patched;
      String mappedRelative =
          rewriteRelativePathValue(primary + "/" + secondary + "/", callerUid);
      if (mappedRelative == null)
        return patched;
      String normalized = normalizeRelativePathValue(mappedRelative);
      int slash = normalized.indexOf('/');
      if (slash < 0)
        return patched;
      String mappedPrimary = normalized.substring(0, slash);
      String mappedSecondary = normalized.substring(slash + 1);
      if (mappedPrimary.equals(primary) && mappedSecondary.equals(secondary))
        return patched;
      if (patched == null)
        patched = selectionArgs.clone();
      patched[primaryIndex] = mappedPrimary;
      patched[secondaryIndex] = mappedSecondary;
      return patched;
    }

    private static boolean isInteger(String value) {
      int start = value.startsWith("-") ? 1 : 0;
      if (start >= value.length())
        return false;
      for (int i = start; i < value.length(); i++) {
        char c = value.charAt(i);
        if (c < '0' || c > '9')
          return false;
      }
      return true;
    }

    private static boolean looksLikeRelativePath(String value) {
      if (value.indexOf('/') < 0)
        return false;
      if (value.startsWith("/storage/") || value.startsWith("/mnt/") ||
          value.startsWith("/data/") || value.startsWith("file://"))
        return false;
      return true;
    }

    private static String rewriteRelativePathValue(String value, int callerUid) {
      String normalized = normalizeRelativePathValue(value);
      if (normalized.length() == 0)
        return null;
      String probePath = buildMediaStoreProbePath(normalized, ".srx_probe",
                                                 callerUid);
      String mappedPath = rewriteStoragePathForValues(probePath, callerUid);
      String mappedRelative = relativePathFromStoragePath(mappedPath, callerUid);
      if (mappedRelative == null)
        return null;
      if (normalizeRelativePathValue(mappedRelative).equals(normalized))
        return null;
      return mappedRelative;
    }

    private static String rewriteBucketIdValue(String value, int callerUid) {
      String recent = lookupRecentBucketIdRewrite(callerUid, value);
      if (recent != null)
        return recent;
      try {
        String mapped = rewriteMediaStoreBucketId(value, callerUid);
        rememberBucketIdRewrite(callerUid, value, mapped);
        return mapped;
      } catch (Throwable ignored) {
        return null;
      }
    }

    private static void logSelectionPatch(String kind, int callerUid,
                                          String selection) {
      if (!shouldLog())
        return;
      try {
        android.util.Log.i("SRX", "java query selection patch kind=" + kind +
                                      " caller_uid=" + callerUid +
                                      " selection=" + selection);
      } catch (Throwable ignored) {
      }
    }
  }

  private static void rememberBucketIdRewrite(int callerUid,
                                              String displayBucketId,
                                              String realBucketId) {
    if (callerUid < 0 || displayBucketId == null || realBucketId == null)
      return;
    displayBucketId = displayBucketId.trim();
    realBucketId = realBucketId.trim();
    if (displayBucketId.length() == 0 || realBucketId.length() == 0 ||
        displayBucketId.equals(realBucketId))
      return;
    String key = bucketIdRewriteKey(callerUid, displayBucketId);
    synchronized (RECENT_BUCKET_ID_REWRITES) {
      if (RECENT_BUCKET_ID_REWRITES.size() >= MAX_RECENT_BUCKET_ID_REWRITES &&
          !RECENT_BUCKET_ID_REWRITES.containsKey(key)) {
        RECENT_BUCKET_ID_REWRITES.clear();
      }
      RECENT_BUCKET_ID_REWRITES.put(key, realBucketId);
    }
    logBucketIdRewrite("remember", callerUid, displayBucketId, realBucketId);
  }

  private static String lookupRecentBucketIdRewrite(int callerUid,
                                                    String displayBucketId) {
    if (callerUid < 0 || displayBucketId == null)
      return null;
    displayBucketId = displayBucketId.trim();
    if (displayBucketId.length() == 0)
      return null;
    String realBucketId;
    synchronized (RECENT_BUCKET_ID_REWRITES) {
      realBucketId =
          RECENT_BUCKET_ID_REWRITES.get(bucketIdRewriteKey(callerUid,
                                                           displayBucketId));
    }
    if (realBucketId != null)
      logBucketIdRewrite("hit", callerUid, displayBucketId, realBucketId);
    return realBucketId;
  }

  private static String bucketIdRewriteKey(int callerUid,
                                           String displayBucketId) {
    return callerUid + ":" + displayBucketId;
  }

  private static void logBucketIdRewrite(String stage, int callerUid,
                                         String displayBucketId,
                                         String realBucketId) {
    if (!shouldLog())
      return;
    if (callerUid == android.os.Process.myUid())
      return;
    if (BUCKET_ID_REWRITE_LOG_COUNT >= 96)
      return;
    BUCKET_ID_REWRITE_LOG_COUNT++;
    try {
      android.util.Log.i("SRX", "java bucket_id_rewrite " + stage +
                                    " caller_uid=" + callerUid +
                                    " display=" + displayBucketId +
                                    " real=" + realBucketId +
                                    " n=" + BUCKET_ID_REWRITE_LOG_COUNT);
    } catch (Throwable ignored) {
    }
  }

  private static boolean isSingleItemQuery(Object[] args) {
    try {
      if (args == null || args.length == 0 ||
          !(args[0] instanceof android.net.Uri))
        return false;
      android.net.Uri uri = (android.net.Uri)args[0];
      String last = uri.getLastPathSegment();
      if (last != null && last.length() > 0 && isAllDigits(last))
        return true;
      return hasSingleIdSelection(args);
    } catch (Throwable ignored) {
      return false;
    }
  }

  private static boolean isAllDigits(String value) {
    for (int i = 0; i < value.length(); i++) {
      char c = value.charAt(i);
      if (c < '0' || c > '9')
        return false;
    }
    return true;
  }

  private static boolean hasSingleIdSelection(Object[] args) {
    for (int i = 0; i < args.length; i++) {
      Object arg = args[i];
      if (arg instanceof String && isIdSelection((String)arg)) {
        if (i + 1 >= args.length || args[i + 1] == null)
          return true;
        if (args[i + 1] instanceof String[])
          return isSingleIdSelectionArgs((String[])args[i + 1]);
      } else if (arg instanceof android.os.Bundle &&
                 isSingleIdSelectionBundle((android.os.Bundle)arg)) {
        return true;
      }
    }
    return false;
  }

  private static boolean isIdSelection(String selection) {
    if (selection == null)
      return false;
    String value = compactSqlSelection(selection).toLowerCase(Locale.ROOT);
    return "_id=?".equals(value) || "_id=?1".equals(value) ||
        (value.startsWith("_id=") && isSingleIdOperand(value.substring(4))) ||
        (value.startsWith("_idin(") && value.endsWith(")") &&
         isSingleIdOperand(value.substring(6, value.length() - 1)));
  }

  private static String compactSqlSelection(String selection) {
    StringBuilder sb = new StringBuilder(selection.length());
    for (int i = 0; i < selection.length(); i++) {
      char c = selection.charAt(i);
      if (!Character.isWhitespace(c))
        sb.append(c);
    }
    return sb.toString();
  }

  private static boolean isSingleIdOperand(String value) {
    if (value == null || value.length() == 0 || value.indexOf(',') >= 0)
      return false;
    if (isIntegerText(value))
      return true;
    if (value.charAt(0) != '?')
      return false;
    if (value.length() == 1)
      return true;
    for (int i = 1; i < value.length(); i++) {
      char c = value.charAt(i);
      if (c < '0' || c > '9')
        return false;
    }
    return true;
  }

  private static boolean isIntegerText(String value) {
    int start = value.startsWith("-") ? 1 : 0;
    if (start >= value.length())
      return false;
    for (int i = start; i < value.length(); i++) {
      char c = value.charAt(i);
      if (c < '0' || c > '9')
        return false;
    }
    return true;
  }

  private static boolean isSingleIdSelectionBundle(android.os.Bundle bundle) {
    if (bundle == null)
      return false;
    String selection = bundle.getString("android:query-arg-sql-selection");
    if (!isIdSelection(selection))
      return false;
    String[] args = bundle.getStringArray("android:query-arg-sql-selection-args");
    return isSingleIdSelectionArgs(args);
  }

  private static boolean isSingleIdSelectionArgs(String[] args) {
    if (args == null || args.length == 0)
      return true;
    if (args.length != 1 || args[0] == null)
      return false;
    return isSingleIdOperand(compactSqlSelection(args[0]));
  }

  private static String describeQueryArgs(Object[] args) {
    if (args == null)
      return "null";
    StringBuilder sb = new StringBuilder();
    sb.append('[');
    for (int i = 0; i < args.length; i++) {
      if (i > 0)
        sb.append(", ");
      Object arg = args[i];
      if (arg instanceof android.net.Uri) {
        sb.append("uri=").append(String.valueOf(arg));
      } else if (arg instanceof String) {
        sb.append("String=").append(String.valueOf(arg));
      } else if (arg instanceof String[]) {
        sb.append("String[]").append(Arrays.toString((String[])arg));
      } else if (arg instanceof android.os.Bundle) {
        sb.append("Bundle").append(describeBundle((android.os.Bundle)arg));
      } else if (arg instanceof ContentValues) {
        sb.append("ContentValues")
            .append(describeContentValues((ContentValues)arg));
      } else if (arg instanceof ContentValues[]) {
        ContentValues[] values = (ContentValues[])arg;
        sb.append("ContentValues[](").append(values.length).append(')');
        if (values.length > 0)
          sb.append(describeContentValues(values[0]));
      } else if (arg == null) {
        sb.append("null");
      } else {
        sb.append(arg.getClass().getName());
      }
    }
    sb.append(']');
    return sb.toString();
  }

  private static String describeContentValues(ContentValues values) {
    if (values == null)
      return "null";
    StringBuilder sb = new StringBuilder();
    sb.append('{');
    boolean first = true;
    String[] keys = {
        "_data", "data", "relative_path", "_display_name", "display_name",
        "mime_type", "_size", "size", "is_pending", "date_expires",
        "primary_directory", "secondary_directory"
    };
    for (String key : keys) {
      if (!values.containsKey(key))
        continue;
      if (!first)
        sb.append(", ");
      first = false;
      sb.append(key).append('=').append(values.getAsString(key));
    }
    if (!first)
      sb.append(", ");
    sb.append("size=").append(values.size()).append('}');
    return sb.toString();
  }

  private static String describeBundle(android.os.Bundle bundle) {
    if (bundle == null)
      return "null";
    StringBuilder sb = new StringBuilder();
    sb.append('{');
    boolean first = true;
    try {
      for (String key : bundle.keySet()) {
        if (!first)
          sb.append(", ");
        first = false;
        Object value = bundle.get(key);
        sb.append(key).append('=');
        if (value instanceof String[]) {
          sb.append(Arrays.toString((String[])value));
        } else {
          sb.append(String.valueOf(value));
        }
      }
    } catch (Throwable t) {
      if (!first)
        sb.append(", ");
      sb.append("error=").append(t.getClass().getName());
    }
    sb.append('}');
    return sb.toString();
  }

  private static void logCursor(String stage, Cursor cursor, int pathColumn,
                                int before, int after, int callerUid) {
    if (!shouldLog())
      return;
    if (callerUid == android.os.Process.myUid())
      return;
    if (CURSOR_LOG_COUNT >= 64)
      return;
    CURSOR_LOG_COUNT++;
    try {
      android.util.Log.i(
          "SRX", "java cursor stage=" + stage + " caller_uid=" + callerUid +
                     " class=" + cursor.getClass().getName() + " count=" +
                     before + " after=" + after + " pathColumn=" + pathColumn +
                     " columns=" + Arrays.toString(cursor.getColumnNames()));
    } catch (Throwable ignored) {
    }
  }

  private static void logFilter(String reason, long id, int callerUid) {
    if (!shouldLog())
      return;
    if (callerUid == android.os.Process.myUid())
      return;
    if (FILTER_LOG_COUNT >= 64)
      return;
    FILTER_LOG_COUNT++;
    try {
      android.util.Log.i("SRX", "java cursor " + reason +
                                    " caller_uid=" + callerUid + " id=" + id);
    } catch (Throwable ignored) {
    }
  }

  private static final class FilteringCursor extends AbstractCursor {
    private final Cursor base;
    private final int[] rows;
    private final String[] rewrites;
    private final String[] relativeRewrites;
    private final String[] bucketIdRewrites;
    private final int pathColumn;
    private final int relativePathColumn;
    private final int bucketIdColumn;
    private final String[] visibleColumns;
    private final int baseColumnCount;

    private FilteringCursor(Cursor base, int[] rows, String[] rewrites,
                            int pathColumn, String[] relativeRewrites,
                            int relativePathColumn, String[] bucketIdRewrites,
                            int bucketIdColumn, String[] visibleColumns) {
      this.base = base;
      this.rows = rows;
      this.rewrites = rewrites;
      this.relativeRewrites = relativeRewrites;
      this.bucketIdRewrites = bucketIdRewrites;
      this.pathColumn = pathColumn;
      this.relativePathColumn = relativePathColumn;
      this.bucketIdColumn = bucketIdColumn;
      this.visibleColumns = visibleColumns;
      this.baseColumnCount = safeColumnCount(base);
    }

    static Cursor wrap(Cursor base, int callerUid, String[] visibleColumns,
                       boolean allowMissingMappedTarget, Object[] queryArgs) {
      if (base == null || base.isClosed())
        return base;
      int pathColumn = findPathColumn(base);
      int idColumn = findIdColumn(base);
      int relativePathColumn = findRelativePathColumn(base);
      int bucketIdColumn = findColumnIgnoreCase(base, "bucket_id");
      int count = safeCount(base);
      logCursor("wrap", base, pathColumn, count, count, callerUid);
      if (count <= 0)
        return base;
      if (pathColumn < 0 && relativePathColumn < 0 && idColumn < 0 &&
          bucketIdColumn < 0) {
        return wrapOpaqueAggregateIfHidden(base, callerUid, count,
                                           visibleColumns);
      }

      ArrayList<Integer> visibleRows = new ArrayList<>();
      String[] rewrites = new String[count];
      String[] relativeRewrites = new String[count];
      String[] bucketIdRewrites = new String[count];
      boolean hideOpaqueRows = pathColumn < 0 && relativePathColumn < 0 &&
                               bucketIdColumn < 0 &&
                               isAnyPublicMediaRootHidden(callerUid);
      int recordedQueryPaths = 0;
      int oldPosition = base.getPosition();
      try {
        base.moveToPosition(-1);
        while (base.moveToNext()) {
          int row = base.getPosition();
          long id = readId(base, idColumn);
          if (pathColumn >= 0) {
            String path = base.getString(pathColumn);
            String rewritten =
                path == null ? null : filterPath(path, callerUid);
            if (HIDDEN_ROW_SENTINEL.equals(rewritten) ||
                (rewritten != null && rewritten.length() == 0)) {
              logFilter("filter", id, callerUid);
              continue;
            }
            if ((rewritten == null || rewritten.equals(path)) &&
                isHiddenByCallerVisibility(path, callerUid)) {
              logFilter("filter_visibility", id, callerUid);
              continue;
            }
            if (!allowMissingMappedTarget &&
                !isMediaStorePendingPath(path) &&
                !storagePathExistsForCaller(path, callerUid)) {
              logFilter("filter_missing_plain", id, callerUid);
              continue;
            }
            if (rewritten != null && !rewritten.equals(path)) {
              if (!allowMissingMappedTarget) {
                boolean rewrittenExists =
                    storagePathExistsForCaller(rewritten, callerUid);
                boolean mappingViewRewrite =
                    isMappingViewRewrite(path, rewritten, callerUid);
                if (!rewrittenExists &&
                    (!mappingViewRewrite || !isMediaStorePendingPath(path))) {
                  logFilter("filter_missing_rewrite", id, callerUid);
                  continue;
                }
                if (!rewrittenExists)
                  logFilter("keep_missing_mapping_view", id, callerUid);
              }
              logFilter("rewrite", id, callerUid);
              logKeptRow("rewrite", id, callerUid, path, rewritten, base,
                         bucketIdColumn);
              rewrites[row] = rewritten;
              if (relativePathColumn >= 0) {
                String relative = relativePathFromStoragePath(rewritten, callerUid);
                if (relative != null)
                  relativeRewrites[row] = relative;
              }
              if (bucketIdColumn >= 0) {
                String bucketId = bucketIdFromStoragePath(rewritten, callerUid);
                if (bucketId != null) {
                  bucketIdRewrites[row] = bucketId;
                  rememberBucketIdRewrite(callerUid, bucketId,
                                          readString(base, bucketIdColumn));
                }
              }
            }
            logKeptRow("plain", id, callerUid, path, rewritten, base,
                       bucketIdColumn);
            recordedQueryPaths = recordQueryAccessPaths(
                path, rewritten, callerUid, recordedQueryPaths);
            visibleRows.add(row);
            continue;
          }
          if (relativePathColumn >= 0) {
            String probePath = relativePathProbePath(base, relativePathColumn,
                                                    callerUid);
            if (probePath != null) {
              String rewrittenProbe = filterPath(probePath, callerUid);
              if (HIDDEN_ROW_SENTINEL.equals(rewrittenProbe) ||
                  (rewrittenProbe != null && rewrittenProbe.length() == 0)) {
                logFilter("filter_relative", id, callerUid);
                continue;
              }
              if (rewrittenProbe != null && !rewrittenProbe.equals(probePath)) {
                String relative =
                    relativePathFromStoragePath(rewrittenProbe, callerUid);
                if (relative != null) {
                  relativeRewrites[row] = relative;
                  if (bucketIdColumn >= 0) {
                    String bucketId =
                        bucketIdFromRelativePath(relative, callerUid);
                    if (bucketId != null) {
                      bucketIdRewrites[row] = bucketId;
                      rememberBucketIdRewrite(callerUid, bucketId,
                                              readString(base, bucketIdColumn));
                    }
                  }
                }
              }
            }
            visibleRows.add(row);
            continue;
          }
          String aggregateRootPath = aggregateProbePath(base, callerUid);
          if ((aggregateRootPath != null &&
                isHiddenPath(aggregateRootPath, callerUid)) ||
              (aggregateRootPath == null && hideOpaqueRows)) {
            logFilter("filter_opaque", id, callerUid);
            continue;
          }
          if (bucketIdColumn >= 0 && aggregateRootPath != null) {
            String rewrittenProbe = filterPath(aggregateRootPath, callerUid);
            if (rewrittenProbe != null &&
                !rewrittenProbe.equals(aggregateRootPath)) {
              String bucketId = bucketIdFromStoragePath(rewrittenProbe,
                                                        callerUid);
              if (bucketId != null) {
                bucketIdRewrites[row] = bucketId;
                rememberBucketIdRewrite(callerUid, bucketId,
                                        readString(base, bucketIdColumn));
              }
            }
          }
          visibleRows.add(row);
        }
      } catch (Throwable ignored) {
        return base;
      } finally {
        try {
          base.moveToPosition(oldPosition);
        } catch (Throwable ignored) {
        }
      }

      logCursor("scan", base, pathColumn, count, visibleRows.size(), callerUid);
      logQueryKeep(queryArgs, base, callerUid, count, visibleRows.size(),
                   pathColumn, relativePathColumn, bucketIdColumn, idColumn,
                   allowMissingMappedTarget);
      if (visibleRows.size() == count && !hasAnyRewrite(rewrites) &&
          !hasAnyRewrite(relativeRewrites) && !hasAnyRewrite(bucketIdRewrites) &&
          visibleColumns == null) {
        return base;
      }
      int[] rows = new int[visibleRows.size()];
      for (int i = 0; i < rows.length; i++)
        rows[i] = visibleRows.get(i);
      return new FilteringCursor(base, rows, rewrites, pathColumn,
                                 relativeRewrites, relativePathColumn,
                                 bucketIdRewrites, bucketIdColumn,
                                 visibleColumns);
    }

    private static int recordQueryAccessPaths(String path, String rewritten,
                                              int callerUid, int recorded) {
      if (callerUid == android.os.Process.myUid())
        return recorded;
      recorded = recordQueryAccessPathIfNeeded(path, callerUid, recorded);
      if (rewritten != null && !rewritten.equals(path)) {
        recorded = recordQueryAccessPathIfNeeded(rewritten, callerUid, recorded);
      }
      return recorded;
    }

    private static int recordQueryAccessPathIfNeeded(String path, int callerUid,
                                                     int recorded) {
      if (recorded >= MAX_QUERY_ACCESS_RECORDS_PER_CURSOR || path == null ||
          path.length() == 0) {
        return recorded;
      }
      try {
        recordQueryAccessPath(path, callerUid);
        return recorded + 1;
      } catch (Throwable ignored) {
        return recorded;
      }
    }

    private static void logQueryKeep(Object[] queryArgs, Cursor cursor,
                                     int callerUid, int before, int after,
                                     int pathColumn, int relativePathColumn,
                                     int bucketIdColumn, int idColumn,
                                     boolean allowMissingMappedTarget) {
      if (!shouldLog())
        return;
      if (callerUid == android.os.Process.myUid())
        return;
      if (after <= 0)
        return;
      if (QUERY_KEEP_LOG_COUNT >= 48)
        return;
      QUERY_KEEP_LOG_COUNT++;
      try {
        android.util.Log.i(
            "SRX", "java query_keep caller_uid=" + callerUid +
                       " before=" + before + " after=" + after +
                       " allow_missing=" + allowMissingMappedTarget +
                       " pathColumn=" + pathColumn +
                       " relativePathColumn=" + relativePathColumn +
                       " bucketIdColumn=" + bucketIdColumn +
                       " idColumn=" + idColumn +
                       " columns=" + Arrays.toString(cursor.getColumnNames()) +
                       " args=" + describeQueryArgs(queryArgs));
      } catch (Throwable ignored) {
      }
    }

    private static void logKeptRow(String reason, long id, int callerUid,
                                   String path, String rewritten,
                                   Cursor cursor, int bucketIdColumn) {
      if (!shouldLog())
        return;
      if (callerUid == android.os.Process.myUid())
        return;
      if (KEPT_ROW_LOG_COUNT >= 96)
        return;
      KEPT_ROW_LOG_COUNT++;
      try {
        android.util.Log.i("SRX", "java cursor keep_" + reason +
                                      " caller_uid=" + callerUid +
                                      " id=" + id + " bucket_id=" +
                                      readString(cursor, bucketIdColumn) +
                                      " path=" + path + " rewritten=" +
                                      rewritten + " n=" +
                                      KEPT_ROW_LOG_COUNT);
      } catch (Throwable ignored) {
      }
    }

    private static int safeCount(Cursor cursor) {
      try {
        return cursor.getCount();
      } catch (Throwable ignored) {
        return -1;
      }
    }

    private static int safeColumnCount(Cursor cursor) {
      try {
        return cursor.getColumnCount();
      } catch (Throwable ignored) {
        return 0;
      }
    }

    private static int findPathColumn(Cursor cursor) {
      String[] names = cursor.getColumnNames();
      if (names == null)
        return -1;
      int fallback = -1;
      for (int i = 0; i < names.length; i++) {
        String name = names[i];
        if ("_data".equals(name))
          return i;
        if ("data".equalsIgnoreCase(name))
          fallback = i;
      }
      return fallback;
    }

    private static int findRelativePathColumn(Cursor cursor) {
      return findColumnIgnoreCase(cursor, "relative_path");
    }

    private static int findColumnIgnoreCase(Cursor cursor, String target) {
      String[] names = cursor.getColumnNames();
      if (names == null || target == null)
        return -1;
      for (int i = 0; i < names.length; i++) {
        if (target.equalsIgnoreCase(names[i]))
          return i;
      }
      return -1;
    }

    private static Cursor wrapOpaqueAggregateIfHidden(Cursor base, int callerUid,
                                                      int count,
                                                      String[] visibleColumns) {
      if (!isAnyPublicMediaRootHidden(callerUid))
        return base;
      logFilter("filter_opaque_all", -1, callerUid);
      return new FilteringCursor(base, new int[0], new String[count], -1,
                                 new String[count], -1, new String[count], -1,
                                 visibleColumns);
    }

    private static String aggregateProbePath(Cursor cursor, int callerUid) {
      int bucketIdColumn = findColumnIgnoreCase(cursor, "bucket_id");
      int bucketNameColumn = findColumnIgnoreCase(cursor, "bucket_display_name");
      if (bucketIdColumn < 0)
        return null;
      long bucketId = readLong(cursor, bucketIdColumn, Long.MIN_VALUE);
      String bucketName = readString(cursor, bucketNameColumn);
      if (bucketId == Long.MIN_VALUE)
        return null;
      String path = bucketProbePath(bucketId, bucketName, callerUid);
      logBucketProbe(bucketId, bucketName, callerUid, path);
      return path;
    }

    private static String bucketProbePath(long bucketId, String bucketName,
                                          int callerUid) {
      int userId = userIdFromUid(callerUid);
      if (userId < 0)
        return null;
      String storageRoot = "/storage/emulated/" + userId;
      ArrayList<String> candidates =
          bucketProbeCandidates(bucketName, callerUid);
      for (String relative : candidates) {
        String path = storageRoot + "/" + normalizeRelativePath(relative);
        if (path.toLowerCase(Locale.ROOT).hashCode() == (int)bucketId)
          return path + "/.srx_probe";
      }
      return null;
    }

    private static ArrayList<String> bucketProbeCandidates(String bucketName,
                                                           int callerUid) {
      ArrayList<String> candidates = new ArrayList<>();
      String normalizedName = normalizeRelativePath(bucketName);
      for (String root : PUBLIC_MEDIA_ROOTS) {
        candidates.add(root);
        if (normalizedName.length() > 0)
          candidates.add(root + "/" + normalizedName);
      }
      return candidates;
    }

    private static void logBucketProbe(long bucketId, String bucketName,
                                       int callerUid, String path) {
      if (!shouldLog())
        return;
      if (callerUid == android.os.Process.myUid())
        return;
      if (BUCKET_PROBE_LOG_COUNT >= 96)
        return;
      String name = bucketName == null ? "" : bucketName;
      BUCKET_PROBE_LOG_COUNT++;
      try {
        android.util.Log.i("SRX", "java bucket probe caller_uid=" +
                                      callerUid + " bucket_id=" + bucketId +
                                      " bucket_name=" + name +
                                      " path=" + path + " n=" +
                                      BUCKET_PROBE_LOG_COUNT);
      } catch (Throwable ignored) {
      }
    }

    private static String relativePathProbePath(Cursor cursor,
                                                int relativePathColumn,
                                                int callerUid) {
      String relativePath = readString(cursor, relativePathColumn);
      if (relativePath == null || relativePath.length() == 0)
        return null;
      int userId = userIdFromUid(callerUid);
      if (userId < 0)
        return null;
      return "/storage/emulated/" + userId + "/" +
          normalizeRelativePath(relativePath) + "/.srx_probe";
    }

    private static String bucketIdFromStoragePath(String path, int callerUid) {
      String relative = relativePathFromStoragePath(path, callerUid);
      return bucketIdFromRelativePath(relative, callerUid);
    }

    private static String bucketIdFromRelativePath(String relativePath,
                                                   int callerUid) {
      if (relativePath == null || relativePath.length() == 0)
        return null;
      int userId = userIdFromUid(callerUid);
      if (userId < 0)
        return null;
      String path = "/storage/emulated/" + userId + "/" +
          normalizeRelativePath(relativePath);
      return String.valueOf(path.toLowerCase(Locale.ROOT).hashCode());
    }

    private static boolean isAnyPublicMediaRootHidden(int callerUid) {
      int userId = userIdFromUid(callerUid);
      if (userId < 0)
        return false;
      for (String root : PUBLIC_MEDIA_ROOTS) {
        String probePath = "/storage/emulated/" + userId + "/" + root +
                           "/.srx_probe";
        if (isHiddenPath(probePath, callerUid))
          return true;
      }
      return false;
    }

    private static boolean isHiddenPath(String path, int callerUid) {
      if (path == null || path.length() == 0)
        return false;
      String rewritten = filterPath(path, callerUid);
      return HIDDEN_ROW_SENTINEL.equals(rewritten) ||
          (rewritten != null && rewritten.length() == 0) ||
          ((rewritten == null || rewritten.equals(path)) &&
           isHiddenByCallerVisibility(path, callerUid));
    }

    private static boolean isHiddenByCallerVisibility(String path,
                                                      int callerUid) {
      if (path == null || path.length() == 0)
        return false;
      try {
        return shouldHideCursorPath(path, callerUid);
      } catch (Throwable ignored) {
        return false;
      }
    }

    private static boolean storagePathExists(String path) {
      return pathExistsBySyscall(path);
    }

    private static boolean storagePathExistsForCaller(String path, int callerUid) {
      if (storagePathExists(path))
        return true;
      try {
        String mappedPath = resolveOpenPath(path, callerUid);
        return mappedPath != null && mappedPath.length() > 0 &&
            storagePathExists(mappedPath);
      } catch (Throwable ignored) {
        return false;
      }
    }

    private static boolean isMappingViewRewrite(String originalPath,
                                                String rewrittenPath,
                                                int callerUid) {
      try {
        String openTarget = resolveOpenPath(rewrittenPath, callerUid);
        return storagePathsEquivalent(openTarget, originalPath, callerUid);
      } catch (Throwable ignored) {
        return false;
      }
    }

    private static boolean storagePathsEquivalent(String left, String right,
                                                  int callerUid) {
      String normalizedLeft = normalizeComparableStoragePath(left, callerUid);
      String normalizedRight = normalizeComparableStoragePath(right, callerUid);
      return normalizedLeft != null && normalizedLeft.equals(normalizedRight);
    }

    private static boolean isMediaStorePendingPath(String path) {
      if (path == null || path.length() == 0)
        return false;
      String value = path.startsWith("file://") ? path.substring("file://".length()) : path;
      int slash = value.lastIndexOf('/');
      String name = slash >= 0 ? value.substring(slash + 1) : value;
      return name.startsWith(".pending-");
    }

    private static String normalizeComparableStoragePath(String path,
                                                         int callerUid) {
      if (path == null || path.length() == 0)
        return null;
      String value = path.trim();
      if (value.startsWith("file://"))
        value = value.substring("file://".length());
      int userId = userIdFromUid(callerUid);
      if (userId < 0)
        return value;
      String dataRoot = "/data/media/" + userId + "/";
      if (value.startsWith(dataRoot)) {
        value = "/storage/emulated/" + userId + "/" +
            value.substring(dataRoot.length());
      }
      String fuseRoot = "/mnt/user/" + userId + "/primary/";
      if (value.startsWith(fuseRoot)) {
        value = "/storage/emulated/" + userId + "/" +
            value.substring(fuseRoot.length());
      }
      while (value.endsWith("/") && value.length() > 1)
        value = value.substring(0, value.length() - 1);
      return value;
    }

    private static int userIdFromUid(int uid) {
      if (uid < 0)
        return -1;
      return uid / ANDROID_USER_ID_OFFSET;
    }

    private static String normalizeRelativePath(String relativePath) {
      String value = relativePath == null ? "" : relativePath.trim();
      while (value.startsWith("/"))
        value = value.substring(1);
      while (value.endsWith("/"))
        value = value.substring(0, value.length() - 1);
      return value;
    }

    private static String readString(Cursor cursor, int column) {
      if (column < 0)
        return null;
      try {
        return cursor.getString(column);
      } catch (Throwable ignored) {
        return null;
      }
    }

    private static long readLong(Cursor cursor, int column, long fallback) {
      if (column < 0)
        return fallback;
      try {
        return cursor.getLong(column);
      } catch (Throwable ignored) {
        return fallback;
      }
    }

    private static int findIdColumn(Cursor cursor) {
      String[] names = cursor.getColumnNames();
      if (names == null)
        return -1;
      for (int i = 0; i < names.length; i++) {
        if ("_id".equals(names[i]))
          return i;
      }
      return -1;
    }

    private static long readId(Cursor cursor, int idColumn) {
      if (idColumn < 0)
        return -1;
      try {
        return cursor.getLong(idColumn);
      } catch (Throwable ignored) {
        return -1;
      }
    }

    private static boolean hasAnyRewrite(String[] rewrites) {
      for (String rewrite : rewrites) {
        if (rewrite != null)
          return true;
      }
      return false;
    }

    @Override
    public int getCount() {
      return rows.length;
    }

    @Override
    public String[] getColumnNames() {
      return visibleColumns != null ? visibleColumns : base.getColumnNames();
    }

    @Override
    public short getShort(int column) {
      return base.getShort(column);
    }

    @Override
    public int getInt(int column) {
      if (column == bucketIdColumn) {
        int row = currentBaseRow();
        String rewritten = row >= 0 ? bucketIdRewrites[row] : null;
        if (rewritten != null) {
          try {
            return Integer.parseInt(rewritten);
          } catch (Throwable ignored) {
          }
        }
      }
      return base.getInt(column);
    }

    @Override
    public long getLong(int column) {
      if (column == bucketIdColumn) {
        int row = currentBaseRow();
        String rewritten = row >= 0 ? bucketIdRewrites[row] : null;
        if (rewritten != null) {
          try {
            return Long.parseLong(rewritten);
          } catch (Throwable ignored) {
          }
        }
      }
      return base.getLong(column);
    }

    @Override
    public float getFloat(int column) {
      return base.getFloat(column);
    }

    @Override
    public double getDouble(int column) {
      return base.getDouble(column);
    }

    @Override
    public String getString(int column) {
      int row = currentBaseRow();
      if (column == pathColumn) {
        String rewritten = row >= 0 ? rewrites[row] : null;
        if (rewritten != null)
          return rewritten;
      }
      if (column == relativePathColumn) {
        String rewritten = row >= 0 ? relativeRewrites[row] : null;
        if (rewritten != null)
          return rewritten;
      }
      if (column == bucketIdColumn) {
        String rewritten = row >= 0 ? bucketIdRewrites[row] : null;
        if (rewritten != null)
          return rewritten;
      }
      return base.getString(column);
    }

    @Override
    public boolean isNull(int column) {
      return base.isNull(column);
    }

    @Override
    public byte[] getBlob(int column) {
      return base.getBlob(column);
    }

    @Override
    public int getType(int column) {
      return base.getType(column);
    }

    @Override
    public boolean onMove(int oldPosition, int newPosition) {
      return base.moveToPosition(rows[newPosition]);
    }

    @Override
    public void close() {
      base.close();
      super.close();
    }

    private int currentBaseRow() {
      if (mPos < 0 || mPos >= rows.length)
        return -1;
      return rows[mPos];
    }
  }
}
