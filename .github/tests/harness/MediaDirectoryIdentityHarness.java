import java.util.Iterator;
import java.util.LinkedHashMap;
import java.util.HashMap;
import java.util.Map;

public class MediaDirectoryIdentityHarness {
  static final long RECENT_MEDIA_DIRECTORY_CALLER_TTL_MS = 50;
  static final int RECENT_MEDIA_DIRECTORY_CALLER_LIMIT = 16;
  static final LinkedHashMap<String, DirectoryCallerHint> RECENT_MEDIA_DIRECTORY_CALLERS = new LinkedHashMap<>();
  static final Map<String, CallerIdentity> TASKS = new HashMap<>();
  static CallerIdentity current;
  static String restored;
  static class android {
    static class os {
      static class Binder {
        static int uid = 1023, pid = 900;
        static int getCallingUid() { return uid; }
        static int getCallingPid() { return pid; }
      }
      static class Process {
        static int myUid() { return 1023; }
        static int myPid() { return 900; }
      }
      static class SystemClock {
        static long now = 100;
        static long elapsedRealtime() { return now; }
      }
    }
  }
  static class CallerIdentity {
    int uid, pid;
    String packageName, snapshot;
    boolean restorable;
    CallerIdentity(int uid, int pid, String snapshot) {
      CallerIdentity original = TASKS.get(snapshot);
      this.uid = uid;
      this.pid = pid;
      this.snapshot = snapshot;
      this.packageName = original.packageName;
      this.restorable = original.restorable;
    }
    CallerIdentity(int uid, int pid, String pkg, String snapshot, boolean restorable) {
      this.uid = uid;
      this.pid = pid;
      this.packageName = pkg;
      this.snapshot = snapshot;
      this.restorable = restorable;
      TASKS.put(snapshot, this);
    }
  }
  static CallerIdentity currentCallerIdentity() { return current; }
  static void enterCallerScope(int uid, int pid) {
    restored = null;
    current = new CallerIdentity(uid, pid, "", "fresh-" + uid + "-" + pid, false);
  }
  static void enterCallerScope(int uid, int pid, String pkg, String snapshot) {
    restored = snapshot;
    current = new CallerIdentity(uid, pid, snapshot);
  }
  static void check(boolean value, String label) {
    if (!value) throw new AssertionError(label);
  }

  // __DIRECTORY_IDENTITY_IMPLEMENTATION__

  public static void main(String[] args) {
    String path = "/storage/emulated/0/Download/test";
    CallerIdentity first = new CallerIdentity(10123, 42, "test.first", "first-task-55", true);
    current = first;
    rememberMediaDirectoryCaller(path, first.uid);
    rememberMediaDirectoryCaller(path, first.uid);
    current = null;
    check(enterDirectoryCallerScope("/data/media/0/Download/test"), "internal restore entered");
    check(current.uid == first.uid && current.pid == first.pid, "original UID and PID");
    check("first-task-55".equals(restored), "original full task snapshot");
    check(!enterDirectoryCallerScope(path), "nested scope retained");
    check("first-task-55".equals(restored), "nested scope unchanged");
    for (int uid : new int[] {0, 2000, 10124, 1023}) {
      current = null;
      android.os.Binder.uid = uid;
      android.os.Binder.pid = 901;
      check(enterDirectoryCallerScope(path), "external caller entered");
      check(current.uid == uid && current.pid == 901, "external caller wins over path hint");
      check(restored == null, "external caller did not restore another task");
    }
    android.os.Binder.uid = 1023;
    android.os.Binder.pid = 900;
    current = new CallerIdentity(10123, 43, "test.second", "second-task-56", true);
    rememberMediaDirectoryCaller(path, 10123);
    check(recentMediaDirectoryCaller(path) == null, "shared UID conflict closed");
    current = first;
    rememberMediaDirectoryCaller(path, 10123);
    check(recentMediaDirectoryCaller(path) == null, "conflict cannot be overwritten before expiry");
    android.os.SystemClock.now += RECENT_MEDIA_DIRECTORY_CALLER_TTL_MS + 1;
    rememberMediaDirectoryCaller(path, 10123);
    check(recentMediaDirectoryCaller(path) != null, "expired conflict permits new evidence");
    current = new CallerIdentity(10123, 44, "", "unverified-task", false);
    rememberMediaDirectoryCaller(path, 10123);
    check(recentMediaDirectoryCaller(path) == null, "unverified producer invalidates old evidence");
    current = null;
    enterDirectoryCallerScope(path);
    check(restored == null && current.uid == 1023, "unknown internal request keeps host identity");
    RECENT_MEDIA_DIRECTORY_CALLERS.clear();
    current = first;
    rememberMediaDirectoryCaller(path, first.uid);
    current = new CallerIdentity(10123, 43, "test.second", "second-task-56", true);
    rememberMediaDirectoryCaller(path, 10123);
    for (int index = 1; index < RECENT_MEDIA_DIRECTORY_CALLER_LIMIT; index++) {
      current = first;
      rememberMediaDirectoryCaller(path + "/" + index, first.uid);
    }
    check(RECENT_MEDIA_DIRECTORY_CALLERS.size() == RECENT_MEDIA_DIRECTORY_CALLER_LIMIT, "full cache");
    rememberMediaDirectoryCaller(path, first.uid);
    check(recentMediaDirectoryCaller(path) == null, "full cache preserves existing conflict");
    rememberMediaDirectoryCaller(path + "/overflow", first.uid);
    check(recentMediaDirectoryCaller(path + "/overflow") == null, "capacity does not reopen old evidence");
    check(recentMediaDirectoryCaller(path) == null, "overflow keeps conflict closed");
    android.os.SystemClock.now += RECENT_MEDIA_DIRECTORY_CALLER_TTL_MS + 1;
    rememberMediaDirectoryCaller(path + "/overflow", first.uid);
    check(recentMediaDirectoryCaller(path + "/overflow") != null, "expiry frees cache capacity");
    System.out.println("DIRECTORY IDENTITY CASES PASSED");
  }
}
