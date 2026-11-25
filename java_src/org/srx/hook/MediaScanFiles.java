package org.srx.hook;

import android.content.ContentProvider;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.provider.MediaStore;
import android.system.Os;
import android.system.OsConstants;
import java.io.IOException;
import java.util.Arrays;
import java.util.HashSet;
import java.util.Set;

final class MediaScanFiles {
  private static final String[] COLUMNS = {
    "_id", "volume_name", "generation_added", "owner_package_name", "_data"
  };
  private final ContentProvider provider;
  final MediaBindings bindings;

  MediaScanFiles(ContentProvider provider) {
    this.provider = provider;
    bindings = new MediaBindings(provider);
  }

  static String backing(String logical, String owner, int user) {
    String prefix = "/storage/emulated/" + user + "/";
    if (logical == null
        || !logical.startsWith(prefix)
        || owner == null
        || owner.isEmpty()
        || owner.indexOf('/') >= 0
        || owner.equals(".")
        || owner.equals("..")) return null;
    String relative = logical.substring(prefix.length());
    if (relative.isEmpty() || relative.equals("Android") || relative.startsWith("Android/"))
      return null;
    for (String part : relative.split("/", -1)) {
      if (part.isEmpty() || part.equals(".") || part.equals("..")) return null;
    }
    return "/data/media/" + user + "/Android/data/" + owner + "/sdcard/" + relative;
  }

  private int user() {
    return android.os.Process.myUid() / 100000;
  }

  private String identity(Cursor row) {
    return row.getString(1) + "/" + row.getLong(0) + "/" + row.getLong(2) + "/" + row.getString(3);
  }

  private String verifiedBacking(Cursor row, Set<String> ids, String version) {
    if (version == null || !ids.contains(version + "/" + identity(row))) return null;
    return backing(row.getString(4), row.getString(3), user());
  }

  static final class Unavailable extends RuntimeException {
    final boolean temporary;

    Unavailable(boolean temporary) {
      this.temporary = temporary;
    }
  }

  private ParcelFileDescriptor openBacking(String path) {
    if (path == null) return null;
    ParcelFileDescriptor result = null;
    try {
      java.io.FileDescriptor fd =
          Os.open(
              path,
              OsConstants.O_RDONLY
                  | OsConstants.O_NONBLOCK
                  | OsConstants.O_NOFOLLOW
                  | OsConstants.O_CLOEXEC,
              0);
      try {
        result = ParcelFileDescriptor.dup(fd);
      } finally {
        Os.close(fd);
      }
      if (OsConstants.S_ISREG(Os.fstat(result.getFileDescriptor()).st_mode)
          && Hooker.isVerifiedMediaFd(path, result.getFd())) {
        ParcelFileDescriptor verified = result;
        result = null;
        return verified;
      }
      throw new Unavailable(true);
    } catch (android.system.ErrnoException e) {
      String state = android.os.Environment.getExternalStorageState();
      android.os.UserManager users =
          provider.getContext().getSystemService(android.os.UserManager.class);
      boolean available =
          android.os.Environment.MEDIA_MOUNTED.equals(state)
              || android.os.Environment.MEDIA_MOUNTED_READ_ONLY.equals(state);
      throw new Unavailable(
          e.errno != OsConstants.ENOENT || !available || users == null || !users.isUserUnlocked());
    } catch (IOException e) {
      throw new Unavailable(true);
    } finally {
      if (result != null)
        try {
          result.close();
        } catch (IOException ignored) {
        }
    }
  }

  void finishPublication(Uri uri, String owner) {
    String volume = MediaStore.getVolumeName(uri);
    if (!("external".equals(volume) || "external_primary".equals(volume))
        || uri.getPathSegments().size() < 3) return;
    String category = uri.getPathSegments().get(1);
    if (!("file".equals(category)
        || "downloads".equals(category)
        || "images".equals(category)
        || "video".equals(category)
        || "audio".equals(category))) return;
    bindings.write(
        db -> {
          String version = bindings.uuid();
          bindings.ready(db, version);
          String[] args = {version, uri.getLastPathSegment(), owner};
          try (Cursor row =
              db.rawQuery(
                  "SELECT f._data,f._size"
                      + MediaBindings.JOIN
                      + " AND f._id=? AND b.owner=? AND f.is_pending=0",
                  args)) {
            if (!row.moveToFirst()) return null;
            String logical = row.getString(0);
            try (ParcelFileDescriptor fd = openBacking(backing(logical, owner, user()))) {
              if (fd == null) return null;
              long size = Os.fstat(fd.getFileDescriptor()).st_size;
              if (row.isNull(1) || row.getLong(1) != size)
                db.execSQL(
                    "UPDATE files SET _size=? WHERE _id=?",
                    new Object[] {size, uri.getLastPathSegment()});
            } catch (Unavailable | android.system.ErrnoException | IOException ignored) {
            }
          }
          return null;
        });
  }

  ParcelFileDescriptor open(String logical) {
    if (logical == null || !logical.startsWith("/storage/emulated/" + user() + "/")) return null;
    String physical =
        bindings.read(
            db -> {
              String version = bindings.uuid();
              bindings.ready(db, version);
              try (Cursor row =
                  db.rawQuery(
                      "SELECT f._data,f.owner_package_name" + MediaBindings.JOIN + " AND f._data=?",
                      new String[] {version, logical})) {
                return row.moveToFirst()
                    ? backing(row.getString(0), row.getString(1), user())
                    : null;
              }
            });
    return openBacking(physical);
  }

  long[] includePrivateFiles(Uri collection, Bundle args, long[] seen) {
    if (!"external_primary".equals(MediaStore.getVolumeName(collection))) return seen;
    String version = bindings.uuid();
    Set<String> ids =
        bindings.read(
            db -> {
              bindings.ready(db, version);
              Set<String> result = new HashSet<>();
              try (Cursor rows =
                  db.rawQuery(
                      "SELECT f._id,f.volume_name,f.generation_added,f.owner_package_name"
                          + MediaBindings.JOIN,
                      new String[] {version})) {
                while (rows.moveToNext()) result.add(version + "/" + identity(rows));
              }
              return result;
            });
    if (ids.isEmpty()) return seen;
    long[] result = seen;
    int count = seen.length;
    try (Cursor rows = provider.query(collection, COLUMNS, args, null)) {
      while (rows != null && rows.moveToNext()) {
        long id = rows.getLong(0);
        String path = rows.getString(4);
        if (path == null || Arrays.binarySearch(seen, id) >= 0) continue;
        String physical = verifiedBacking(rows, ids, version);
        if (physical == null) continue;
        try (ParcelFileDescriptor fd = openBacking(physical)) {
          if (fd == null) continue;
        } catch (Unavailable unavailable) {
          if (!unavailable.temporary) continue;
        } catch (IOException ignored) {
        }
        if (count == result.length) result = Arrays.copyOf(result, count + Math.max(8, count / 2));
        result[count++] = id;
      }
    }
    if (count == seen.length) return seen;
    result = Arrays.copyOf(result, count);
    Arrays.sort(result);
    return result;
  }
}
