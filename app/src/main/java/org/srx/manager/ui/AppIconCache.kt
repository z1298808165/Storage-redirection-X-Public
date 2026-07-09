package org.srx.manager.ui

import android.content.Context
import android.content.pm.ApplicationInfo
import android.graphics.Bitmap
import android.os.Process
import android.util.LruCache
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext
import me.zhanghai.android.appiconloader.AppIconLoader

private fun ApplicationInfo.withCurrentUserUid(): ApplicationInfo {
  val myUserId = Process.myUid() / 100000
  val appId = uid % 100000
  val targetUid = myUserId * 100000 + appId
  if (uid == targetUid) return this
  return ApplicationInfo(this).apply { uid = targetUid }
}

object AppIconCache {
  private const val MinCacheKb = 4 * 1024
  private const val MaxCacheKb = 24 * 1024

  private val cacheSizeKb =
      (Runtime.getRuntime().maxMemory() / 1024 / 16).toInt().coerceIn(MinCacheKb, MaxCacheKb)
  private val cache =
      object : LruCache<String, Bitmap>(cacheSizeKb) {
        override fun sizeOf(key: String, value: Bitmap): Int = value.allocationByteCount / 1024
      }
  private val semaphore = Semaphore(4)

  fun get(info: ApplicationInfo): Bitmap? = synchronized(cache) { cache.get(key(info)) }

  suspend fun load(context: Context, info: ApplicationInfo, size: Int): Bitmap =
      semaphore.withPermit {
        synchronized(cache) { cache.get(key(info)) }
            ?.let {
              return@withPermit it
            }
        withContext(Dispatchers.IO) {
          val loader = AppIconLoader(size, false, context)
          val bitmap = loader.loadIcon(info.withCurrentUserUid())
          val prepared =
              runCatching { bitmap.copy(Bitmap.Config.HARDWARE, false)?.also { bitmap.recycle() } }
                  .getOrNull() ?: bitmap.also { it.prepareToDraw() }
          synchronized(cache) { cache.put(key(info), prepared) }
          prepared
        }
      }

  private fun key(info: ApplicationInfo): String =
      "${info.packageName}:${info.uid}:${info.sourceDir}"
}
