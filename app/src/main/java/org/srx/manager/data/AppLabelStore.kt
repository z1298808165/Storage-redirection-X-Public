package org.srx.manager.data

import android.content.Context
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.srx.manager.root.isSafePackageName

/** 标签缓存格式版本：字段语义变化时递增，旧缓存直接丢弃而不是按新格式误读。 */
private const val AppLabelCacheVersion = 1

/** 缓存超过这个时长就整体视为过期，避免应用改名或换语言后长期显示旧名称。 */
private const val AppLabelCacheMaxAgeMs = 30L * 24 * 60 * 60 * 1000

/** 单个应用标签的缓存项：APK 路径变化（应用更新）即视为失效。 */
internal data class CachedAppLabel(
    val sourceDir: String,
    val label: String,
)

/**
 * 应用名称磁盘缓存。
 *
 * `ApplicationInfo.loadLabel` 要打开每个 APK 解析 resources.arsc，整机 470 个应用一次要 1s 左右， 而名称在应用更新（APK
 * 路径变化）之外几乎不变。把它落到应用私有缓存后，冷启动只剩读一次 JSON， 首屏从「先等 1s 解析」变成「直接有名称」。
 */
internal object AppLabelCacheCodec {
  private val json = Json {
    ignoreUnknownKeys = true
    encodeDefaults = true
    explicitNulls = false
  }

  @Serializable
  private data class Snapshot(
      val version: Int = AppLabelCacheVersion,
      val savedAt: Long = 0L,
      val entries: Map<String, Entry> = emptyMap(),
  )

  @Serializable private data class Entry(val dir: String = "", val label: String = "")

  fun encode(labels: Map<String, CachedAppLabel>, savedAt: Long): String =
      json.encodeToString(
          Snapshot(
              savedAt = savedAt,
              entries =
                  labels
                      .filterKeys(::isSafePackageName)
                      .filterValues { it.label.isNotBlank() }
                      .mapValues { (_, value) ->
                        Entry(dir = value.sourceDir, label = value.label)
                      },
          )
      )

  /** 版本不符、结构损坏或已过期一律返回空表：缓存只是加速手段，读不到就重新解析。 */
  fun decode(text: String, now: Long): Map<String, CachedAppLabel> {
    val snapshot =
        runCatching { json.decodeFromString<Snapshot>(text) }.getOrNull() ?: return emptyMap()
    if (snapshot.version != AppLabelCacheVersion) return emptyMap()
    if (snapshot.savedAt <= 0L || now - snapshot.savedAt > AppLabelCacheMaxAgeMs) return emptyMap()
    return snapshot.entries
        .mapNotNull { (pkg, entry) ->
          val label = entry.label.takeIf { it.isNotBlank() } ?: return@mapNotNull null
          if (!isSafePackageName(pkg)) return@mapNotNull null
          pkg to CachedAppLabel(entry.dir, label)
        }
        .toMap()
  }
}

/** 标签缓存的读写；写失败不影响功能，下一次加载会重新解析。 */
internal class AppLabelStore(private val context: Context) {
  private fun cacheFile() = File(File(context.cacheDir, CacheDirName), FileName)

  suspend fun read(): Map<String, CachedAppLabel> =
      withContext(Dispatchers.IO) {
        val file = cacheFile()
        if (!file.isFile || file.length() <= 0L || file.length() > MaxBytes)
            return@withContext emptyMap()
        val text = runCatching { file.readText() }.getOrNull() ?: return@withContext emptyMap()
        AppLabelCacheCodec.decode(text, System.currentTimeMillis())
      }

  suspend fun write(labels: Map<String, CachedAppLabel>) {
    if (labels.isEmpty()) return
    withContext(Dispatchers.IO) {
      runCatching {
            val file = cacheFile()
            file.parentFile?.mkdirs()
            val temp = File(file.parentFile, "$FileName.tmp")
            temp.writeText(AppLabelCacheCodec.encode(labels, System.currentTimeMillis()))
            if (!temp.renameTo(file)) {
              file.delete()
              temp.renameTo(file)
            }
          }
          .onFailure { android.util.Log.w("SrxRepository", "app_label_cache_write_failed") }
    }
  }

  private companion object {
    const val CacheDirName = "app_labels"
    const val FileName = "labels.json"
    const val MaxBytes = 8L * 1024 * 1024
  }
}
