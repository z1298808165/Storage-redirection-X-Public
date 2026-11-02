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
import org.srx.manager.PerfLog
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
    /** 文件监视页首屏只读取最近记录，完整日志仍保留在诊断导出中。 */
    const val LogInitialLoadLines = 1000
  }

  private data class TimedCache<T>(val value: T, val loadedAtNanos: Long)

  private val json = Json {
    ignoreUnknownKeys = true
    prettyPrint = true
    encodeDefaults = true
    explicitNulls = false
  }
  private val fileStore = RootFileStore(shell)
  /** 应用列表首屏快照：仅在本地缓存目录，冷启动/切用户时先出内容，真实加载完成后覆盖。 */
  private val appListSnapshots = AppListSnapshotStore(context)
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
  private val dexAppListCacheMutex = Mutex()
  private var configuredAppsCache: TimedCache<Map<String, AppConfig>>? = null
  private val dexAppListCache = mutableMapOf<String, TimedCache<DexAppList>>()
  /** 应用名称缓存：键为包名，值含安装包路径，应用升级或卸载重装后自动失效。 */
  private val appLabelCache = ConcurrentHashMap<String, CachedAppLabel>()
  /** 名称磁盘缓存：冷启动直接读出上一次的名字，省掉整机 loadLabel 的 APK 资源解析。 */
  private val appLabelStore = AppLabelStore(context)
  private val appLabelCacheMutex = Mutex()
  @Volatile private var appLabelCacheLoaded = false
  @Volatile private var appLabelCacheDirty = false
  /** 按 APK 路径解析出的包信息：分身用户独有的应用只能从 APK 读标签与图标。 */
  private val archiveInfoCache = ConcurrentHashMap<String, ApplicationInfo>()

  suspend fun checkRoot(): Boolean = shell.checkRoot()

  suspend fun readDashboard(): DashboardState = coroutineScope {
    val global = async { readGlobalConfig() }
    val statusAndVersion = async { moduleController.statusAndVersion() }
    val configs = async { readConfiguredAppConfigs(force = true) }
    val runtimeActivations = async { readRuntimeActivations() }
    val storageBackend = async { readStorageBackendStatus() }
    val loadedConfigs = configs.await()
    val (status, version) = statusAndVersion.await()
    DashboardState(
        status = status,
        version = version,
        globalConfig = global.await(),
        enabledApps = countEnabledAppConfigs(loadedConfigs),
        runtimeActivations = runtimeActivations.await(),
        storageBackend = storageBackend.await(),
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

  /** 读取共享宿主会话状态指示；失败或输出不可识别时返回 null，界面按未知展示。 */
  suspend fun readStorageBackendStatus(): StorageBackendStatus? =
      moduleController.storageBackendStatus()

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
        val t0 = PerfLog.mark()
        val configs = async { readConfiguredAppConfigs(force) }
        val apps = async(Dispatchers.IO) { loadPackageManagerApps(userId) }
        val packages = async { appQuery.listUserPackages(userId) }
        val configMap = configs.await()
        val afterConfig = PerfLog.sinceMs(t0)
        val pmApps = apps.await()
        val afterPm = PerfLog.sinceMs(t0)
        val userPackages = packages.await()
        val afterShell = PerfLog.sinceMs(t0)
        // dex 只在 shell 的 pm 查询完全不可用时兜底：它要拉起一次 app_process（实测 1.3~1.9s），
        // 而成员、系统标记、APK 路径都能由 pm 查询拿到，标签与图标由本进程的 PackageManager 补齐。
        val dexAppList =
            if (userPackages.isEmpty) loadDexAppList(userId, force) else DexAppList.Empty
        PerfLog.log(
            "load user=$userId config=${afterConfig}ms pm=${afterPm}ms shell=${afterShell}ms " +
                "dex=${PerfLog.sinceMs(t0)}ms (pm=${pmApps.size} " +
                "shell=${userPackages.packageNames.size} sys=${userPackages.systemPackages.size} " +
                "dex=${dexAppList.labels.size})"
        )
        val tBuild = PerfLog.mark()
        // PackageManager 只枚举调用方用户：非主用户的应用清单必须由该用户的 root 通道决定，
        // 否则切到分身用户后仍会把主用户的应用全部列出来。两条通道都不可用时才退回枚举结果。
        val includePackageManagerMembers =
            userId == "0" || (userPackages.isEmpty && dexAppList.labels.isEmpty())
        buildInstalledApps(
                configMap,
                pmApps,
                dexAppList,
                userPackages,
                includePackageManagerMembers,
                userId,
            )
            .also {
              PerfLog.log(
                  "build user=$userId ${PerfLog.sinceMs(tBuild)}ms " +
                      "total=${PerfLog.sinceMs(t0)}ms rows=${it.size}"
              )
              appListSnapshots.write(userId, it)
            }
      }

  /**
   * 读取上一次成功加载的应用列表快照，用于首屏秒开。
   *
   * 快照不含 `ApplicationInfo`，这里用调用方 PackageManager 补齐：主用户视角能覆盖绝大多数应用， 分身用户独有的应用会短暂显示占位图标，随后由真实加载结果替换。
   */
  suspend fun readCachedAppList(userId: String): List<InstalledApp> {
    val cached = appListSnapshots.read(userId) ?: return emptyList()
    val infos =
        withContext(Dispatchers.IO) {
          runCatching { loadPackageManagerApps(userId).associateBy { it.packageName } }
              .getOrDefault(emptyMap())
        }
    if (infos.isEmpty()) return cached
    return cached.map { app -> infos[app.packageName]?.let { app.copy(appInfo = it) } ?: app }
  }

  suspend fun loadInstalledAppsForPackages(
      packageNames: Set<String>,
      userId: String,
      force: Boolean = false,
  ): List<InstalledApp> = coroutineScope {
    val safePackages = packageNames.asSequence().filter(::isSafePackageName).distinct().toList()
    if (safePackages.isEmpty()) return@coroutineScope emptyList()

    val configs = async { readConfiguredAppConfigs(force) }
    // 少数包的场景仍要走一次按用户的 pm 查询：非主用户独有的包不在调用方视角里，
    // 需要 APK 路径才能解析标签与图标（比拉起 dex 进程快一个数量级）。
    val packages = async { appQuery.listUserPackages(userId) }
    val configMap = configs.await()
    val userPackages = packages.await()

    withContext(Dispatchers.IO) {
      ensureAppLabelCacheLoaded()
      // 只需要少量包信息时逐个查询，避免为几个包枚举整机应用列表。
      val built =
          safePackages.map { pkg ->
            val info = loadPackageManagerApp(pkg) ?: loadArchiveAppInfo(userPackages.apkPaths[pkg])
            InstalledApp(
                packageName = pkg,
                label = resolveAppLabel(pkg, info) ?: pkg,
                isSystem = isSystemPackage(pkg, info, userPackages.systemPackages),
                appInfo = info,
                config = configMap[pkg],
                // 非主用户解析不到 PackageManager 记录是正常现象，是否安装以该用户的包清单为准。
                isInstalled = info != null || pkg in userPackages.packageNames,
                userId = userId,
            )
          }
      flushAppLabelCache()
      built
    }
  }

  suspend fun listUsers(): List<String> = appQuery.listUsers()

  suspend fun moduleStatus(): ModuleStatus = moduleController.status()

  suspend fun setModuleEnabled(enabled: Boolean): Boolean = moduleController.setEnabled(enabled)

  suspend fun restartMediaProvider(): MediaProviderRestartResult =
      moduleController.restartMediaProvider()

  suspend fun readLogSnapshot(): MonitorLogSnapshot {
    val raw = fileStore.readTailWithBackups(FileMonitorLogPath, LogInitialLoadLines)
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
  ): DiagnosticExportResult = diagnosticExporter.exportToUri(uri, onProgress)

  suspend fun exportDiagnosticArchiveToDirectory(
      directoryUri: Uri,
      fileName: String,
      onProgress: ((DiagnosticArchiveProgress) -> Unit)? = null,
  ): DiagnosticExportResult =
      diagnosticExporter.exportToDirectory(directoryUri, fileName, onProgress)

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
      val t0 = PerfLog.mark()
      val out = fileStore.readConfiguredAppConfigDump()
      val tShell = PerfLog.sinceMs(t0)
      val parsed =
          withContext(Dispatchers.Default) {
            parseConfiguredAppConfigDump(out, ConfiguredAppConfigMarker, json)
          }
      PerfLog.log(
          "config shell=${tShell}ms parse=${PerfLog.sinceMs(t0) - tShell}ms " +
              "bytes=${out.length} apps=${parsed.size}"
      )
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
      dexAppList: DexAppList,
      userPackages: UserPackageList = UserPackageList.Empty,
      includePackageManagerMembers: Boolean = true,
      userId: String = "0",
  ): List<InstalledApp> =
      withContext(Dispatchers.IO) {
        val pmByPackage = pmApps.associateBy { it.packageName }
        val userPackageSet = userPackages.packageNames.toSet()
        // 成员集合来自「该用户可用的通道」；主用户才允许用 PackageManager 枚举结果作为成员，
        // 非主用户仅用它补充标签与图标，避免把主用户的应用带进分身用户的列表。
        val packageManagerMembers =
            if (includePackageManagerMembers) pmByPackage.keys else emptySet()
        // 配置项同样按用户收敛：只在当前用户有该应用的配置档案时才纳入，
        // 否则分身用户会把主用户「已配置但本机未安装」的应用整批列出来。
        val configuredForUser = configMap.filterValues { it.users[userId] != null }.keys
        val allPackages =
            (packageManagerMembers +
                    dexAppList.labels.keys +
                    userPackages.packageNames +
                    configuredForUser)
                .filter(::isSafePackageName)
                .filter { it != context.packageName }
                .distinct()

        // loadLabel 需要打开 APK 读取资源，单线程串行解析整机应用会明显拖慢首屏，
        // 因此先读磁盘名称缓存（命中率接近全量），再对剩下的新应用分片并发解析。
        ensureAppLabelCacheLoaded()
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
                              // 非主用户独有的应用不在调用方视角里，退回按 APK 路径解析，
                              // 这样分身用户也能拿到正确标签与图标，而不是显示包名占位。
                              val info =
                                  pmByPackage[pkg]
                                      ?: loadPackageManagerApp(pkg)
                                      ?: loadArchiveAppInfo(userPackages.apkPaths[pkg])
                              InstalledApp(
                                  packageName = pkg,
                                  label = resolveAppLabel(pkg, info) ?: pkg,
                                  isSystem =
                                      isSystemPackage(pkg, info, userPackages.systemPackages),
                                  appInfo = info,
                                  config = configMap[pkg],
                                  isInstalled = info != null || pkg in userPackageSet,
                                  userId = userId,
                              )
                            }
                          }
                        }
                        .awaitAll()
                  }
                  .flatten()
            }

        flushAppLabelCache()
        built.sortedWith(
            compareBy<InstalledApp> { statusRank(it) }
                .thenBy { it.searchLabel }
                .thenBy { it.packageName }
        )
      }

  /**
   * 按 APK 路径解析包信息。
   *
   * 分身用户独有的应用在调用方视角里不存在（PackageManager 只覆盖调用方用户），但 APK 本身 是全局可读的；`getPackageArchiveInfo` 能读到标签、图标与
   * flags，避免为了几个包去拉起 dex 进程。
   */
  private fun loadArchiveAppInfo(apkPath: String?): ApplicationInfo? {
    if (apkPath.isNullOrBlank() || !apkPath.startsWith("/")) return null
    archiveInfoCache[apkPath]?.let {
      return it
    }
    val info =
        runCatching {
              context.packageManager.getPackageArchiveInfo(apkPath, 0)?.applicationInfo?.also {
                it.sourceDir = apkPath
                it.publicSourceDir = apkPath
              }
            }
            .getOrNull()
    if (info != null) archiveInfoCache[apkPath] = info
    return info
  }

  /** 按安装包路径缓存应用名称：应用升级后 sourceDir 或版本会变化，缓存自然失效。 */
  private fun resolveAppLabel(packageName: String, info: ApplicationInfo?): String? {
    if (info == null) return null
    val sourceDir = info.sourceDir.orEmpty()
    appLabelCache[packageName]
        ?.takeIf { it.sourceDir == sourceDir }
        ?.let {
          return it.label
        }
    val label = info.loadLabel(context.packageManager).toString().takeIf { it.isNotBlank() }
    if (label != null) {
      appLabelCache[packageName] = CachedAppLabel(sourceDir, label)
      appLabelCacheDirty = true
    }
    return label
  }

  /** 进程内第一次解析名称前读一次磁盘缓存：整机 loadLabel 是首屏最贵的一步。 */
  private suspend fun ensureAppLabelCacheLoaded() {
    if (appLabelCacheLoaded) return
    appLabelCacheMutex.withLock {
      if (appLabelCacheLoaded) return@withLock
      val cached = appLabelStore.read()
      cached.forEach { (pkg, entry) ->
        // 并发解析可能已经拿到更新的名字，不覆盖已有项。
        appLabelCache.putIfAbsent(pkg, entry)
      }
      appLabelCacheLoaded = true
    }
  }

  /** 本轮新解析出的名字落盘，下次冷启动就不用再逐个打开 APK。 */
  private suspend fun flushAppLabelCache() {
    if (!appLabelCacheDirty) return
    appLabelCacheDirty = false
    appLabelStore.write(appLabelCache.toMap())
  }

  /**
   * 系统应用判定以所选用户为准：`pm list packages -s --user` 给出该用户视角的系统应用集合， 只有拿不到该集合时才退回 PackageManager 的
   * flags（它只覆盖调用方用户，跨用户会误判）。
   */
  private fun isSystemPackage(
      packageName: String,
      info: ApplicationInfo?,
      systemPackages: Set<String>,
  ): Boolean {
    if (systemPackages.isNotEmpty()) return packageName in systemPackages
    val flags = info?.flags ?: return false
    // 与 `pm list packages -s` 一致：预装应用与「已更新的预装应用」都算系统应用。
    return flags and (ApplicationInfo.FLAG_SYSTEM or ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) != 0
  }

  private suspend fun loadDexAppList(userId: String, force: Boolean): DexAppList =
      dexAppListCacheMutex.withLock {
        dexAppListCache[userId]
            ?.takeUnless { force || it.isExpired() }
            ?.value
            ?.let {
              return@withLock it
            }
        val appList = appQuery.loadDexAppList(userId)
        dexAppListCache[userId] = TimedCache(appList, System.nanoTime())
        appList
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
    } catch (e: Exception) {
      android.util.Log.w(
          "SrxRepository",
          "load_installed_apps_failed user=$userId error=${e.message}",
      )
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
    } catch (e: Exception) {
      android.util.Log.w(
          "SrxRepository",
          "load_app_info_failed pkg=$packageName error=${e.message}",
      )
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
