package org.srx.hook;

import android.content.ContentProvider;
import android.content.SharedPreferences;
import android.database.Cursor;
import android.database.DatabaseUtils;
import android.database.sqlite.SQLiteDatabase;
import android.database.sqlite.SQLiteStatement;
import android.system.Os;
import java.io.File;
import java.lang.reflect.InvocationTargetException;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.function.Function;

final class MediaBindings {
  private static final String CREATE =
      "CREATE TABLE IF NOT EXISTS srx_private_media("
          + "file_id INTEGER PRIMARY KEY REFERENCES files(_id) ON DELETE CASCADE ON UPDATE CASCADE,"
          + "uuid TEXT NOT NULL,volume TEXT NOT NULL,generation INTEGER NOT NULL,owner TEXT NOT NULL,"
          + "user_id INTEGER NOT NULL)";
  private static final String INVALIDATE =
      "CREATE TRIGGER IF NOT EXISTS srx_private_media_identity "
          + "AFTER UPDATE OF _id,volume_name,generation_added,owner_package_name,_user_id ON files "
          + "WHEN old._id IS NOT new._id OR old.volume_name IS NOT new.volume_name "
          + "OR old.generation_added IS NOT new.generation_added "
          + "OR old.owner_package_name IS NOT new.owner_package_name OR old._user_id IS NOT new._user_id "
          + "BEGIN DELETE FROM srx_private_media WHERE file_id=new._id; END";
  private static final String MOVE_SCOPE =
      "CREATE TABLE IF NOT EXISTS srx_private_media_move("
          + "target TEXT PRIMARY KEY NOT NULL,owner TEXT NOT NULL)";
  private static final String REVOKE_MOVE =
      "CREATE TRIGGER IF NOT EXISTS srx_private_media_path "
          + "AFTER UPDATE OF _data ON files WHEN old._data IS NOT new._data AND NOT EXISTS "
          + "(SELECT 1 FROM srx_private_media_move WHERE target=new._data COLLATE BINARY "
          + "AND owner=new.owner_package_name) BEGIN DELETE FROM srx_private_media WHERE file_id=new._id; END";
  private static final String SELECT_BINDING =
      "SELECT _id,?,volume_name,generation_added,owner_package_name,_user_id "
          + "FROM files WHERE volume_name='external_primary' AND _id=? AND owner_package_name=?";
  static final String JOIN =
      " FROM files f JOIN srx_private_media b ON b.file_id=f._id "
          + "AND b.volume=f.volume_name AND b.generation=f.generation_added AND b.owner=f.owner_package_name "
          + "AND b.user_id=f._user_id WHERE b.uuid=?";
  private final ContentProvider provider;
  final String path;
  private final SharedPreferences legacy;
  private volatile boolean legacyRetired;

  MediaBindings(ContentProvider provider) {
    this.provider = provider;
    path = provider.getContext().getDatabasePath("external.db").getPath();
    legacy = provider.getContext().getSharedPreferences("srx_private_media", 0);
  }

  String uuid() {
    return uuid(path);
  }

  private static String uuid(String databasePath) {
    try {
      String value = new String(Os.getxattr(databasePath, "user.uuid"), StandardCharsets.US_ASCII);
      if (UUID.fromString(value).toString().equals(value)) return value;
    } catch (android.system.ErrnoException | IllegalArgumentException ignored) {
    }
    throw new IllegalStateException("无法确认外部媒体数据库的身份");
  }

  Object helper() {
    return Hooker.fieldValue(provider, "mExternalDatabase");
  }

  <T> T read(Function<SQLiteDatabase, T> operation) {
    return withDatabase("runWithoutTransaction", operation);
  }

  <T> T write(Function<SQLiteDatabase, T> operation) {
    return withDatabase("runWithTransaction", operation);
  }

  private <T> T withDatabase(String method, Function<SQLiteDatabase, T> operation) {
    Object helper = helper();
    if (helper == null) throw new IllegalStateException("外部媒体数据库尚未打开");
    try {
      @SuppressWarnings("unchecked")
      T result = (T) helper.getClass().getMethod(method, Function.class).invoke(helper, operation);
      return result;
    } catch (InvocationTargetException e) {
      if (e.getCause() instanceof RuntimeException) throw (RuntimeException) e.getCause();
      throw new IllegalStateException(e.getCause());
    } catch (ReflectiveOperationException e) {
      throw new IllegalStateException(e);
    }
  }

  void ready(SQLiteDatabase db, String uuid) {
    if (!path.equals(db.getPath())) throw new IllegalStateException("媒体绑定不属于当前数据库");
    if (DatabaseUtils.longForQuery(
                db,
                "SELECT count(*) FROM sqlite_master WHERE name IN "
                    + "('srx_private_media','srx_private_media_identity','srx_private_media_path',"
                    + "'srx_private_media_move','srx_private_media_migration')",
                null)
            != 5
        || !migrationComplete(db, uuid)) prepare(db, uuid);
    if (!db.inTransaction() && !legacyRetired) {
      legacyRetired = legacy.getAll().isEmpty() || legacy.edit().clear().commit();
    }
  }

  private static void createSchema(SQLiteDatabase db) {
    if (DatabaseUtils.longForQuery(db, "PRAGMA foreign_keys", null) != 1)
      throw new IllegalStateException("媒体数据库必须启用绑定表的外键约束");
    db.execSQL(CREATE);
    db.execSQL(INVALIDATE);
    db.execSQL(MOVE_SCOPE);
    db.execSQL(REVOKE_MOVE);
    db.execSQL(
        "CREATE TABLE IF NOT EXISTS srx_private_media_migration(uuid TEXT PRIMARY KEY NOT NULL)");
  }

  private static boolean migrationComplete(SQLiteDatabase db, String uuid) {
    return DatabaseUtils.longForQuery(
            db,
            "SELECT count(*) FROM srx_private_media_migration WHERE uuid=?",
            new String[] {uuid})
        == 1;
  }

  private void prepare(SQLiteDatabase db, String uuid) {
    boolean outer = db.inTransaction();
    if (!outer) db.beginTransactionNonExclusive();
    try {
      createSchema(db);
      if (!migrationComplete(db, uuid)) {
        migrate(db, uuid);
        db.execSQL("INSERT INTO srx_private_media_migration VALUES(?)", new Object[] {uuid});
      }
      if (!outer) db.setTransactionSuccessful();
    } finally {
      if (!outer) db.endTransaction();
    }
  }

  static boolean installed(SQLiteDatabase db) {
    return "external.db".equals(new File(db.getPath()).getName())
        && DatabaseUtils.longForQuery(
                db,
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='srx_private_media'",
                null)
            == 1;
  }

  static Object upgrade(SQLiteDatabase db, Insert operation) throws Throwable {
    if (!installed(db)) return operation.run();
    if (!db.inTransaction()) throw new IllegalStateException("升级媒体数据库结构需要已有事务");
    String version = uuid(db.getPath());
    db.execSQL(
        "CREATE TEMP TABLE srx_media_before_upgrade AS SELECT b.*, f._data AS path" + JOIN,
        new Object[] {version});
    try {
      Object result = operation.run();
      createSchema(db);
      if (version.equals(uuid(db.getPath())))
        db.execSQL(
            "INSERT OR IGNORE INTO srx_private_media SELECT s.file_id,s.uuid,s.volume,s.generation,s.owner,s.user_id "
                + "FROM temp.srx_media_before_upgrade s JOIN files f ON f._id=s.file_id AND f.volume_name=s.volume "
                + "AND f.generation_added=s.generation AND f.owner_package_name=s.owner AND f._user_id=s.user_id "
                + "AND f._data=s.path COLLATE BINARY");
      return result;
    } finally {
      db.execSQL("DROP TABLE temp.srx_media_before_upgrade");
    }
  }

  static Object move(SQLiteDatabase db, String owner, String target, Insert original)
      throws Throwable {
    db.execSQL("SAVEPOINT srx_private_move");
    try {
      db.execSQL(
          "INSERT INTO srx_private_media_move(target,owner) VALUES(?,?)",
          new Object[] {target, owner});
      Object result = original.run();
      db.execSQL("DELETE FROM srx_private_media_move WHERE target=?", new Object[] {target});
      db.execSQL("RELEASE SAVEPOINT srx_private_move");
      return result;
    } catch (Throwable error) {
      db.execSQL("ROLLBACK TO SAVEPOINT srx_private_move");
      db.execSQL("RELEASE SAVEPOINT srx_private_move");
      throw error;
    }
  }

  private void migrate(SQLiteDatabase db, String uuid) {
    for (Map.Entry<String, ?> group : legacy.getAll().entrySet()) {
      if (!(group.getValue() instanceof Set)) continue;
      for (Object item : (Set<?>) group.getValue()) {
        if (!(item instanceof String)) continue;
        String[] key = ((String) item).split("/", -1);
        if (key.length != 5 || !uuid.equals(key[0]) || !"external_primary".equals(key[1])) continue;
        try (Cursor row =
            db.rawQuery(
                "SELECT _data FROM files WHERE _id=? AND generation_added=? "
                    + "AND volume_name=? AND owner_package_name=?",
                new String[] {key[2], key[3], key[1], key[4]})) {
          if (!row.moveToFirst()) continue;
          String logical = row.getString(0);
          if (MediaScanFiles.backing(logical, key[4], android.os.Process.myUid() / 100000) == null
              || !group.getKey().equals(new File(logical).getParent())) continue;
          db.execSQL(
              "INSERT OR IGNORE INTO srx_private_media " + SELECT_BINDING,
              new Object[] {uuid, key[2], key[4]});
        }
      }
    }
  }

  interface Insert {
    Object run() throws Throwable;
  }

  Object insert(SQLiteDatabase db, String uuid, String owner, String logical, Insert original)
      throws Throwable {
    Object stateField = Hooker.fieldValue(helper(), "mTransactionState");
    Object state = stateField instanceof ThreadLocal ? ((ThreadLocal<?>) stateField).get() : null;
    PendingEvents pending = new PendingEvents(state);
    db.execSQL("SAVEPOINT srx_private_insert");
    try {
      Object result = original.run();
      try {
        bind(db, uuid, ((Number) result).longValue(), owner, logical);
      } catch (RuntimeException e) {
        throw new IllegalStateException("无法提交私有媒体文件绑定", e);
      }
      db.execSQL("RELEASE SAVEPOINT srx_private_insert");
      return result;
    } catch (Throwable e) {
      db.execSQL("ROLLBACK TO SAVEPOINT srx_private_insert");
      db.execSQL("RELEASE SAVEPOINT srx_private_insert");
      pending.restore();
      throw e;
    }
  }

  // SQLite 回滚不会撤回已加入 Provider 内存队列的工作，只撤销本次插入新增的部分。
  private static final class PendingEvents {
    final android.util.SparseArray<android.util.ArraySet<android.net.Uri>> notifications;
    final android.util.SparseArray<android.util.ArraySet<android.net.Uri>> before =
        new android.util.SparseArray<>();
    final java.util.List<?> blocking, background;
    final int blockingCount, backgroundCount;

    @SuppressWarnings("unchecked")
    PendingEvents(Object state) {
      notifications =
          (android.util.SparseArray<android.util.ArraySet<android.net.Uri>>)
              Hooker.fieldValue(state, "notifyChanges");
      blocking = (java.util.List<?>) Hooker.fieldValue(state, "blockingTasks");
      background = (java.util.List<?>) Hooker.fieldValue(state, "backgroundTasks");
      if (notifications == null || blocking == null || background == null)
        throw new IllegalStateException("无法识别媒体事务的通知与任务队列");
      for (int i = 0; i < notifications.size(); i++)
        before.put(notifications.keyAt(i), new android.util.ArraySet<>(notifications.valueAt(i)));
      blockingCount = blocking.size();
      backgroundCount = background.size();
    }

    void restore() {
      notifications.clear();
      for (int i = 0; i < before.size(); i++) notifications.put(before.keyAt(i), before.valueAt(i));
      blocking.subList(blockingCount, blocking.size()).clear();
      background.subList(backgroundCount, background.size()).clear();
    }
  }

  void bind(SQLiteDatabase db, String uuid, long id, String owner, String logical) {
    try (SQLiteStatement statement =
        db.compileStatement(
            "INSERT INTO srx_private_media " + SELECT_BINDING + " AND _data=? COLLATE BINARY")) {
      statement.bindString(1, uuid);
      statement.bindLong(2, id);
      statement.bindString(3, owner);
      statement.bindString(4, logical);
      if (statement.executeInsert() != id) throw new IllegalStateException("私有媒体条目的身份已改变");
    }
  }
}
