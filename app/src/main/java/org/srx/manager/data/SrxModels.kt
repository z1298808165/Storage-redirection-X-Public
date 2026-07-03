package org.srx.manager.data

import android.content.pm.ApplicationInfo
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable

const val ModuleDir = "/data/adb/modules/storage.redirect.x"
const val ConfigDir = "$ModuleDir/config"
const val AppsDir = "$ConfigDir/apps"
const val TemplatesConfigPath = "$ConfigDir/templates.json"
const val FileMonitorFiltersConfigPath = "$ConfigDir/file_monitor_filters.json"
const val LogsDir = "$ModuleDir/logs"
const val GlobalConfigPath = "$ConfigDir/global.json"
const val StatsPath = "$ModuleDir/stats"
const val RuntimeDisablePath = "$ModuleDir/.runtime_disabled"
const val SrxCtlPath = "$ModuleDir/bin/srxctl"
const val DiagnosticArchiveScriptPath = "$ModuleDir/service.d/diagnostic_archive.sh"
const val ListAppsDexPath = "$ModuleDir/bin/list_apps.dex"
const val ListAppsOutputPath = "/data/Namespace-Proxy/list.config"
const val FileMonitorLogPath = "$LogsDir/file_monitor.log"
const val BackupMaxBytes = 8 * 1024 * 1024

@Serializable
data class GlobalConfig(
    @SerialName("file_monitor_enabled")
    val fileMonitorEnabled: Boolean = false,
    @SerialName("fuse_fix_enabled")
    val fuseFixEnabled: Boolean = true,
    @SerialName("fuse_daemon_redirect_enabled")
    val fuseDaemonRedirectEnabled: Boolean = false,
    @SerialName("verbose_logging_enabled")
    val verboseLoggingEnabled: Boolean = false,
    @SerialName("auto_enable_redirect_for_new_apps")
    val autoEnableRedirectForNewApps: Boolean = false,
    @SerialName("auto_enable_new_apps_template_id")
    val autoEnableNewAppsTemplateId: String = "",
    @SerialName("app_config_auto_save")
    val appConfigAutoSave: Boolean = false,
)

@Serializable
data class AppConfig(
    val users: Map<String, UserProfile> = emptyMap(),
)

@Serializable
data class ConfigTemplate(
    val id: String,
    val name: String,
    val config: AppConfig = AppConfig(),
)

@Serializable
data class ConfigTemplateStore(
    val templates: List<ConfigTemplate> = emptyList(),
)

@Serializable
data class FileMonitorFilters(
    @SerialName("excluded_paths")
    val excludedPaths: List<String> = listOf("Android/data"),
    @SerialName("excluded_operations")
    val excludedOperations: List<String> = listOf(
        "open:read",
        "open*:read",
        "provider_open:read",
        "rename*",
        "unlink*",
        "delete*",
        "rmdir*",
        "link*",
        "symlink*",
        "truncate*",
        "ftruncate*",
        "chmod*",
        "fchmod*",
        "utimens*",
        "futimens*",
        "attrib*",
    ),
)

@Serializable
data class UserProfile(
    val enabled: Boolean = true,
    @SerialName("mapping_mode_only")
    val mappingModeOnly: Boolean = false,
    @SerialName("allowed_real_paths")
    val allowedRealPaths: List<String> = emptyList(),
    @SerialName("excluded_real_paths")
    val excludedRealPaths: List<String> = emptyList(),
    @SerialName("sandboxed_paths")
    val sandboxedPaths: List<String> = emptyList(),
    @SerialName("read_only_paths")
    val readOnlyPaths: List<String> = emptyList(),
    @SerialName("path_mappings")
    val pathMappings: Map<String, String> = emptyMap(),
)

enum class ModuleStatus {
    Enabled,
    Disabled,
    RebootRequired,
    Unknown,
}

enum class AppFilter {
    User,
    System,
    Configured,
}

data class InstalledApp(
    val packageName: String,
    val label: String,
    val isSystem: Boolean,
    val appInfo: ApplicationInfo?,
    val config: AppConfig?,
    val isInstalled: Boolean = appInfo != null,
) {
    val isConfigured: Boolean get() = config != null
    val isMissing: Boolean get() = isConfigured && !isInstalled
    val isEnabled: Boolean
        get() = isInstalled && config?.users?.values?.any { it.enabled } == true
}

data class LogEntry(
    val timestamp: String,
    val timeText: String,
    val processPackage: String,
    val callerPackage: String,
    val packageName: String,
    val label: String,
    val operation: String,
    val action: String,
    val path: String,
    val ok: Boolean,
    val errorText: String = "",
    val fromPath: String = "",
    val backendPath: String = "",
    val landingPath: String = "",
    val watchPackage: String = "",
    val identifyMethod: String = "",
    val identifyReliability: String = "",
    val source: String = "",
    val resultGroup: String = "",
    val isModuleWebUiExport: Boolean = false,
)

data class DashboardState(
    val status: ModuleStatus = ModuleStatus.Unknown,
    val version: String = "",
    val globalConfig: GlobalConfig = GlobalConfig(),
    val enabledApps: Int = 0,
    val effectiveEvents: Int = 0,
)

@Serializable
data class BackupPayload(
    val magic: String,
    val schema: Int,
    val module: BackupModuleInfo,
    val createdAt: String,
    val summary: BackupSummary,
    val integrity: BackupIntegrity,
    val data: BackupData,
)

@Serializable
data class BackupModuleInfo(
    val id: String,
    val version: String = "",
)

@Serializable
data class BackupSummary(
    val appCount: Int,
    val userCount: Int,
)

@Serializable
data class BackupIntegrity(
    val algorithm: String,
    val value: String,
)

@Serializable
data class BackupData(
    val global: GlobalConfig = GlobalConfig(),
    val apps: Map<String, AppConfig> = emptyMap(),
    val templates: List<ConfigTemplate> = emptyList(),
    @SerialName("monitor_filters")
    val monitorFilters: FileMonitorFilters = FileMonitorFilters(),
    val ui: BackupUiPreferences? = null,
)

@Serializable
data class BackupUiPreferences(
    @SerialName("predictive_back")
    val predictiveBack: Boolean? = null,
    @SerialName("floating_bottom_bar")
    val floatingBottomBar: Boolean? = null,
    @SerialName("liquid_glass")
    val liquidGlass: Boolean? = null,
    @SerialName("blur_effect")
    val blurEffect: Boolean? = null,
    @SerialName("dynamic_color")
    val dynamicColor: Boolean? = null,
    @SerialName("accent_color")
    val accentColor: Int? = null,
    @SerialName("color_style")
    val colorStyle: UiColorStyle? = null,
    @SerialName("color_spec")
    val colorSpec: UiColorSpec? = null,
    @SerialName("theme_mode")
    val themeMode: UiThemeMode? = null,
    @SerialName("page_scale")
    val pageScale: Float? = null,
    @SerialName("auto_check_updates")
    val autoCheckUpdates: Boolean? = null,
    @SerialName("update_channel")
    val updateChannel: UpdateChannel? = null,
)
