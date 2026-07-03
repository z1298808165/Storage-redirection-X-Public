package org.srx.manager.data

import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.provider.DocumentsContract
import android.net.Uri
import android.os.Build
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.withContext
import kotlinx.serialization.SerializationException
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import java.time.Instant
import java.util.UUID
import org.srx.manager.root.RootShell
import org.srx.manager.root.isSafePackageName
import org.srx.manager.root.shellQuote

class SrxRepository(
    private val context: Context,
    private val shell: RootShell,
) {
    private companion object {
        const val BackupMagic = "storage.redirect.x.backup"
        const val BackupSchemaVersion = 2
        const val BackupModuleId = "storage.redirect.x"
        const val LogPreviewTailLines = 500
    }

    private val json = Json {
        ignoreUnknownKeys = true
        prettyPrint = true
        encodeDefaults = true
        explicitNulls = false
    }
    private val fileStore = RootFileStore(shell)
    private val moduleController = RootModuleController(shell)
    private val appQuery = RootAppQuery(shell)
    private val storageBrowser = RootStorageBrowser(shell)

    suspend fun checkRoot(): Boolean = shell.checkRoot()

    suspend fun readDashboard(): DashboardState = coroutineScope {
        val global = async { readGlobalConfig() }
        val status = async { moduleStatus() }
        val version = async { moduleVersion() }
        val configs = async { readConfiguredAppConfigs(force = true) }
        val events = async { readEffectiveEvents() }
        val loadedConfigs = configs.await()
        DashboardState(
            status = status.await(),
            version = version.await(),
            globalConfig = global.await(),
            enabledApps = countEnabledAppConfigs(loadedConfigs),
            effectiveEvents = events.await(),
        )
    }

    suspend fun readDashboardSummary(): DashboardState = coroutineScope {
        val global = async { readGlobalConfig() }
        val status = async { moduleStatus() }
        val version = async { moduleVersion() }
        DashboardState(
            status = status.await(),
            version = version.await(),
            globalConfig = global.await(),
        )
    }

    suspend fun readDashboardCounts(): Pair<Int, Int> = coroutineScope {
        val configs = async { readConfiguredAppConfigs(force = true) }
        val events = async { readEffectiveEvents() }
        val enabledApps = countEnabledAppConfigs(configs.await())
        enabledApps to events.await()
    }

    suspend fun readGlobalConfig(): GlobalConfig {
        val text = readFile(GlobalConfigPath)
        if (text.isBlank()) return GlobalConfig()
        return SrxConfigNormalizer.normalizeGlobalConfig(
            runCatching { json.decodeFromString<GlobalConfig>(text) }.getOrDefault(GlobalConfig()),
        )
    }

    suspend fun readFileMonitorFilters(): FileMonitorFilters {
        val text = readFile(FileMonitorFiltersConfigPath)
        if (text.isBlank()) return FileMonitorFilters()
        return SrxConfigNormalizer.normalizeFileMonitorFilters(
            runCatching { json.decodeFromString<FileMonitorFilters>(text) }.getOrDefault(FileMonitorFilters()),
        )
    }

    suspend fun writeFileMonitorFilters(filters: FileMonitorFilters): Boolean {
        val normalized = SrxConfigNormalizer.normalizeFileMonitorFilters(filters)
        val ok = writeConfigFile(FileMonitorFiltersConfigPath, json.encodeToString(normalized) + "\n")
        if (ok) touchConfig()
        return ok
    }

    suspend fun writeGlobalConfig(config: GlobalConfig): Boolean {
        val ok = writeConfigFile(GlobalConfigPath, json.encodeToString(SrxConfigNormalizer.normalizeGlobalConfig(config)) + "\n")
        return ok
    }

    suspend fun readAppConfig(packageName: String): AppConfig? {
        if (!isSafePackageName(packageName)) return null
        val text = readFile("$AppsDir/$packageName.json")
        if (text.isBlank()) return null
        return runCatching { SrxConfigNormalizer.normalizeAppConfig(json.decodeFromString<AppConfig>(text)) }.getOrNull()
    }

    suspend fun writeAppConfig(packageName: String, config: AppConfig): Boolean {
        if (!isSafePackageName(packageName)) return false
        return writeConfigFile("$AppsDir/$packageName.json", json.encodeToString(SrxConfigNormalizer.normalizeAppConfig(config)) + "\n")
    }

    suspend fun writeAppConfigs(configs: Map<String, AppConfig>): Boolean {
        val safeConfigs = configs
            .filterKeys(::isSafePackageName)
            .mapValues { (_, config) -> SrxConfigNormalizer.normalizeAppConfig(config) }
            .toSortedMap()
        if (safeConfigs.isEmpty()) return false
        val token = "${System.currentTimeMillis()}_${(0..99999).random()}"
        val stage = "/data/local/tmp/srx_bulk_apps_$token"
        try {
            if (!fileStore.prepareCleanDir(stage)) return false
            safeConfigs.forEach { (packageName, config) ->
                if (!writeFile("$stage/$packageName.json", json.encodeToString(config) + "\n", touchAfter = false)) return false
            }
            return fileStore.publishStagedAppConfigs(stage)
        } finally {
            fileStore.removeTree(stage)
        }
    }

    suspend fun deleteAppConfig(packageName: String): Boolean {
        if (!isSafePackageName(packageName)) return false
        return fileStore.deleteConfig("$AppsDir/$packageName.json")
    }

    suspend fun readTemplates(): List<ConfigTemplate> {
        val text = readFile(TemplatesConfigPath)
        if (text.isBlank()) return emptyList()
        return runCatching { SrxConfigNormalizer.normalizeTemplateStore(json.decodeFromString<ConfigTemplateStore>(text)).templates }
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
        val templateId = id?.takeIf(SrxConfigNormalizer::isSafeTemplateId) ?: UUID.randomUUID().toString()
        val template = ConfigTemplate(
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

    suspend fun loadInstalledApps(userId: String, force: Boolean = false): List<InstalledApp> = coroutineScope {
        val configs = async { readConfiguredAppConfigs(force) }
        val apps = async(Dispatchers.IO) { loadPackageManagerApps(userId) }
        val dexApps = async { if (userId == "0") emptyMap() else loadDexAppLabels(userId, force) }
        val configMap = configs.await()
        val pmApps = apps.await()
        val dexMap = dexApps.await()
        buildInstalledApps(configMap, pmApps, dexMap)
    }

    suspend fun loadInstalledAppsForPackages(
        packageNames: Set<String>,
        userId: String,
        force: Boolean = false,
    ): List<InstalledApp> = coroutineScope {
        val safePackages = packageNames
            .asSequence()
            .filter(::isSafePackageName)
            .distinct()
            .toList()
        if (safePackages.isEmpty()) return@coroutineScope emptyList()

        val configs = async { readConfiguredAppConfigs(force) }
        val dexApps = async { if (userId == "0") emptyMap() else loadDexAppLabels(userId, force) }
        val configMap = configs.await()
        val dexMap = dexApps.await()

        withContext(Dispatchers.IO) {
            safePackages.map { pkg ->
                val info = loadPackageManagerApp(pkg)
                InstalledApp(
                    packageName = pkg,
                    label = info?.loadLabel(context.packageManager)?.toString()?.takeIf { it.isNotBlank() } ?: dexMap[pkg] ?: pkg,
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

    suspend fun restartMediaProvider(): Boolean = moduleController.restartMediaProvider()

    suspend fun readLogs(): List<LogEntry> {
        val raw = fileStore.readTail(FileMonitorLogPath, LogPreviewTailLines)
        return withContext(Dispatchers.IO) { parseMonitorLogEntries(raw, ::resolveLogPackageLabel) }
    }

    suspend fun clearLogs(): Boolean {
        return fileStore.clearFileMonitorLog()
    }

    suspend fun exportDiagnosticArchive(
        uri: Uri,
        onProgress: (suspend (DiagnosticArchiveProgress) -> Unit)? = null,
    ): Boolean = withContext(Dispatchers.IO) {
        val archivePath = createDiagnosticArchive(onProgress) ?: return@withContext false
        try {
            onProgress?.invoke(DiagnosticArchiveProgress(99, "copy", "正在写入目标文件"))
            val ok = copyRootFileToUri(archivePath, uri)
            if (ok) onProgress?.invoke(DiagnosticArchiveProgress(100, "done", "日志包已保存"))
            ok
        } finally {
            fileStore.removeFile(archivePath)
        }
    }

    suspend fun exportDiagnosticArchiveToDirectory(
        directoryUri: Uri,
        fileName: String,
        onProgress: (suspend (DiagnosticArchiveProgress) -> Unit)? = null,
    ): Boolean = withContext(Dispatchers.IO) {
        val safeName = fileName.replace(Regex("[\\\\/:*?\"<>|\\u0000-\\u001f]"), "_").ifBlank { "storage-redirect-x-logs.tar.gz" }
        val monitorTargetPath = resolvePrimaryTreePublicStorageDirectory(directoryUri)?.let { joinPath(it, safeName) }
        val archivePath = createDiagnosticArchive(onProgress) ?: return@withContext false
        try {
            onProgress?.invoke(DiagnosticArchiveProgress(99, "copy", "正在写入目标文件"))
            val ok = exportDiagnosticArchiveToDocumentTree(archivePath, directoryUri, safeName) ||
                copyDiagnosticArchiveToPrimaryTree(archivePath, directoryUri, safeName)
            if (ok) {
                onProgress?.invoke(DiagnosticArchiveProgress(100, "done", "日志包已保存"))
                recordAppExportMonitor(monitorTargetPath, "diagnostic")
            }
            ok
        } finally {
            fileStore.removeFile(archivePath)
        }
    }

    private fun exportDiagnosticArchiveToDocumentTree(archivePath: String, directoryUri: Uri, fileName: String): Boolean {
        val resolver = context.contentResolver
        val treeDocumentId = runCatching { DocumentsContract.getTreeDocumentId(directoryUri) }.getOrNull()
            ?: return false
        val parentUri = DocumentsContract.buildDocumentUriUsingTree(directoryUri, treeDocumentId)
        val fileUri = runCatching {
            DocumentsContract.createDocument(resolver, parentUri, "application/gzip", fileName)
        }.getOrNull() ?: return false
        val ok = copyRootFileToUri(archivePath, fileUri)
        if (!ok) runCatching { DocumentsContract.deleteDocument(resolver, fileUri) }
        return ok
    }

    private fun copyRootFileToUri(archivePath: String, uri: Uri): Boolean {
        val proc = try {
            ProcessBuilder("su", "-c", "cat ${shellQuote(archivePath)}")
                .redirectErrorStream(false)
                .start()
        } catch (_: Exception) {
            return false
        }
        return try {
            context.contentResolver.openOutputStream(uri, "w")?.use { output ->
                proc.inputStream.use { input -> input.copyTo(output) }
            } ?: return false
            proc.waitFor() == 0
        } catch (_: Exception) {
            false
        } finally {
            runCatching { proc.destroy() }
        }
    }

    private suspend fun recordAppExportMonitor(targetPath: String?, kind: String) {
        val path = targetPath?.takeIf { it.isNotBlank() } ?: return
        val packageName = context.packageName.takeIf { isSafePackageName(it) } ?: "org.srx.manager"
        val exportKind = when (kind.lowercase()) {
            "backup" -> "backup"
            else -> "diagnostic"
        }
        val source = if (exportKind == "backup") "app_backup" else "app_export"
        val command = "mkdir -p ${shellQuote(LogsDir)} && " +
            "ts=\$(date '+%Y-%m-%d %H:%M:%S' 2>/dev/null || toybox date '+%Y-%m-%d %H:%M:%S' 2>/dev/null); " +
            "printf '%s|%s|%s|OPEN|%s|ret=0|errno=0|identify_method=caller|identify_reliability=high|op=provider_open|op_filter=provider_open:write|source=%s|export_kind=%s\\n' " +
            "\"\${ts:-unknown}\" ${shellQuote(packageName)} ${shellQuote(packageName)} ${shellQuote(path)} ${shellQuote(source)} ${shellQuote(exportKind)} >> ${shellQuote(FileMonitorLogPath)}; " +
            "chmod 666 ${shellQuote(FileMonitorLogPath)} 2>/dev/null || true"
        runCatching { shell.exec(command, timeoutMs = 10_000L) }
    }

    private suspend fun copyDiagnosticArchiveToPrimaryTree(archivePath: String, directoryUri: Uri, fileName: String): Boolean {
        val directoryPath = resolvePrimaryTreeDataMediaDirectory(directoryUri) ?: return false
        val targetPath = joinPath(directoryPath, fileName)
        val command = "dir=${shellQuote(directoryPath)}; " +
            "target=${shellQuote(targetPath)}; " +
            "archive=${shellQuote(archivePath)}; " +
            "mkdir -p \"\$dir\" || exit 1; " +
            "chown 1023:1023 \"\$dir\" 2>/dev/null || true; " +
            "chmod 2775 \"\$dir\" 2>/dev/null || true; " +
            "cat \"\$archive\" > \"\$target\"; " +
            "rc=\$?; " +
            "if [ \$rc -eq 0 ]; then chown 1023:1023 \"\$target\" 2>/dev/null || true; chmod 0644 \"\$target\" 2>/dev/null || true; [ -s \"\$target\" ] || rc=1; fi; " +
            "exit \$rc"
        return shell.exec(command).isSuccess
    }

    private fun resolvePrimaryTreeDataMediaDirectory(directoryUri: Uri): String? {
        val tree = resolvePrimaryTreePath(directoryUri) ?: return null
        return buildPrimaryTreePath("/data/media/${tree.userId}", tree.segments)
    }

    private fun resolvePrimaryTreePublicStorageDirectory(directoryUri: Uri): String? {
        val tree = resolvePrimaryTreePath(directoryUri) ?: return null
        return buildPrimaryTreePath("/storage/emulated/${tree.userId}", tree.segments)
    }

    private fun resolvePrimaryTreePath(directoryUri: Uri): PrimaryTreePath? {
        val documentId = runCatching { DocumentsContract.getTreeDocumentId(directoryUri) }.getOrNull()
            ?: return null
        if (!documentId.startsWith("primary:")) return null
        val relativePath = documentId.removePrefix("primary:").trim('/')
        val segments = if (relativePath.isEmpty()) {
            emptyList()
        } else {
            relativePath.split('/').filter { it.isNotEmpty() }
        }
        if (segments.any { it == "." || it == ".." || it.indexOf('\u0000') >= 0 }) return null
        return PrimaryTreePath(android.os.Process.myUid() / 100000, segments)
    }

    private fun buildPrimaryTreePath(root: String, segments: List<String>): String {
        return buildString {
            append(root)
            for (segment in segments) {
                append('/')
                append(segment)
            }
        }
    }

    private fun joinPath(directoryPath: String, fileName: String): String {
        return if (directoryPath.endsWith('/')) "$directoryPath$fileName" else "$directoryPath/$fileName"
    }

    private data class PrimaryTreePath(
        val userId: Int,
        val segments: List<String>,
    )

    suspend fun createDiagnosticArchive(onProgress: (suspend (DiagnosticArchiveProgress) -> Unit)? = null): String? {
        return fileStore.createDiagnosticArchive(onProgress)
    }

    suspend fun buildBackupFileText(): String = coroutineScope {
        val appsDeferred = async { readConfiguredAppConfigs(force = true) }
        val globalDeferred = async { readGlobalConfig() }
        val templatesDeferred = async { readTemplates() }
        val monitorFiltersDeferred = async { readFileMonitorFilters() }
        val versionDeferred = async { moduleVersion() }
        val uiPreferencesDeferred = async { PreferencesRepository(context).readBackupUiPreferences() }
        val apps = withContext(Dispatchers.Default) {
            appsDeferred.await()
                .filterKeys(::isSafePackageName)
                .toSortedMap()
                .mapValues { (_, config) -> SrxConfigNormalizer.normalizeAppConfig(config) }
        }
        val data = BackupData(
            global = globalDeferred.await(),
            apps = apps,
            templates = templatesDeferred.await(),
            monitorFilters = monitorFiltersDeferred.await(),
            ui = uiPreferencesDeferred.await(),
        )
        withContext(Dispatchers.Default) {
            val canonical = SrxConfigNormalizer.stableJson(json, data)
            val payload = BackupPayload(
                magic = BackupMagic,
                schema = BackupSchemaVersion,
                module = BackupModuleInfo(id = BackupModuleId, version = versionDeferred.await()),
                createdAt = Instant.now().toString(),
                summary = BackupSummary(
                    appCount = apps.size,
                    userCount = apps.values.sumOf { it.users.size },
                ),
                integrity = BackupIntegrity(
                    algorithm = "SHA-256",
                    value = SrxConfigNormalizer.sha256Hex(canonical),
                ),
                data = data,
            )
            json.encodeToString(payload) + "\n"
        }
    }

    suspend fun buildBackupZipBytes(): ByteArray = withContext(Dispatchers.IO) {
        BackupArchiveCodec.encodeZip(buildBackupFileText())
    }

    suspend fun restoreBackupFileText(text: String): Boolean {
        val data = parseBackupPayload(text)
        return restoreConfigSnapshot(data)
    }

    suspend fun restoreBackupFileBytes(bytes: ByteArray): Boolean {
        return restoreBackupFileText(BackupArchiveCodec.decode(bytes))
    }

    suspend fun listStorageDirectories(userId: String, dirRel: String): List<String> =
        storageBrowser.listDirectories(userId, dirRel)

    private suspend fun readFile(path: String): String =
        fileStore.read(path)

    private suspend fun writeFile(path: String, content: String, touchAfter: Boolean = false): Boolean =
        fileStore.write(path, content, touchAfter)

    private suspend fun writeConfigFile(path: String, content: String): Boolean =
        fileStore.writeConfig(path, content)

    private suspend fun touchConfig() {
        fileStore.touchConfig()
    }

    private suspend fun restoreConfigSnapshot(data: BackupData): Boolean {
        val normalizedData = SrxConfigNormalizer.normalizeBackupData(data)
        val token = "${System.currentTimeMillis()}_${(0..99999).random()}"
        val stage = "/data/local/tmp/srx_restore_stage_$token"
        val rollback = "/data/local/tmp/srx_restore_rollback_$token"
        val stageApps = "$stage/apps"
        try {
            fileStore.removeTree(stage, rollback)
            if (!fileStore.prepareCleanDir(stageApps)) return false
            if (!writeFile("$stage/global.json", json.encodeToString(normalizedData.global) + "\n")) return false
            if (!writeFile("$stage/templates.json", json.encodeToString(ConfigTemplateStore(normalizedData.templates)) + "\n")) return false
            if (!writeFile("$stage/file_monitor_filters.json", json.encodeToString(normalizedData.monitorFilters) + "\n")) return false
            normalizedData.apps
                .filterKeys(::isSafePackageName)
                .toSortedMap()
                .forEach { (packageName, config) ->
                    if (!writeFile("$stageApps/$packageName.json", json.encodeToString(config) + "\n")) return false
                }
            val result = fileStore.restoreConfigStage(stage, rollback)
            if (result) {
                normalizedData.ui?.let { PreferencesRepository(context).restoreBackupUiPreferences(it) }
                touchConfig()
                moduleController.ensureLogCollectors()
            }
            return result
        } finally {
            fileStore.removeTree(stage, rollback)
        }
    }

    private suspend fun readConfiguredAppConfigs(force: Boolean): Map<String, AppConfig> {
        val out = fileStore.readConfiguredAppConfigDump()
        return withContext(Dispatchers.Default) {
            parseConfiguredAppConfigDump(out, ConfiguredAppConfigMarker, json)
        }
    }

    private suspend fun countEnabledAppConfigs(configs: Map<String, AppConfig>): Int = withContext(Dispatchers.Default) {
        configs.count { (_, cfg) -> cfg.users.values.any { it.enabled } }
    }

    private suspend fun buildInstalledApps(
        configMap: Map<String, AppConfig>,
        pmApps: List<ApplicationInfo>,
        dexMap: Map<String, String>,
    ): List<InstalledApp> = withContext(Dispatchers.IO) {
        val pmByPackage = pmApps.associateBy { it.packageName }
        val allPackages = (pmByPackage.keys + dexMap.keys + configMap.keys + context.packageName)
            .filter(::isSafePackageName)
            .distinct()

        allPackages.map { pkg ->
            val info = pmByPackage[pkg] ?: loadPackageManagerApp(pkg)
            InstalledApp(
                packageName = pkg,
                label = info?.loadLabel(context.packageManager)?.toString()?.takeIf { it.isNotBlank() } ?: dexMap[pkg] ?: pkg,
                isSystem = info?.let { it.flags and ApplicationInfo.FLAG_SYSTEM != 0 } ?: false,
                appInfo = info,
                config = configMap[pkg],
                isInstalled = info != null,
            )
        }.sortedWith(compareBy<InstalledApp> { statusRank(it) }.thenBy { it.label.lowercase() }.thenBy { it.packageName })
    }

    private suspend fun parseBackupPayload(text: String): BackupData = withContext(Dispatchers.Default) {
        if (text.toByteArray(Charsets.UTF_8).size > BackupMaxBytes) {
            throw IllegalArgumentException("备份文件过大")
        }
        val payload = try {
            json.decodeFromString<BackupPayload>(text)
        } catch (_: SerializationException) {
            throw IllegalArgumentException("备份文件不是有效 JSON")
        } catch (_: IllegalArgumentException) {
            throw IllegalArgumentException("备份文件不是有效 JSON")
        }
        if (payload.magic != BackupMagic) throw IllegalArgumentException("不是 Storage Redirect X 备份")
        if (payload.schema !in 1..BackupSchemaVersion) throw IllegalArgumentException("备份格式版本不支持")
        if (payload.module.id != BackupModuleId) throw IllegalArgumentException("备份属于其它模块")
        val data = SrxConfigNormalizer.normalizeBackupData(payload.data)
        val expected = SrxConfigNormalizer.backupDigestCandidates(json, data)
        if (!payload.integrity.algorithm.equals("SHA-256", ignoreCase = true) || payload.integrity.value !in expected) {
            throw IllegalArgumentException("备份校验失败，文件可能被改动")
        }
        data
    }

    private suspend fun loadDexAppLabels(userId: String, force: Boolean): Map<String, String> =
        appQuery.loadDexAppLabels(userId)

    private fun loadPackageManagerApps(userId: String): List<ApplicationInfo> {
        val pm = context.packageManager
        return try {
            if (Build.VERSION.SDK_INT >= 33) {
                pm.getInstalledApplications(PackageManager.ApplicationInfoFlags.of(0))
            } else {
                @Suppress("DEPRECATION")
                pm.getInstalledApplications(0)
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
                @Suppress("DEPRECATION")
                pm.getApplicationInfo(packageName, 0)
            }
        } catch (_: Exception) {
            null
        }
    }

    private suspend fun moduleVersion(): String =
        moduleController.version()

    private suspend fun readEffectiveEvents(): Int {
        val stats = readFile(StatsPath).trim().toIntOrNull()
        if (stats != null && stats >= 0) return stats
        return parseMonitorLogEntries(readFile(FileMonitorLogPath)).count { it.ok }
    }

    private fun resolveLogPackageLabel(packageName: String): String {
        if (packageName.isBlank() || packageName == "-") return packageName
        return loadPackageManagerApp(packageName)
            ?.loadLabel(context.packageManager)
            ?.toString()
            ?.takeIf { it.isNotBlank() }
            ?: packageName
    }

    private fun statusRank(app: InstalledApp): Int = when {
        app.isEnabled -> 0
        app.isMissing -> 1
        app.isConfigured -> 2
        else -> 3
    }
}
