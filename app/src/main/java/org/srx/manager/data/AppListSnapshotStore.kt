package org.srx.manager.data

import android.content.Context
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.srx.manager.root.isSafePackageName
import org.srx.manager.root.isSafeUserId

/** 快照格式版本：字段语义变化时递增，旧快照会被直接丢弃而不是按新格式误读。 */
private const val AppListSnapshotVersion = 1

/**
 * 应用列表首屏快照。
 *
 * 只保存渲染一屏列表所需的字段：`ApplicationInfo` 依赖 PackageManager 句柄，不能序列化， 读回后由调用方补齐（图标缺失时先用占位图，真实刷新会在几百毫秒内替换）。
 */
@Serializable
internal data class AppListSnapshot(
    val version: Int = AppListSnapshotVersion,
    val user: String = "0",
    val savedAt: Long = 0L,
    val apps: List<Entry> = emptyList(),
) {
  @Serializable
  internal data class Entry(
      val packageName: String,
      val label: String,
      val system: Boolean = false,
      val installed: Boolean = true,
      val configured: Boolean = false,
      val enabled: Boolean = false,
  )
}

/** 快照的编解码与模型映射（纯函数部分，便于单测覆盖跨版本兼容行为）。 */
internal object AppListSnapshotCodec {
  private val json = Json {
    ignoreUnknownKeys = true
    encodeDefaults = true
    explicitNulls = false
  }

  fun encode(userId: String, apps: List<InstalledApp>, savedAt: Long): String =
      json.encodeToString(
          AppListSnapshot(
              user = userId,
              savedAt = savedAt,
              apps =
                  apps
                      .filter { isSafePackageName(it.packageName) }
                      .map { app ->
                        AppListSnapshot.Entry(
                            packageName = app.packageName,
                            label = app.label,
                            system = app.isSystem,
                            installed = app.isInstalled,
                            configured = app.isConfigured,
                            enabled = app.isEnabled,
                        )
                      },
          )
      )

  /** 解析失败、版本不符或用户不符一律返回 null：快照只是加速手段，读不到就走完整加载。 */
  fun decode(userId: String, text: String): List<InstalledApp>? {
    if (!isSafeUserId(userId)) return null
    val snapshot =
        runCatching { json.decodeFromString<AppListSnapshot>(text) }.getOrNull() ?: return null
    if (snapshot.version != AppListSnapshotVersion || snapshot.user != userId) return null
    return snapshot.apps.mapNotNull { entry ->
      if (!isSafePackageName(entry.packageName)) return@mapNotNull null
      InstalledApp(
          packageName = entry.packageName,
          label = entry.label.ifBlank { entry.packageName },
          isSystem = entry.system,
          appInfo = null,
          // 只重建「该用户有配置」这一信息，快照不承载任何配置内容，配置仍以文件为准。
          config =
              if (entry.configured) {
                AppConfig(users = mapOf(userId to UserProfile(enabled = entry.enabled)))
              } else {
                null
              },
          isInstalled = entry.installed,
          userId = userId,
      )
    }
  }
}

/**
 * 快照文件读写。
 *
 * 冷启动与非主用户的首次切换都要等一次 root 查询（后者还要拉起 dex 进程）才能出结果， 先渲染上一次的列表可以把「空列表 + 转圈」压到几十毫秒，随后真实加载完成再整体替换。
 */
internal class AppListSnapshotStore(private val context: Context) {
  private fun snapshotFile(userId: String) =
      File(File(context.cacheDir, AppListCacheDirName), "user_$userId.json")

  suspend fun read(userId: String): List<InstalledApp>? =
      withContext(Dispatchers.IO) {
        if (!isSafeUserId(userId)) return@withContext null
        val file = snapshotFile(userId)
        if (!file.isFile || file.length() <= 0L || file.length() > AppListSnapshotMaxBytes) {
          return@withContext null
        }
        val text = runCatching { file.readText() }.getOrNull() ?: return@withContext null
        AppListSnapshotCodec.decode(userId, text)
      }

  suspend fun write(userId: String, apps: List<InstalledApp>) =
      withContext(Dispatchers.IO) {
        if (!isSafeUserId(userId) || apps.isEmpty()) return@withContext
        val file = snapshotFile(userId)
        runCatching {
              file.parentFile?.mkdirs()
              val temp = File(file.parentFile, "${file.name}.tmp")
              temp.writeText(AppListSnapshotCodec.encode(userId, apps, System.currentTimeMillis()))
              if (!temp.renameTo(file)) {
                file.delete()
                temp.renameTo(file)
              }
            }
            .onFailure { android.util.Log.w("SrxRepository", "app_list_snapshot_write_failed") }
      }

  private companion object {
    const val AppListCacheDirName = "app_list"
    const val AppListSnapshotMaxBytes = 4L * 1024 * 1024
  }
}
