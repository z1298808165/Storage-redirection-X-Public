package org.srx.manager.data

import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.srx.manager.root.RootShell
import org.srx.manager.root.isSafePackageName

data class MonitorLogSnapshot(
    val entries: List<LogEntry>,
    val filters: FileMonitorFilters,
)

class SrxRepository(
    private val context: Context,
    private val shell: RootShell,
) {
  private companion object {
    // 配置文件通过写入路径主动使缓存失效（invalidateConfiguredAppsCache），
    // TTL 仅作兜底；30 秒足够覆盖从 Dashboard 页导航到应用列表页的典型路径，
    // 避免连续两次 su 进程调用。
    const val AppDataCacheTtlNanos = 30_000_000_000L
    /** 解析应用名称的并发分片数：loadLabel 属于 IO 密集操作，过高并发只会加剧磁盘争用。 */
    const val LabelResolveParallelism = 4
  }

  private data class TimedCache<T>(val value: T, val loadedAtNanos: Long)

  private val json = Json {
    ignoreUnknownKeys = true
    prettyPrint = true
    encodeDefaults = true
    explicitNulls = false
  }
  private val fileStore = RootFileStore(shell)
  /** 配置文件边界：Repository 只负责业务组合，未知字段保留和缓存失效由此委托处理。 */
  private val configFiles = ConfigFileRepository(fileStore) { invalidateConfiguredAppsCache() }
  /** 诊断日志包导出边界：Repository 只保留业务入口。 */
  private val diagnosticExporter = DiagnosticArchiveExporter(context, shell, fileStore)
  private val moduleController = RootModuleController(shell)
  /** 配置快照恢复边界：临时目录编排与原子替换集中在这里。 */
  private val snapshotRestorer =
      ConfigSnapshotRestorer(context, fileStore, moduleController, json) {
        invalidateConfiguredAppsCache()
      }
  private val appQuery = RootAppQuery(shell)
  private val storageBrowser = RootStorageBrowser(shell)
  private val configuredAppsCacheMutex = Mutex()
  private val dexLabelsCacheMutex = Mutex()
  private var configuredAppsCache: TimedCache<Map<String, AppConfig>>? = null
  private val dexLabelsCache = mutableMapOf<String, TimedCache<Map<String, String>>>()
  /** 应用名称缓存：键包含安装包路径，应用升级或卸载重装后自动失效。 */
  private val appLabelCache = ConcurrentHashMap<String, String>()

  suspend fun checkRoot(): Boolean = shell.checkRoot()

  suspend fun readDashboard(): DashboardState = coroutineScope {
    val global = async { readGlobalConfig() }
    val statusAndVersion = async { moduleController.statusAndVersion() }
    val configs = async { readConfiguredAppConfigs(force = true) }
    val runtimeActivations = async { readRuntimeActivations() }
    val loadedConfigs = configs.await()
    val (status, version) = statusAndVersion.await()
    DashboardState(
        status = status,
        version = version,
        globalConfig = global.await(),
        enabledApps = countEnabledAppConfigs(loadedConfigs),
        runtimeActivations = runtimeActivations.await(),
    )
  }

  suspend fun readDashboardSummary(): DashboardState = coroutineScope {
    val global = async { readGlobalConfig() }
    // 状态与版本都来自模块目录，合并为一次 root 调用；每次 exec 都要新建 su 进程，
    // 是概览加载最贵的单项开销。
    val statusAndVersion = async { moduleController.statusAndVersion() }
    val (status, version) = statusAndVersion.await()
    DashboardState(
        status = status,
        version = version,
        globalConfig = global.await(),
    )
  }

  suspend fun readDashboardCounts(): Pair<Int, String> = coroutineScope {
    // 走缓存而非强制重读：所有写配置路径都会调用 invalidateConfiguredAppsCache，
    // 因此写入后必然重新加载；其余情况由缓存 TTL 兜住陈旧度。原先恒定 force 会让
    // 每次触发都重新 dump 并解析全部 apps/*.json。
    val configs = async { readConfiguredAppConfigs(force = false) }
    val runtimeActivations = async { readRuntimeActivations() }
    val enabledApps = countEnabledAppConfigs(configs.await())
    enabledApps to runtimeActivations.await()
  }

  suspend fun readGlobalConfig(): GlobalConfig {
    return when (
        val result = SrxConfigDecoder.decode<GlobalConfig>(json, readFile(GlobalConfigPath))
    ) {
      ConfigDecodeResult.Empty -> GlobalConfig()
      is ConfigDecodeResult.Success -> SrxConfigNormalizer.normalizeGlobalConfig(result.value)
      is ConfigDecodeResult.Invalid -> {
        logConfigDecodeFailure(GlobalConfigPath, result.reason)
        GlobalConfig()
      }
    }
  }

  suspend fun readFileMonitorFilters(): FileMonitorFilters {
    return when (
        val result =
            SrxConfigDecoder.decode<FileMonitorFilters>(
                json,
                readFile(FileMonitorFiltersConfigPath),
            )
    ) {
      ConfigDecodeResult.Empty -> FileMonitorFilters()
      is ConfigDecodeResult.Success -> SrxConfigNormalizer.normalizeFileMonitorFilters(result.value)
      is ConfigDecodeResult.Invalid -> {
        logConfigDecodeFailure(FileMonitorFiltersConfigPath, result.reason)
        FileMonitorFilters()
      }
    }
  }

  private fun logConfigDecodeFailure(path: String, reason: String) {
    android.util.Log.w("SrxRepository", "config_decode_failed path=$path reason=$reason")
  }

  suspend fun writeFileMonitorFilters(filters: FileMonitorFilters): Boolean {
    val normalized = SrxConfigNormalizer.normalizeFileMonitorFilters(filters)
    val ok =
        writeMergedConfigFile(
            FileMonitorFiltersConfigPath,
            json.encodeToString(normalized) + "\n",
        )
    if (ok) touchConfig()
    return ok
  }

  suspend fun writeGlobalConfig(config: GlobalConfig): Boolean {
    val ok =
        writeMergedConfigFile(
            GlobalConfigPath,
            json.encodeToString(SrxConfigNormalizer.normalizeGlobalConfig(config)) + "\n",
        )
    return ok
  }

  suspend fun readAppConfig(packageName: String): AppConfig? {
    if (!isSafePackageName(packageName)) return null
    val text = readFile("$AppsDir/$packageName.json")
    if (text.isBlank()) return null
    return runCatching {
          SrxConfigNormalizer.normalizeAppConfig(json.decodeFromString<AppConfig>(text))
        }
        .getOrNull()
  }

  suspend fun writeAppConfig(packageName: String, config: AppConfig): Boolean {
    if (!isSafePackageName(packageName)) return false
    return writeMergedConfigFile(
        "$AppsDir/$packageName.json",
        json.encodeToString(SrxConfigNormalizer.normalizeAppConfig(config)) + "\n",
    )
  }

  suspend fun writeAppConfigs(configs: Map<String, AppConfig>): Boolean {
    val safeConfigs =
        configs
            .filterKeys(::isSafePackageName)
            .mapValues { (_, config) -> SrxConfigNormalizer.normalizeAppConfig(config) }
            .toSortedMap()
    if (safeConfigs.isEmpty()) return false
    val token = "${System.currentTimeMillis()}_${(0..99999).random()}"
    val stage = "/data/local/tmp/srx_bulk_apps_$token"
    try {
      if (!fileStore.prepareCleanDir(stage)) return false
      val stagedFiles =
          safeConfigs
              .mapKeys { (packageName, _) -> "$packageName.json" }
              .mapValues { (_, config) -> json.encodeToString(config) + "\n" }
      if (!fileStore.writeStagedFiles(stage, stagedFiles)) return false
      val published = fileStore.publishStagedAppConfigs(stage)
      if (published) invalidateConfiguredAppsCache()
      return published
    } finally {
      fileStore.removeTree(stage)
    }
  }

  suspend fun deleteAppConfig(packageName: String): Boolean {
    if (!isSafePackageName(packageName)) return false
    val deleted = fileStore.deleteConfig("$AppsDir/$packageName.json")
    if (deleted) invalidateConfiguredAppsCache()
    return deleted
  }

  suspend fun readTemplates(): List<ConfigTemplate> {
    val text = readFile(TemplatesConfigPath)
    if (text.isBlank()) return emptyList()
    return runCatching {
          SrxConfigNormalizer.normalizeTemplateStore(
                  json.decodeFromString<ConfigTemplateStore>(text)
              )
              .templates
        }
        .getOrDefault(emptyList())
  }

  suspend fun writeTemplates(templates: List<ConfigTemplate>): Boolean {
    val store = SrxConfigNormalizer.normalizeTemplateStore(ConfigTemplateStore(templates))
    return writeFile(TemplatesConfigPath, json.encodeToString(store) + "\n", touchAfter = false)
  }

  suspend fun upsertTemplate(name: String, config: AppConfig, id: String? = null): Boolean {
    val cleanName = name.trim().take(48)
    if (cleanName.isBlank()) return false
    val templates = readTemplates().toMutableList()
    val templateId =
        id?.takeIf(SrxConfigNormalizer::isSafeTemplateId) ?: UUID.randomUUID().toString()
    val template =
        ConfigTemplate(
            id = templateId,
            name = cleanName,
            config = SrxConfigNormalizer.normalizeAppConfig(config),
        )
    val index = templates.indexOfFirst { it.id == templateId }
    if (index >= 0) templates[index] = template else templates += template
    return writeTemplates(templates)
  }

  suspend fun deleteTemplate(templateId: String): Boolean {
    if (!SrxConfigNormalizer.isSafeTemplateId(templateId)) return false
    if (readGlobalConfig().autoEnableNewAppsTemplateId == templateId) return false
    val templates = readTemplates().filterNot { it.id == templateId }
    return writeTemplates(templates)
  }

  suspend fun applyTemplateToApps(templateId: String, packageNames: Collection<String>): Boolean {
    if (!SrxConfigNormalizer.isSafeTemplateId(templateId)) return false
    val template = readTemplates().firstOrNull { it.id == templateId } ?: return false
    val targets = packageNames.filter(::isSafePackageName).distinct()
    if (targets.isEmpty()) return false
    return writeAppConfigs(targets.associateWith { template.config })
  }

  suspend fun loadInstalledApps(userId: String, force: Boolean = false): List<InstalledApp> =
      coroutineScope {
        val configs = async { readConfiguredAppConfigs(force) }
        val apps = async(Dispatchers.IO) { loadPackageManagerApps(userId) }
        val dexApps = async { loadDexAppLabels(userId, force) }
        val shellPackages = async { appQuery.listInstalledPackages(userId) }
        val configMap = configs.await()
        val pmApps = apps.await()
        val dexMap = dexApps.await()
        buildInstalledApps(configMap, pmApps, dexMap, shellPackages.await())
      }

  suspend fun loadInstalledAppsForPackages(
      packageNames: Set<String>,
      userId: String,
      force: Boolean = false,
  ): List<InstalledApp> = coroutineScope {
    val safePackages = packageNames.asSequence().filter(::isSafePackageName).distinct().toList()
    if (safePackages.isEmpty()) return@coroutineScope emptyList()

    val configs = async { readConfiguredAppConfigs(force) }
    val dexApps = async { if (userId == "0") emptyMap() else loadDexAppLabels(userId, force) }
    val configMap = configs.await()
    val dexMap = dexApps.await()

    withContext(Dispatchers.IO) {
      // 只需要少量包信息时逐个查询，避免为几个包枚举整机应用列表。
      safePackages.map { pkg ->
        val info = loadPackageManagerApp(pkg)
        InstalledApp(
            packageName = pkg,
            label = resolveAppLabel(pkg, info) ?: dexMap[pkg] ?: pkg,
            isSystem = info?.let { it.flags and ApplicationInfo.FLAG_SYSTEM != 0 } ?: false,
            appInfo = info,
            config = configMap[pkg],
            isInstalled = info != null,
        )
      }
    }
  }

  suspend fun listUsers(): List<String> = appQuery.listUsers()

  suspend fun moduleStatus(): ModuleStatus = moduleController.status()

  suspend fun setModuleEnabled(enabled: Boolean): Boolean = moduleController.setEnabled(enabled)

  suspend fun restartMediaProvider(): MediaProviderRestartResult =
      moduleController.restartMediaProvider()

  suspend fun readLogSnapshot(): MonitorLogSnapshot {
    val raw = fileStore.readAllWithBackups(FileMonitorLogPath)
    val filters = readFileMonitorFilters()
    val entries = withContext(Dispatchers.IO) { parseMonitorLogEntries(raw, filters) }
    return MonitorLogSnapshot(entries, filters)
  }

  suspend fun clearLogs(): Boolean {
    return fileStore.clearFileMonitorLog()
  }

  suspend fun resetRuntimeStats(): Boolean = fileStore.resetRuntimeStats()

  suspend fun exportDiagnosticArchive(
      uri: Uri,
      onProgress: ((DiagnosticArchiveProgress) -> Unit)? = null,
  ): Boolean = diagnosticExporter.exportToUri(uri, onProgress)

  suspend fun exportDiagnosticArchiveToDirectory(
      directoryUri: Uri,
      fileName: String,
      onProgress: ((DiagnosticArchiveProgress) -> Unit)? = null,
  ): Boolean = diagnosticExporter.exportToDirectory(directoryUri, fileName, onProgress)

  suspend fun createDiagnosticArchive(
      onProgress: ((DiagnosticArchiveProgress) -> Unit)? = null
  ): String? {
    return fileStore.createDiagnosticArchive(onProgress)
  }

  suspend fun buildBackupFileText(): String = coroutineScope {
    val appsDeferred = async { readConfiguredAppConfigs(force = true) }
    val globalDeferred = async { readGlobalConfig() }
    val templatesDeferred = async { readTemplates() }
    val monitorFiltersDeferred = async { readFileMonitorFilters() }
    val versionDeferred = async { moduleVersion() }
    val uiPreferencesDeferred = async { PreferencesRepository(context).readBackupUiPreferences() }
    val apps =
        withContext(Dispatchers.Default) {
          appsDeferred.await().filterKeys(::isSafePackageName).toSortedMap().mapValues { (_, config)
            ->
            SrxConfigNormalizer.normalizeAppConfig(config)
          }
        }
    val data =
        BackupData(
            global = globalDeferred.await(),
            apps = apps,
            templates = templatesDeferred.await(),
            monitorFilters = monitorFiltersDeferred.await(),
            ui = uiPreferencesDeferred.await(),
        )
    withContext(Dispatchers.Default) {
      BackupPayloadCodec.encode(json, data, versionDeferred.await())
    }
  }

  suspend fun buildBackupZipBytes(): ByteArray =
      withContext(Dispatchers.IO) { BackupArchiveCodec.encodeZip(buildBackupFileText()) }

  suspend fun restoreBackupFileText(text: String): Boolean {
    val data = withContext(Dispatchers.Default) { BackupPayloadCodec.decode(json, text) }
    return snapshotRestorer.restore(data)
  }

  suspend fun restoreBackupFileBytes(bytes: ByteArray): Boolean {
    return restoreBackupFileText(BackupArchiveCodec.decode(bytes))
  }

  suspend fun listStorageDirectories(userId: String, dirRel: String): List<String> =
      storageBrowser.listDirectories(userId, dirRel)

  private suspend fun readFile(path: String): String = configFiles.read(path)

  private suspend fun writeFile(
      path: String,
      content: String,
      touchAfter: Boolean = false,
  ): Boolean = fileStore.write(path, content, touchAfter)

  private suspend fun writeConfigFile(path: String, content: String): Boolean =
      configFiles.write(path, content, touchAfter = true)

  /**
   * 把序列化结果与磁盘上的原始 JSON 浅合并后再写回。
   *
   * 读取时 `ignoreUnknownKeys` 会丢弃 App 不认识的键，写入时又整文件重建， 因此模块或 WebUI 新增的配置字段会在用户改任意一个开关后被静默清空并回落默认值。
   * 这里保留原文件中 App 未覆盖的顶层键，只覆盖本次实际序列化出的键。
   *
   * 只做顶层浅合并：嵌套对象（如 `users`）由 App 完整建模并整体负责， 深合并反而会让用户删除的条目无法真正删除。
   */
  private suspend fun writeMergedConfigFile(path: String, content: String): Boolean =
      configFiles.write(path, content, touchAfter = true)

  private suspend fun touchConfig() {
    fileStore.touchConfig()
  }

  private suspend fun readConfiguredAppConfigs(force: Boolean): Map<String, AppConfig> {
    return configuredAppsCacheMutex.withLock {
      configuredAppsCache
          ?.takeUnless { force || it.isExpired() }
          ?.value
          ?.let {
            return@withLock it
          }
      val out = fileStore.readConfiguredAppConfigDump()
      val parsed =
          withContext(Dispatchers.Default) {
            parseConfiguredAppConfigDump(out, ConfiguredAppConfigMarker, json)
          }
      configuredAppsCache = TimedCache(parsed, System.nanoTime())
      parsed
    }
  }

  private suspend fun invalidateConfiguredAppsCache() {
    configuredAppsCacheMutex.withLock { configuredAppsCache = null }
  }

  private suspend fun countEnabledAppConfigs(configs: Map<String, AppConfig>): Int =
      withContext(Dispatchers.Default) {
        configs.count { (_, cfg) -> cfg.users.values.any { it.enabled } }
      }

  private suspend fun buildInstalledApps(
      configMap: Map<String, AppConfig>,
      pmApps: List<ApplicationInfo>,
      dexMap: Map<String, String>,
      shellPackages: List<String> = emptyList(),
  ): List<InstalledApp> =
      withContext(Dispatchers.IO) {
        val pmByPackage = pmApps.associateBy { it.packageName }
        val shellPackageSet = shellPackages.toSet()
        val allPackages =
            (pmByPackage.keys + dexMap.keys + shellPackages + configMap.keys + context.packageName)
                .filter(::isSafePackageName)
                .distinct()

        // loadLabel 需要打开 APK 读取资源，单线程串行解析整机应用会明显拖慢首屏，
        // 因此分片并发解析，并按安装包路径缓存名称，避免每次刷新重复解析。
        val chunkSize = ((allPackages.size + LabelResolveParallelism - 1) / LabelResolveParallelism)
        val built =
            if (chunkSize <= 0) {
              emptyList()
            } else {
              coroutineScope {
                    allPackages
                        .chunked(chunkSize)
                        .map { chunk ->
                          async {
                            chunk.map { pkg ->
                              val info = pmByPackage[pkg] ?: loadPackageManagerApp(pkg)
                              InstalledApp(
                                  packageName = pkg,
                                  label = resolveAppLabel(pkg, info) ?: dexMap[pkg] ?: pkg,
                                  isSystem =
                                      info?.let { it.flags and ApplicationInfo.FLAG_SYSTEM != 0 }
                                          ?: false,
                                  appInfo = info,
                                  config = configMap[pkg],
                                  isInstalled = info != null || pkg in shellPackageSet,
                              )
                            }
                          }
                        }
                        .awaitAll()
                  }
                  .flatten()
            }

        built.sortedWith(
            compareBy<InstalledApp> { statusRank(it) }
                .thenBy { it.searchLabel }
                .thenBy { it.packageName }
        )
      }

  /** 按安装包路径缓存应用名称：应用升级后 sourceDir 或版本会变化，缓存自然失效。 */
  private fun resolveAppLabel(packageName: String, info: ApplicationInfo?): String? {
    if (info == null) return null
    val key = "$packageName:${info.sourceDir}"
    appLabelCache[key]?.let {
      return it
    }
    val label = info.loadLabel(context.packageManager).toString().takeIf { it.isNotBlank() }
    if (label != null) appLabelCache[key] = label
    return label
  }

  private suspend fun loadDexAppLabels(userId: String, force: Boolean): Map<String, String> =
      dexLabelsCacheMutex.withLock {
        dexLabelsCache[userId]
            ?.takeUnless { force || it.isExpired() }
            ?.value
            ?.let {
              return@withLock it
            }
        val labels = appQuery.loadDexAppLabels(userId)
        dexLabelsCache[userId] = TimedCache(labels, System.nanoTime())
        labels
      }

  private fun TimedCache<*>.isExpired(): Boolean =
      System.nanoTime() - loadedAtNanos >= AppDataCacheTtlNanos

  private fun loadPackageManagerApps(userId: String): List<ApplicationInfo> {
    val pm = context.packageManager
    return try {
      if (Build.VERSION.SDK_INT >= 33) {
        pm.getInstalledApplications(PackageManager.ApplicationInfoFlags.of(0))
      } else {
        @Suppress("DEPRECATION") pm.getInstalledApplications(0)
      }
    } catch (_: Exception) {
      emptyList()
    }
  }

  private fun loadPackageManagerApp(packageName: String): ApplicationInfo? {
    if (!isSafePackageName(packageName)) return null
    val pm = context.packageManager
    return try {
      if (Build.VERSION.SDK_INT >= 33) {
        pm.getApplicationInfo(packageName, PackageManager.ApplicationInfoFlags.of(0))
      } else {
        @Suppress("DEPRECATION") pm.getApplicationInfo(packageName, 0)
      }
    } catch (_: Exception) {
      null
    }
  }

  private suspend fun moduleVersion(): String = moduleController.version()

  private suspend fun readRuntimeActivations(): String =
      parseRuntimeActivationCount(readFile(StatsPath))

  private fun statusRank(app: InstalledApp): Int =
      when {
        app.isEnabled -> 0
        app.isMissing -> 1
        app.isConfigured -> 2
        else -> 3
      }
}
