package org.srx.manager.data

/** 配置文件读写边界，集中处理未知字段保留和应用配置缓存失效。 */
internal class ConfigFileRepository(
    private val fileStore: RootFileStore,
    private val invalidateApps: suspend () -> Unit,
) {
  suspend fun read(path: String): String = fileStore.read(path)

  suspend fun write(path: String, content: String, touchAfter: Boolean = true): Boolean {
    val existing = fileStore.read(path)
    val merged = SrxConfigNormalizer.mergeUnknownTopLevelKeys(content, existing)
    val written = fileStore.write(path, merged, touchAfter)
    if (written && path.startsWith("$AppsDir/")) invalidateApps()
    return written
  }
}
