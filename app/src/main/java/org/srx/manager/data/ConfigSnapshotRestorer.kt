package org.srx.manager.data

import android.content.Context
import kotlinx.serialization.json.Json
import org.srx.manager.root.isSafePackageName

/**
 * 配置快照恢复边界。
 *
 * 恢复必须在临时目录里先把整份配置拼好，再原子替换模块配置目录； 因此这里单独承载「编排临时目录 → 写入 → 提交 → 清理」的生命周期， 避免这段易错的流程与备份编码、配置读取混在一起。
 */
internal class ConfigSnapshotRestorer(
    private val context: Context,
    private val fileStore: RootFileStore,
    private val moduleController: RootModuleController,
    private val json: Json,
    private val invalidateConfiguredAppsCache: suspend () -> Unit,
) {
  suspend fun restore(data: BackupData): Boolean {
    val normalizedData = SrxConfigNormalizer.normalizeBackupData(data)
    val token = "${System.currentTimeMillis()}_${(0..99999).random()}"
    val stage = "/data/local/tmp/srx_restore_stage_$token"
    val rollback = "/data/local/tmp/srx_restore_rollback_$token"
    val stageApps = "$stage/apps"
    try {
      fileStore.removeTree(stage, rollback)
      if (!fileStore.prepareCleanDir(stageApps)) return false
      val stagedFiles = buildMap {
        put("global.json", json.encodeToString(normalizedData.global) + "\n")
        put(
            "templates.json",
            json.encodeToString(ConfigTemplateStore(normalizedData.templates)) + "\n",
        )
        put(
            "file_monitor_filters.json",
            json.encodeToString(normalizedData.monitorFilters) + "\n",
        )
        normalizedData.apps.filterKeys(::isSafePackageName).toSortedMap().forEach {
            (packageName, config) ->
          put("apps/$packageName.json", json.encodeToString(config) + "\n")
        }
      }
      if (!fileStore.writeStagedFiles(stage, stagedFiles)) return false
      val result = fileStore.restoreConfigStage(stage, rollback)
      if (result) {
        invalidateConfiguredAppsCache()
        normalizedData.ui?.let { PreferencesRepository(context).restoreBackupUiPreferences(it) }
        fileStore.touchConfig()
        moduleController.ensureLogCollectors()
      }
      return result
    } finally {
      fileStore.removeTree(stage, rollback)
    }
  }
}
