package org.srx.manager.ui

import android.content.Context
import android.content.pm.ApplicationInfo
import android.graphics.Bitmap
import android.os.Process
import android.util.LruCache
import java.util.concurrent.ConcurrentHashMap
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
      (Runtime.getRuntime().maxMemory() / 1024 / 8).toInt().coerceIn(MinCacheKb, MaxCacheKb)
  private val cache =
      object : LruCache<String, Bitmap>(cacheSizeKb) {
        override fun sizeOf(key: String, value: Bitmap): Int = value.allocationByteCount / 1024
      }
  private val semaphore = Semaphore(4)
  // AppIconLoader 构造时会准备缩放与形状资源，按尺寸复用可避免每个图标重复创建。
  private val loaders = ConcurrentHashMap<Int, AppIconLoader>()

  fun get(info: ApplicationInfo, size: Int): Bitmap? =
      synchronized(cache) { cache.get(key(info, size)) }

  suspend fun load(context: Context, info: ApplicationInfo, size: Int): Bitmap =
      semaphore.withPermit {
        // 排队等待期间可能已有同一图标完成加载，进入临界区后再查一次缓存。
        synchronized(cache) { cache.get(key(info, size)) }
            ?.let {
              return@withPermit it
            }
        withContext(Dispatchers.IO) {
          val loader =
              loaders.getOrPut(size) { AppIconLoader(size, false, context.applicationContext) }
          val bitmap = loader.loadIcon(info.withCurrentUserUid())
          val prepared =
              runCatching { bitmap.copy(Bitmap.Config.HARDWARE, false)?.also { bitmap.recycle() } }
                  .getOrNull() ?: bitmap.also { it.prepareToDraw() }
          synchronized(cache) { cache.put(key(info, size), prepared) }
          prepared
        }
      }

  private fun key(info: ApplicationInfo, size: Int): String =
      "${info.packageName}:${info.uid}:${info.sourceDir}:$size"
}
