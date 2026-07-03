package org.srx.manager.data

import org.srx.manager.root.isSafePackageName

internal fun parseMonitorLogEntries(
    raw: String,
    filters: FileMonitorFilters? = null,
    labelResolver: (String) -> String = { it },
): List<LogEntry> {
    val labelCache = mutableMapOf<String, String>()
    return raw.lineSequence()
        .filter { it.isNotBlank() }
        .takeLastCompat(500)
        .mapNotNull { parseMonitorLogLine(it, labelCache, labelResolver) }
        .filterNot { it.path.substringAfterLast('/').matches(TempJsLogPathRegex) }
        .filterNot { it.isMediaStorePendingIntermediateRecord() }
        .filterNot { filters != null && it.matchesMonitorFilters(filters) }
        .toList()
        .coalesceMonitorLogEntries()
        .asReversed()
}

private fun parseMonitorLogLine(
    line: String,
    labelCache: MutableMap<String, String>,
    labelResolver: (String) -> String,
): LogEntry? {
    val parts = line.split('|')
    if (parts.size < 5) return null
    val extras = parts.drop(5).mapNotNull {
        val index = it.indexOf('=')
        if (index <= 0) null else it.substring(0, index) to it.substring(index + 1)
    }.toMap()
    if (extras["op"] == "monitor_watch") return null
    val ret = extras["ret"]?.toIntOrNull()
    val ok = ret?.let { it >= 0 } ?: !line.contains("error", true)
    val resultGroup = monitorResultGroup(ok, extras)
    val processPkg = parts.getOrNull(1).orEmpty()
    val callerPkg = parts.getOrNull(2).orEmpty()
    val watchPkg = extras["watch_package"].orEmpty()
    val identifyMethod = extras["identify_method"].orEmpty()
    val identifyReliability = extras["identify_reliability"].orEmpty()
    val source = extras["source"].orEmpty()
    val op = extras["op_filter"] ?: extras["op"] ?: parts.getOrNull(3).orEmpty()
    val landingPath = normalizeMonitorLogPath(parts.getOrNull(4).orEmpty())
    val isModuleExport = isModuleExportRecord(extras, processPkg, callerPkg, watchPkg)
    val pkg = if (isModuleExport) ModulePackageName else selectMonitorDisplayPackage(processPkg, callerPkg, watchPkg, identifyMethod)
    val operation = if (isModuleExport) "export" else formatMonitorLogOperationBadge(op)
    return LogEntry(
        timestamp = parts[0],
        timeText = parts[0].takeIf { it.length >= 16 }?.substring(11, 16) ?: "--:--",
        processPackage = processPkg,
        callerPackage = callerPkg,
        packageName = pkg,
        label = if (isModuleExport) ModuleDisplayLabel else resolveCachedLabel(pkg, labelCache, labelResolver),
        operation = operation,
        action = if (isModuleExport) describeModuleExportOperation(extras) else describeMonitorOperation(op, extras),
        path = landingPath,
        ok = ok,
        errorText = if (ok) "" else describeMonitorFailure(ret, extras),
        fromPath = extras["from"].orEmpty(),
        backendPath = extras["backend"].orEmpty(),
        landingPath = landingPath,
        watchPackage = watchPkg,
        identifyMethod = identifyMethod,
        identifyReliability = identifyReliability,
        source = source,
        resultGroup = resultGroup,
        filterOperation = op,
        isModuleWebUiExport = isModuleExport,
    )
}

private fun resolveCachedLabel(
    packageName: String,
    cache: MutableMap<String, String>,
    labelResolver: (String) -> String,
): String {
    if (packageName.isBlank() || packageName == "-") return packageName
    if (packageName == ModulePackageName) return ModuleDisplayLabel
    return cache.getOrPut(packageName) { labelResolver(packageName) }
}

private fun selectMonitorDisplayPackage(
    processPkg: String,
    callerPkg: String,
    watchPkg: String,
    identifyMethod: String,
): String =
    listOf(
        callerPkg.takeIf { it != processPkg && !it.isIntermediateLogPackage() },
        watchPkg.takeIf { identifyMethod == "watch_package" && !it.isIntermediateLogPackage() },
        callerPkg.takeIf { !it.isIntermediateLogPackage() },
        processPkg.takeIf { !it.isIntermediateLogPackage() },
        callerPkg,
        processPkg,
    )
        .firstOrNull { isSinglePackageName(it.orEmpty()) }
        ?: processPkg

private fun isSinglePackageName(value: String): Boolean =
    isSafePackageName(value) && value.contains('.')

private fun String?.isIntermediateLogPackage(): Boolean {
    val value = this.orEmpty()
    return value == "com.google.android.providers.media.module" ||
        value == "com.android.providers.media.module" ||
        value == "com.android.providers.media" ||
        value == "com.android.providers.downloads" ||
        value == "com.android.providers.downloads.ui" ||
        value == "com.android.externalstorage" ||
        value == "com.android.mtp" ||
        value.contains(".documentsui") ||
        value.contains(".photopicker")
}

private fun isModuleExportRecord(
    extras: Map<String, String>,
    processPkg: String,
    callerPkg: String,
    watchPkg: String,
): Boolean =
    extras["identify_method"] == "module_export" ||
        extras["source"] == "webui_export" ||
        extras["source"] == "webui_backup" ||
        listOf(processPkg, callerPkg, watchPkg).any { it == ModulePackageName }

private fun String.isDiagnosticLogArchivePath(): Boolean =
    substringAfterLast('/').matches(DiagnosticLogArchiveRegex)

private fun String?.isManagerAppPackage(): Boolean =
    this == "org.srx.manager" || this == "org.srx.manager.debug"

private fun describeModuleExportOperation(extras: Map<String, String>): String =
    when (extras["export_kind"].orEmpty().lowercase()) {
        "backup" -> "备份导出"
        "diagnostic", "logs", "log" -> "日志包导出"
        else -> when (extras["source"]) {
            "webui_backup" -> "备份导出"
            "webui_export" -> "日志包导出"
            else -> "模块导出"
        }
    }

private fun describeMonitorOperation(op: String, extras: Map<String, String>): String {
    val value = op.lowercase()
    return when (value) {
        "provider_open:read" -> "Provider 读取请求"
        "provider_open:create" -> "Provider 创建请求"
        "provider_open:write" -> "Provider 写入请求"
        else -> when (value.removeSuffix(":create").removeSuffix(":write").removeSuffix(":read")) {
            "open", "openat", "openat2", "provider_open" -> "带创建意图的文件打开"
            "mkdir", "mkdirat" -> "目录创建请求"
            "mknod", "mknodat" -> "文件节点创建请求"
            "create", "fuse_create", "inotify" -> "创建类文件操作"
            else -> if (op.isBlank()) "文件操作记录" else "文件操作：$op"
        }
    }
}

private fun formatMonitorLogOperationBadge(op: String): String {
    val value = op.trim().lowercase()
    if (value.isBlank()) return "unknown"
    return when (value) {
        "provider_open:read", "provider_open:create", "provider_open:write" -> value
        "inotify", "fuse_create" -> "create"
        else -> value.removeSuffix(":create").removeSuffix(":read")
    }
}

private fun describeMonitorFailure(ret: Int?, extras: Map<String, String>): String {
    if (ret != null && ret >= 0) return ""
    if (extras["deny_reason"] == "read_only_rule") {
        return "失败：命中只读模式规则"
    }
    return describeMonitorErrno(ret, extras["errno"]?.toIntOrNull())
}

private fun describeMonitorErrno(ret: Int?, errno: Int?): String {
    if (ret != null && ret >= 0) return ""
    return when (errno) {
        1 -> "失败：无权限"
        2 -> "失败：路径不存在"
        13 -> "失败：权限被拒绝"
        20 -> "失败：不是目录"
        21 -> "失败：是目录"
        28 -> "失败：空间不足"
        30 -> "失败：只读文件系统"
        107 -> "失败：传输端未连接"
        null, 0 -> "失败"
        else -> "失败 errno=$errno"
    }
}

private fun normalizeMonitorLogPath(path: String): String {
    val value = path.removePrefix("file://")
    val name = value.substringAfterLast('/')
    return if (name.matches(TempJsLogPathRegex)) {
        value.substringBeforeLast('/', "") + "/" + name.removePrefix(".").removeSuffix(".js")
    } else {
        value
    }
}

private fun List<LogEntry>.coalesceMonitorLogEntries(): List<LogEntry> {
    val groups = LinkedHashMap<String, LogEntry>()
    val ordered = mutableListOf<LogEntry>()
    for (entry in this) {
        val key = entry.coalesceKey()
        if (key.isEmpty()) {
            ordered += entry
            continue
        }
        val existing = groups[key]
        if (existing == null) {
            groups[key] = entry
            ordered += entry
            continue
        }
        val best = preferMonitorLogEntry(existing, entry)
        if (best !== existing) {
            groups[key] = best
            val index = ordered.indexOf(existing)
            if (index >= 0) ordered[index] = best
        }
    }
    return ordered
}

private fun LogEntry.coalesceKey(): String {
    val finalPath = coalescePathIdentity()
    if (finalPath.isBlank()) return ""
    val isDiagnosticArchive = finalPath.isDiagnosticLogArchivePath()
    val coalescePath = finalPath
        .diagnosticArchiveCoalescePath()
        .normalizedStoragePathForCoalesce()
    val op = if (isDiagnosticArchive) "diagnostic_export" else operation.normalizedMonitorOperation()
    return listOf(
        timestamp.take(16),
        coalescePath.replace(Regex("^/storage/emulated/\\d+/"), "/storage/emulated/*/"),
        op,
        coalesceResultGroup(),
    ).joinToString("|")
}

private fun LogEntry.coalescePathIdentity(): String {
    val backend = backendPath.normalizedMonitorPath()
    if (backend.isNotBlank()) return backend
    val finalPath = landingPath.ifBlank { path }.normalizedMonitorPath()
    if (finalPath.isBlank()) return ""
    if (fromPath.isNotBlank() && source.isFuseOrMappedCreateSource()) {
        return fromPath.normalizedMonitorPath()
    }
    return finalPath
}

private fun preferMonitorLogEntry(existing: LogEntry, candidate: LogEntry): LogEntry = when {
    candidate.shouldPreferRequestPathRecordOver(existing) -> candidate
    existing.shouldPreferRequestPathRecordOver(candidate) -> existing
    candidate.logRank() > existing.logRank() -> candidate
    else -> existing
}

private fun LogEntry.shouldPreferRequestPathRecordOver(other: LogEntry): Boolean =
    hasRequestPathRecord() && other.isIntermediateCreateOpenRecord()

private fun LogEntry.hasRequestPathRecord(): Boolean =
    fromPath.isNotBlank() &&
        fromPath != (landingPath.ifBlank { path }) &&
        source.isFuseOrMappedCreateSource()

private fun LogEntry.isIntermediateCreateOpenRecord(): Boolean =
    processPackage.isIntermediateLogPackage() &&
        fromPath.isBlank() &&
        operation.normalizedMonitorOperation() == "create"

private fun LogEntry.isMediaStorePendingIntermediateRecord(): Boolean =
    listOf(path, landingPath, backendPath).any { it.isMediaStorePendingPath() }

private fun String?.isMediaStorePendingPath(): Boolean {
    val name = this.orEmpty().substringAfterLast('/')
    val tail = name.removePrefix(".pending-")
    return tail.length != name.length &&
        tail.indexOf('-').let { index -> index >= 0 && index + 1 < tail.length }
}

private fun String.isFuseOrMappedCreateSource(): Boolean =
    this == "fuse_redirect" ||
        this == "path_mapping" ||
        this == "redirect_root" ||
        this == "read_only_path" ||
        this == "sandbox_path"

private fun LogEntry.coalesceResultGroup(): String {
    if (resultGroup.isNotBlank()) return resultGroup
    return if (ok) "ok" else "error:${errorText.ifBlank { "unknown" }}"
}

private fun monitorResultGroup(ok: Boolean, extras: Map<String, String>): String =
    if (ok) {
        "ok"
    } else if (extras["deny_reason"] == "read_only_rule") {
        "deny:read_only_rule"
    } else {
        "error:${extras["errno"].orEmpty().ifBlank { "unknown" }}"
    }

private fun LogEntry.logRank(): Int {
    var score = 0
    if (ok) score += 1000
    if ((landingPath.ifBlank { path }).isDiagnosticLogArchivePath()) {
        if (packageName.isManagerAppPackage() || callerPackage.isManagerAppPackage()) score += 420
        if (isModuleWebUiExport) score += 260
    }
    if (fromPath.isNotBlank()) score += 80
    if (callerPackage.isNotBlank() && callerPackage != "-" && callerPackage != processPackage) {
        score += if (callerPackage.isIntermediateLogPackage()) 30 else 260
    }
    if (
        identifyMethod == "watch_package" &&
        watchPackage.isNotBlank() &&
        watchPackage != "-" &&
        watchPackage != processPackage
    ) {
        score += if (watchPackage.isIntermediateLogPackage()) 20 else 140
    }
    score += when (identifyMethod) {
        "caller" -> 220
        "module_export" -> 220
        "provider_open" -> 210
        "fuse_redirect" -> 205
        "recent_private_caller" -> 200
        "recent_private_owner" -> 185
        "recent_caller" -> 180
        "watch_package" -> 170
        "java_stack", "stack" -> 150
        "path_config" -> 120
        "daemon_inotify" -> 100
        "media_provider_fallback" -> 70
        "path_owner", "owner_uid" -> 60
        "shared_uid" -> 10
        else -> 20
    }
    score += when (identifyReliability) {
        "high" -> 40
        "medium" -> 25
        "fallback" -> 5
        else -> 0
    }
    if (packageName.isIntermediateLogPackage()) score -= 80
    if (source == "allowed_real_path" && watchPackage.isNotBlank() && identifyMethod != "watch_package") {
        score -= 220
    }
    return score
}

private fun String.normalizedMonitorPath(): String =
    normalizeMonitorLogPath(this).replace(Regex("^file://", RegexOption.IGNORE_CASE), "")

private fun String.normalizedStoragePathForCoalesce(): String =
    normalizedMonitorPath().replace(Regex("^/data/media/(\\d+)/")) {
        "/storage/emulated/${it.groupValues[1]}/"
    }

private fun String.diagnosticArchiveCoalescePath(): String =
    if (isDiagnosticLogArchivePath()) {
        "/storage/emulated/*/Download/" + substringAfterLast('/')
    } else {
        this
    }

private fun String.normalizedMonitorOperation(): String =
    lowercase().removeSuffix(":create").removeSuffix(":write").removeSuffix(":read").let {
        if (it == "open" || it == "openat" || it == "openat2" || it == "provider_open" || it == "create" || it == "fuse_create" || it == "export") "create" else it
    }

private fun LogEntry.matchesMonitorFilters(filters: FileMonitorFilters): Boolean {
    val operationMatched = filters.excludedOperations.any { rule ->
        monitorOperationFilterMatches(rule, filterOperation.ifBlank { operation })
    }
    if (operationMatched) return true

    val paths = listOf(path, landingPath, fromPath, backendPath)
    return filters.excludedPaths.any { rule ->
        paths.any { path -> monitorPathFilterMatches(rule, path) }
    }
}

private fun monitorOperationFilterMatches(rule: String, operation: String): Boolean {
    val pattern = rule.trim().lowercase()
    val value = operation.trim().lowercase()
    if (pattern.isBlank() || value.isBlank() || '/' in pattern) return false
    return wildcardMatches(pattern, value)
}

private fun monitorPathFilterMatches(rule: String, path: String): Boolean {
    val pattern = SrxConfigNormalizer.sanitizeMonitorFilterPath(rule, allowLegacyAbsolute = true)
    if (pattern.isBlank()) return false
    val relative = monitorFilterRelativePath(path)
    if (relative.isBlank()) return false
    if (!pattern.hasMonitorWildcard()) {
        return relative == pattern || relative.startsWith("$pattern/")
    }
    if (wildcardMatches(pattern, relative)) return true
    return pattern.removeSuffix("/**").takeIf { it != pattern }?.let { base ->
        wildcardMatches(base, relative)
    } == true
}

private fun monitorFilterRelativePath(path: String): String {
    var value = normalizeMonitorLogPath(path)
        .replace('\\', '/')
        .replace(Regex("/+"), "/")
        .removeSuffix("/")
    if (value.isBlank()) return ""
    value = value.replace(Regex("^/sdcard(?=/|$)"), "/storage/emulated/0")
    value = value.replace(Regex("^/storage/self/primary(?=/|$)"), "/storage/emulated/0")
    value = value.replace(Regex("^/data/media/(\\d+)(?=/|$)")) { match ->
        "/storage/emulated/${match.groupValues[1]}"
    }
    val match = Regex("^/storage/emulated/\\d+/(.+)$").find(value) ?: return ""
    val relative = match.groupValues[1].trim('/')
    if (relative.isBlank() || relative.split('/').any { it == "." || it == ".." }) return ""
    return relative
}

private fun String.hasMonitorWildcard(): Boolean = '*' in this || '?' in this

private fun wildcardMatches(pattern: String, value: String): Boolean {
    val regex = buildString {
        append('^')
        pattern.forEach { ch ->
            when (ch) {
                '*' -> append(".*")
                '?' -> append('.')
                else -> append(Regex.escape(ch.toString()))
            }
        }
        append('$')
    }.toRegex()
    return regex.matches(value)
}

private fun Sequence<String>.takeLastCompat(count: Int): Sequence<String> =
    toList().takeLast(count).asSequence()

private val TempJsLogPathRegex = Regex("^\\.[^/]+\\.js$", RegexOption.IGNORE_CASE)
private val DiagnosticLogArchiveRegex = Regex("^storage-redirect-x-logs-.+\\.tar\\.gz$", RegexOption.IGNORE_CASE)
private const val ModulePackageName = "storage.redirect.x"
private const val ModuleDisplayLabel = "存储重定向X"
