package org.srx.manager.ui

import android.app.Application
import android.net.Uri
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import java.time.LocalDateTime
import java.time.format.DateTimeFormatter
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.srx.manager.BuildConfig
import org.srx.manager.data.AppConfig
import org.srx.manager.data.AppFilter
import org.srx.manager.data.BackupArchiveCodec
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.data.DashboardState
import org.srx.manager.data.DiagnosticArchiveProgress
import org.srx.manager.data.FileMonitorFilters
import org.srx.manager.data.GlobalConfig
import org.srx.manager.data.InstalledApp
import org.srx.manager.data.LogEntry
import org.srx.manager.data.PreferencesRepository
import org.srx.manager.data.ReleaseUpdate
import org.srx.manager.data.SrxConfigNormalizer
import org.srx.manager.data.SrxRepository
import org.srx.manager.data.UiColorSpec
import org.srx.manager.data.UiColorStyle
import org.srx.manager.data.UiPreferences
import org.srx.manager.data.UiThemeMode
import org.srx.manager.data.UpdateChannel
import org.srx.manager.data.UpdateChecker
import org.srx.manager.data.UserProfile
import org.srx.manager.root.RootShell
import org.srx.manager.root.isSafePackageName

data class AppUiState(
    val rootChecked: Boolean = false,
    val rootGranted: Boolean = false,
    val loading: Boolean = true,
    val busy: Boolean = false,
    val busyMessage: String? = null,
    val busyProgress: Float? = null,
    val appsRefreshing: Boolean = false,
    val logsRefreshing: Boolean = false,
    val appsLoaded: Boolean = false,
    val error: String? = null,
    val dashboard: DashboardState = DashboardState(),
    val apps: List<InstalledApp> = emptyList(),
    val users: List<String> = listOf("0"),
    val selectedUser: String = "0",
    val filter: AppFilter = AppFilter.User,
    val search: String = "",
    val currentApp: InstalledApp? = null,
    val currentConfig: AppConfig? = null,
    val templates: List<ConfigTemplate> = emptyList(),
    val autoTemplateFallbackNoticeId: String = "",
    val logs: List<LogEntry> = emptyList(),
    val logApps: List<InstalledApp> = emptyList(),
    val fileMonitorFilters: FileMonitorFilters = FileMonitorFilters(),
    val updateCheckRunning: Boolean = false,
    val pendingUpdate: ReleaseUpdate? = null,
    val snackbar: String? = null,
)

class SrxViewModel(
    application: Application,
) : AndroidViewModel(application) {
  private val shell = RootShell()
  private val repository = SrxRepository(application, shell)
  private val prefs = PreferencesRepository(application)
  private val updateChecker = UpdateChecker("SRX-Manager/${BuildConfig.VERSION_NAME}")
  private val disabledDefaultProfile = UserProfile(enabled = false)

  val uiPreferences: StateFlow<UiPreferences> =
      prefs.uiPreferences.stateIn(
          viewModelScope,
          SharingStarted.Eagerly,
          UiPreferences(),
      )

  private val _state = MutableStateFlow(AppUiState())
  val state: StateFlow<AppUiState> = _state.asStateFlow()
  private var appsJob: Job? = null
  private var openAppJob: Job? = null
  private var configSaveJob: Job? = null
  private var logAppsJob: Job? = null
  private var dashboardCountsJob: Job? = null
  private val monitorFiltersSaveMutex = Mutex()
  private val globalConfigSaveRequests = Channel<GlobalConfig>(Channel.CONFLATED)

  init {
    viewModelScope.launch {
      for (config in globalConfigSaveRequests) {
        val ok = repository.writeGlobalConfig(config)
        if (ok) {
          if (_state.value.dashboard.globalConfig == config) {
            showMessage("设置已保存")
          }
          continue
        }

        val persisted = runCatching { repository.readGlobalConfig() }.getOrDefault(config)
        if (_state.value.dashboard.globalConfig == config) {
          _state.value =
              _state.value.copy(
                  dashboard = _state.value.dashboard.copy(globalConfig = persisted),
              )
        }
        showMessage("保存设置失败")
      }
    }
    refreshRootAndAll()
  }

  fun refreshRootAndAll() {
    viewModelScope.launch {
      _state.value = _state.value.copy(loading = true, error = null)
      val root = repository.checkRoot()
      _state.value = _state.value.copy(rootChecked = true, rootGranted = root)
      if (!root) {
        _state.value = _state.value.copy(loading = false, error = "未获得 root 权限")
        return@launch
      }
      runCatching { repository.readDashboardSummary() }
          .onSuccess { _state.value = _state.value.copy(dashboard = it, error = null) }
          .onFailure { showMessage("加载概览失败：${it.message ?: "未知错误"}") }
      _state.value = _state.value.copy(loading = false)
      refreshDashboardCounts()
      refreshUsers()
      refreshTemplates()
      refreshFileMonitorFilters()
    }
  }

  fun refreshDashboard() {
    viewModelScope.launch {
      runCatching { repository.readDashboardSummary() }
          .onSuccess { _state.value = _state.value.copy(dashboard = it, error = null) }
          .onFailure { showMessage("加载概览失败：${it.message ?: "未知错误"}") }
      refreshDashboardCounts()
    }
  }

  fun refreshDashboardCounts() {
    if (!_state.value.rootGranted || dashboardCountsJob?.isActive == true) return
    dashboardCountsJob =
        viewModelScope.launch {
          runCatching { repository.readDashboardCounts() }
              .onSuccess { (enabledApps, runtimeActivations) ->
                _state.value =
                    _state.value.copy(
                        dashboard =
                            _state.value.dashboard.copy(
                                enabledApps = enabledApps,
                                runtimeActivations = runtimeActivations,
                            ),
                    )
              }
        }
  }

  fun refreshUsers() {
    viewModelScope.launch {
      runCatching { repository.listUsers() }
          .onSuccess { users ->
            val selected =
                _state.value.selectedUser.takeIf { it in users } ?: users.firstOrNull() ?: "0"
            _state.value = _state.value.copy(users = users, selectedUser = selected)
          }
    }
  }

  fun refreshApps(force: Boolean = false) {
    appsJob?.cancel()
    appsJob =
        viewModelScope.launch {
          _state.value = _state.value.copy(appsRefreshing = true)
          try {
            val apps = repository.loadInstalledApps(_state.value.selectedUser, force)
            _state.value =
                _state.value.copy(
                    apps = apps,
                    appsRefreshing = false,
                    appsLoaded = true,
                    error = null,
                    dashboard =
                        _state.value.dashboard.copy(enabledApps = apps.count { it.isEnabled }),
                )
          } catch (canceled: CancellationException) {
            throw canceled
          } catch (error: Throwable) {
            _state.value = _state.value.copy(appsRefreshing = false, appsLoaded = true)
            showMessage("加载应用列表失败：${error.message ?: "未知错误"}")
          }
        }
  }

  fun ensureAppsLoaded() {
    val state = _state.value
    if (!state.appsLoaded && !state.appsRefreshing) refreshApps(force = true)
  }

  fun selectUser(userId: String) {
    if (userId == _state.value.selectedUser) return
    logAppsJob?.cancel()
    _state.value =
        _state.value.copy(
            selectedUser = userId,
            apps = emptyList(),
            appsLoaded = false,
            logApps = emptyList(),
        )
    refreshApps(force = true)
  }

  fun setFilter(filter: AppFilter) {
    _state.value = _state.value.copy(filter = filter)
  }

  fun setSearch(query: String) {
    _state.value = _state.value.copy(search = query)
  }

  fun openApp(app: InstalledApp) {
    openAppJob?.cancel()
    _state.value = _state.value.copy(currentApp = app, currentConfig = null)
    refreshTemplates()
    openAppJob =
        viewModelScope.launch {
          val config = app.config ?: repository.readAppConfig(app.packageName) ?: defaultAppConfig()
          if (_state.value.currentApp?.packageName == app.packageName) {
            _state.value = _state.value.copy(currentApp = app, currentConfig = config)
          }
        }
  }

  fun closeAppConfig() {
    openAppJob?.cancel()
    openAppJob = null
    _state.value = _state.value.copy(currentApp = null, currentConfig = null)
  }

  fun updateProfile(transform: (UserProfile) -> UserProfile) {
    val state = _state.value
    val app = state.currentApp ?: return
    val current = state.currentConfig ?: return
    val user = state.selectedUser
    val profile = current.users[user] ?: disabledDefaultProfile
    val updated = current.copy(users = current.users + (user to transform(profile)))
    _state.value = state.copy(currentConfig = updated)
    if (state.dashboard.globalConfig.appConfigAutoSave) scheduleConfigSave(app.packageName, updated)
  }

  fun addAllowedPath(value: String) =
      updateListPath(value, allowRuleSyntax = true) { profile, path ->
        profile.copy(allowedRealPaths = (profile.allowedRealPaths + path).distinct().sorted())
      }

  fun addSandboxPath(value: String) =
      updateListPath(value) { profile, path ->
        profile.copy(sandboxedPaths = (profile.sandboxedPaths + path).distinct().sorted())
      }

  fun updateAllowedPath(
      oldValue: String,
      newValue: String,
  ) =
      updateListPath(newValue, allowRuleSyntax = true) { profile, path ->
        profile.copy(
            allowedRealPaths = (profile.allowedRealPaths - oldValue + path).distinct().sorted()
        )
      }

  fun updateSandboxPath(
      oldValue: String,
      newValue: String,
  ) =
      updateListPath(newValue) { profile, path ->
        profile.copy(
            sandboxedPaths = (profile.sandboxedPaths - oldValue + path).distinct().sorted()
        )
      }

  fun removeAllowedPath(value: String) = updateProfile {
    it.copy(allowedRealPaths = it.allowedRealPaths - value)
  }

  fun setReadOnlyEnabled(enabled: Boolean) = updateProfile {
    if (enabled) it else it.copy(readOnlyPaths = emptyList())
  }

  fun addReadOnlyPath(value: String) =
      updateListPath(
          value,
          allowRuleSyntax = true,
          allowWildcards = true,
      ) { profile, path ->
        profile.copy(readOnlyPaths = (profile.readOnlyPaths + path).distinct().sorted())
      }

  fun updateReadOnlyPath(
      oldValue: String,
      newValue: String,
  ) =
      updateListPath(
          newValue,
          allowRuleSyntax = true,
          allowWildcards = true,
      ) { profile, path ->
        profile.copy(readOnlyPaths = (profile.readOnlyPaths - oldValue + path).distinct().sorted())
      }

  fun removeReadOnlyPath(value: String) = updateProfile {
    it.copy(readOnlyPaths = it.readOnlyPaths - value)
  }

  fun removeSandboxPath(value: String) = updateProfile {
    it.copy(sandboxedPaths = it.sandboxedPaths - value)
  }

  fun addMapping(
      from: String,
      to: String,
  ) {
    val cleanFrom = cleanPath(from, allowRuleSyntax = false)
    val cleanTo = cleanPath(to, allowRuleSyntax = false)
    if (cleanFrom.isBlank() || cleanTo.isBlank() || cleanFrom == cleanTo) {
      showMessage("映射路径无效")
      return
    }
    updateProfile {
      it.copy(pathMappings = (it.pathMappings + (cleanFrom to cleanTo)).toSortedMap())
    }
  }

  fun updateMapping(
      oldFrom: String,
      from: String,
      to: String,
  ) {
    val cleanOldFrom = cleanPath(oldFrom, allowRuleSyntax = false)
    val cleanFrom = cleanPath(from, allowRuleSyntax = false)
    val cleanTo = cleanPath(to, allowRuleSyntax = false)
    if (cleanFrom.isBlank() || cleanTo.isBlank() || cleanFrom == cleanTo) {
      showMessage("映射路径无效")
      return
    }
    updateProfile { profile ->
      val mappings = profile.pathMappings.toMutableMap()
      mappings.remove(cleanOldFrom)
      mappings[cleanFrom] = cleanTo
      profile.copy(pathMappings = mappings.toSortedMap())
    }
  }

  fun removeMapping(from: String) = updateProfile { it.copy(pathMappings = it.pathMappings - from) }

  fun saveCurrentConfig(silent: Boolean = false) {
    val app = _state.value.currentApp ?: return
    val config = _state.value.currentConfig ?: return
    if (!isSafePackageName(app.packageName)) return
    configSaveJob?.cancel()
    viewModelScope.launch {
      if (!silent) updateBusy(BusyStateChange.Started())
      val ok = repository.writeAppConfig(app.packageName, config)
      if (!silent) updateBusy(BusyStateChange.Finished)
      if (ok) {
        applyConfigToState(app.packageName, config)
        if (!silent) showMessage("配置已保存")
      } else {
        showMessage("保存配置失败")
      }
    }
  }

  fun deleteCurrentConfig() {
    val app = _state.value.currentApp ?: return
    if (!isSafePackageName(app.packageName)) return
    configSaveJob?.cancel()
    viewModelScope.launch {
      updateBusy(BusyStateChange.Started())
      val ok = repository.deleteAppConfig(app.packageName)
      updateBusy(BusyStateChange.Finished)
      if (ok) {
        applyConfigToState(app.packageName, null)
        showMessage("配置已删除")
        closeAppConfig()
      } else {
        showMessage("删除失败")
      }
    }
  }

  fun updateGlobalConfig(config: GlobalConfig) {
    val normalized = SrxConfigNormalizer.normalizeGlobalConfig(config)
    val current = _state.value.dashboard.globalConfig
    if (normalized == current) return
    _state.value =
        _state.value.copy(
            dashboard = _state.value.dashboard.copy(globalConfig = normalized),
            autoTemplateFallbackNoticeId = "",
        )
    globalConfigSaveRequests.trySend(normalized)
  }

  fun refreshTemplates() {
    viewModelScope.launch {
      runCatching { repository.readTemplates() }
          .onSuccess { templates ->
            _state.value = _state.value.copy(templates = templates)
            reconcileAutoTemplateFallback(templates)
          }
    }
  }

  private suspend fun reconcileAutoTemplateFallback(templates: List<ConfigTemplate>) {
    val global = repository.readGlobalConfig()
    val templateId = global.autoEnableNewAppsTemplateId
    if (
        !global.autoEnableRedirectForNewApps ||
            templateId.isBlank() ||
            templates.any { it.id == templateId }
    ) {
      _state.value =
          _state.value.copy(
              dashboard = _state.value.dashboard.copy(globalConfig = global),
              autoTemplateFallbackNoticeId =
                  when {
                    !global.autoEnableRedirectForNewApps -> ""
                    templateId.isNotBlank() -> ""
                    else -> _state.value.autoTemplateFallbackNoticeId
                  },
          )
      return
    }
    val fallbackGlobal = global.copy(autoEnableNewAppsTemplateId = "")
    if (repository.writeGlobalConfig(fallbackGlobal)) {
      _state.value =
          _state.value.copy(
              dashboard = _state.value.dashboard.copy(globalConfig = fallbackGlobal),
              autoTemplateFallbackNoticeId = templateId,
          )
      showMessage("自动配置模板已失效，已回退")
    }
  }

  fun saveCurrentConfigAsTemplate(name: String) {
    val config = _state.value.currentConfig ?: return
    viewModelScope.launch {
      val ok = repository.upsertTemplate(name, config)
      if (ok) {
        _state.value = _state.value.copy(templates = repository.readTemplates())
        showMessage("模板已保存")
      } else {
        showMessage("保存模板失败")
      }
    }
  }

  fun addTemplate(name: String) {
    viewModelScope.launch {
      val ok = repository.upsertTemplate(name, defaultAppConfig())
      if (ok) {
        _state.value = _state.value.copy(templates = repository.readTemplates())
        showMessage("模板已添加")
      } else {
        showMessage("添加模板失败")
      }
    }
  }

  fun saveTemplate(template: ConfigTemplate) {
    viewModelScope.launch {
      val ok = repository.upsertTemplate(template.name, template.config, template.id)
      if (ok) {
        _state.value = _state.value.copy(templates = repository.readTemplates())
        showMessage("模板已保存")
      } else {
        showMessage("保存模板失败")
      }
    }
  }

  fun renameTemplate(
      templateId: String,
      name: String,
  ) {
    viewModelScope.launch {
      val template = repository.readTemplates().firstOrNull { it.id == templateId } ?: return@launch
      val ok = repository.upsertTemplate(name, template.config, templateId)
      if (ok) {
        _state.value = _state.value.copy(templates = repository.readTemplates())
        showMessage("模板已重命名")
      } else {
        showMessage("重命名失败")
      }
    }
  }

  fun deleteTemplate(templateId: String) {
    viewModelScope.launch {
      if (repository.readGlobalConfig().autoEnableNewAppsTemplateId == templateId) {
        showMessage("该模板正用于新应用自动配置，不能删除")
        return@launch
      }
      val ok = repository.deleteTemplate(templateId)
      if (ok) {
        _state.value = _state.value.copy(templates = repository.readTemplates())
        showMessage("模板已删除")
      } else {
        showMessage("删除模板失败")
      }
    }
  }

  fun applyTemplateToCurrentApp(templateId: String) {
    val app = _state.value.currentApp ?: return
    applyTemplateToApps(templateId, listOf(app.packageName))
  }

  fun applyTemplateToApps(
      templateId: String,
      packageNames: Collection<String>,
  ) {
    val targets = packageNames.filter(::isSafePackageName).distinct()
    if (targets.isEmpty()) return
    viewModelScope.launch {
      updateBusy(BusyStateChange.Started("正在应用模板到 ${targets.size} 个应用"))
      val ok = repository.applyTemplateToApps(templateId, targets)
      val templates = repository.readTemplates()
      val appliedTemplate = templates.firstOrNull { it.id == templateId }
      val templateConfig = appliedTemplate?.config
      if (ok && templateConfig != null) {
        targets.forEach { packageName -> applyConfigToState(packageName, templateConfig) }
        if (_state.value.currentApp?.packageName in targets) {
          _state.value = _state.value.copy(currentConfig = templateConfig)
        }
        showMessage("模板已应用到 ${targets.size} 个应用")
      } else {
        refreshApps(force = true)
        showMessage("应用模板失败")
      }
      updateBusy(BusyStateChange.Finished)
      _state.value = _state.value.copy(templates = templates)
    }
  }

  fun setFloatingBottomBar(enabled: Boolean) {
    viewModelScope.launch { prefs.setFloatingBottomBar(enabled) }
  }

  fun setLiquidGlass(enabled: Boolean) {
    viewModelScope.launch { prefs.setLiquidGlass(enabled) }
  }

  fun setBlurEffect(enabled: Boolean) {
    viewModelScope.launch { prefs.setBlurEffect(enabled) }
  }

  fun setDynamicColor(enabled: Boolean) {
    viewModelScope.launch { prefs.setDynamicColor(enabled) }
  }

  fun setAccentColor(color: Int) {
    viewModelScope.launch { prefs.setAccentColor(color) }
  }

  fun setColorStyle(style: UiColorStyle) {
    viewModelScope.launch { prefs.setColorStyle(style) }
  }

  fun setColorSpec(spec: UiColorSpec) {
    viewModelScope.launch { prefs.setColorSpec(spec) }
  }

  fun setThemeMode(mode: UiThemeMode) {
    viewModelScope.launch { prefs.setThemeMode(mode) }
  }

  fun setPredictiveBack(enabled: Boolean) {
    prefs.setPredictiveBackCompatPref(enabled)
    viewModelScope.launch { prefs.setPredictiveBack(enabled, persistCompatPref = false) }
  }

  fun setPageScale(scale: Float) {
    viewModelScope.launch { prefs.setPageScale(scale) }
  }

  fun setAutoCheckUpdates(enabled: Boolean) {
    viewModelScope.launch { prefs.setAutoCheckUpdates(enabled) }
  }

  fun setUpdateChannel(channel: UpdateChannel) {
    viewModelScope.launch { prefs.setUpdateChannel(channel) }
  }

  fun checkForUpdates(manual: Boolean) {
    if (_state.value.updateCheckRunning) return
    viewModelScope.launch {
      val moduleVersion = _state.value.dashboard.version.trim()
      if (moduleVersion.isBlank()) {
        if (manual) showMessage("尚未读取模块版本")
        return@launch
      }
      _state.value = _state.value.copy(updateCheckRunning = true)
      runCatching {
            updateChecker.check(
                manifestUrl = BuildConfig.UPDATE_MANIFEST_URL,
                repository = BuildConfig.RELEASE_REPOSITORY,
                currentVersionName = moduleVersion,
                channel = uiPreferences.value.updateChannel,
            )
          }
          .onSuccess { update ->
            if (update != null) {
              _state.value = _state.value.copy(pendingUpdate = update)
            } else if (manual) {
              showMessage("已是最新版本")
            }
          }
          .onFailure { error -> if (manual) showMessage("检查更新失败：${error.message ?: "未知错误"}") }
      _state.value = _state.value.copy(updateCheckRunning = false)
    }
  }

  fun clearPendingUpdate() {
    _state.value = _state.value.copy(pendingUpdate = null)
  }

  fun setModuleEnabled(enabled: Boolean) {
    viewModelScope.launch {
      updateBusy(
          BusyStateChange.Started(
              if (enabled) "正在启动模块并重启 MediaProvider" else "正在停止模块并重启 MediaProvider"
          )
      )
      val ok = repository.setModuleEnabled(enabled)
      updateBusy(BusyStateChange.Finished)
      showMessage(if (ok) "模块状态已更新" else "模块状态切换失败")
      refreshDashboard()
    }
  }

  fun restartMediaProvider() {
    viewModelScope.launch {
      updateBusy(BusyStateChange.Started("正在快速重启 MediaProvider"))
      val ok = repository.restartMediaProvider()
      updateBusy(BusyStateChange.Finished)
      showMessage(if (ok) "MediaProvider 已重启" else "重启 MediaProvider 超时")
    }
  }

  fun refreshLogs() {
    viewModelScope.launch {
      updateLogs(LogStateChange.RefreshStarted)
      runCatching { repository.readLogs() to repository.readFileMonitorFilters() }
          .onSuccess { (logs, filters) ->
            updateLogs(LogStateChange.RefreshSucceeded(logs, filters))
            refreshLogAppInfo(logs)
          }
          .onFailure {
            updateLogs(LogStateChange.RefreshFailed)
            showMessage("加载日志失败")
          }
    }
  }

  fun refreshFileMonitorFilters() {
    viewModelScope.launch {
      runCatching { repository.readFileMonitorFilters() }
          .onSuccess { filters -> _state.value = _state.value.copy(fileMonitorFilters = filters) }
    }
  }

  fun saveFileMonitorFilters(
      filters: FileMonitorFilters,
      silent: Boolean = false,
  ) {
    viewModelScope.launch {
      val ok = monitorFiltersSaveMutex.withLock { repository.writeFileMonitorFilters(filters) }
      if (ok) {
        _state.value = _state.value.copy(fileMonitorFilters = filters)
        if (!silent) showMessage("过滤配置已保存")
      } else {
        showMessage("保存过滤配置失败")
      }
    }
  }

  private fun refreshLogAppInfo(logs: List<LogEntry>) {
    val packages =
        logs
            .flatMap {
              listOf(it.packageName, it.callerPackage, it.processPackage, it.watchPackage)
            }
            .filter(::isSafePackageName)
            .toSet()
    if (packages.isEmpty()) {
      updateLogs(LogStateChange.AppsResolved(emptyList()))
      return
    }

    logAppsJob?.cancel()
    logAppsJob =
        viewModelScope.launch {
          runCatching {
                repository.loadInstalledAppsForPackages(packages, _state.value.selectedUser)
              }
              .onSuccess { logApps ->
                val knownApps = _state.value.apps.associateBy { it.packageName }
                val merged =
                    (logApps.associateBy { it.packageName } + knownApps).values.sortedBy {
                      it.packageName
                    }
                updateLogs(LogStateChange.AppsResolved(merged))
              }
        }
  }

  fun clearLogs() {
    viewModelScope.launch {
      val ok = repository.clearLogs()
      if (ok) {
        logAppsJob?.cancel()
        updateLogs(LogStateChange.Cleared)
        showMessage("日志已清空")
      } else {
        showMessage("清空失败")
      }
    }
  }

  fun resetRuntimeStats() {
    if (!_state.value.rootGranted) return
    viewModelScope.launch {
      val ok = repository.resetRuntimeStats()
      if (ok) {
        runCatching { repository.readDashboardCounts() }
            .onSuccess { (enabledApps, runtimeActivations) ->
              _state.value =
                  _state.value.copy(
                      dashboard =
                          _state.value.dashboard.copy(
                              enabledApps = enabledApps,
                              runtimeActivations = runtimeActivations,
                          ),
                  )
            }
        showMessage("生效次数已清零")
      } else {
        showMessage("生效次数清零失败")
      }
    }
  }

  fun backupFileName(): String {
    val stamp = LocalDateTime.now().format(DateTimeFormatter.ofPattern("yyyyMMdd-HHmmss"))
    return "storage-redirect-x-backup-$stamp.srxbak.zip"
  }

  fun diagnosticArchiveFileName(): String {
    val stamp = LocalDateTime.now().format(DateTimeFormatter.ofPattern("yyyyMMdd-HHmmss"))
    return "storage-redirect-x-logs-$stamp.tar.gz"
  }

  fun exportDiagnosticArchiveToUri(uri: Uri) {
    viewModelScope.launch {
      updateBusy(BusyStateChange.Started("正在启动日志导出", 0.01f))
      runCatching { repository.exportDiagnosticArchive(uri, ::updateDiagnosticArchiveProgress) }
          .onSuccess { ok -> showMessage(if (ok) "日志包已保存" else "日志导出失败") }
          .onFailure { showMessage(it.message ?: "日志导出失败") }
      updateBusy(BusyStateChange.Finished)
    }
  }

  fun exportDiagnosticArchiveToDirectory(uri: Uri) {
    viewModelScope.launch {
      updateBusy(BusyStateChange.Started("正在启动日志导出", 0.01f))
      runCatching {
            repository.exportDiagnosticArchiveToDirectory(
                uri,
                diagnosticArchiveFileName(),
                ::updateDiagnosticArchiveProgress,
            )
          }
          .onSuccess { ok -> showMessage(if (ok) "日志包已保存" else "日志导出失败") }
          .onFailure { showMessage(it.message ?: "日志导出失败") }
      updateBusy(BusyStateChange.Finished)
    }
  }

  private suspend fun updateDiagnosticArchiveProgress(progress: DiagnosticArchiveProgress) {
    withContext(Dispatchers.Main.immediate) { updateBusy(BusyStateChange.Progress(progress)) }
  }

  fun exportBackupToUri(uri: Uri) {
    viewModelScope.launch {
      updateBusy(BusyStateChange.Started())
      runCatching { writeBytesToUri(uri, repository.buildBackupZipBytes()) }
          .onSuccess { showMessage("备份已保存") }
          .onFailure { showMessage(it.message ?: "备份失败") }
      updateBusy(BusyStateChange.Finished)
    }
  }

  fun restoreBackupFromUri(uri: Uri) {
    viewModelScope.launch {
      updateBusy(BusyStateChange.Started())
      runCatching { repository.restoreBackupFileBytes(readBytesFromUri(uri)) }
          .onSuccess { ok ->
            if (ok) {
              showMessage("配置已还原")
              refreshDashboard()
              refreshUsers()
              refreshTemplates()
              refreshApps(force = true)
              refreshFileMonitorFilters()
              refreshLogs()
            } else {
              showMessage("写入配置失败")
            }
          }
          .onFailure { showMessage(it.message ?: "还原失败") }
      updateBusy(BusyStateChange.Finished)
    }
  }

  fun listStorageDirectories(
      userId: String,
      dirRel: String,
      onResult: (List<String>) -> Unit,
  ) {
    viewModelScope.launch {
      val entries =
          runCatching { repository.listStorageDirectories(userId, dirRel) }
              .getOrDefault(emptyList())
      onResult(entries)
    }
  }

  fun clearSnackbar() {
    _state.value = _state.value.copy(snackbar = null)
  }

  private fun defaultAppConfig(): AppConfig =
      AppConfig(
          users = mapOf(_state.value.selectedUser to disabledDefaultProfile),
      )

  private fun scheduleConfigSave(
      packageName: String,
      config: AppConfig,
  ) {
    if (!isSafePackageName(packageName)) return
    configSaveJob?.cancel()
    configSaveJob =
        viewModelScope.launch {
          delay(350)
          val ok = repository.writeAppConfig(packageName, config)
          if (ok) {
            applyConfigToState(packageName, config)
          } else {
            showMessage("自动保存失败")
          }
        }
  }

  private fun applyConfigToState(
      packageName: String,
      config: AppConfig?,
  ) {
    val state = _state.value
    val apps =
        state.apps.mapNotNull { app ->
          if (app.packageName != packageName) {
            app
          } else if (config == null && app.isMissing) {
            null
          } else {
            app.copy(config = config)
          }
        }
    val currentApp =
        state.currentApp?.let { app ->
          if (app.packageName == packageName) app.copy(config = config) else app
        }
    _state.value =
        state.copy(
            apps = apps,
            currentApp = currentApp,
            dashboard = state.dashboard.copy(enabledApps = apps.count { it.isEnabled }),
        )
  }

  private fun updateListPath(
      value: String,
      allowRuleSyntax: Boolean = false,
      allowWildcards: Boolean = allowRuleSyntax,
      transform: (UserProfile, String) -> UserProfile,
  ) {
    val path = cleanPath(value, allowRuleSyntax, allowWildcards)
    if (path.isBlank()) {
      showMessage("路径无效：请使用相对路径，不能包含 ..、控制字符或存储根目录")
      return
    }
    updateProfile { transform(it, path) }
  }

  private fun cleanPath(
      value: String,
      allowRuleSyntax: Boolean,
      allowWildcards: Boolean = allowRuleSyntax,
  ): String {
    val raw = value.trim()
    if (!allowRuleSyntax && raw.startsWith("!")) return ""
    return SrxConfigNormalizer.sanitizeEditablePath(raw, allowRuleSyntax, allowWildcards)
  }

  private fun showMessage(message: String) {
    _state.value = _state.value.copy(snackbar = message)
  }

  private fun updateBusy(change: BusyStateChange) {
    _state.value = _state.value.reduceBusy(change)
  }

  private fun updateLogs(change: LogStateChange) {
    _state.value = _state.value.reduceLogs(change)
  }

  private suspend fun writeBytesToUri(
      uri: Uri,
      bytes: ByteArray,
  ) =
      withContext(Dispatchers.IO) {
        val resolver = getApplication<Application>().contentResolver
        resolver.openOutputStream(uri, "w")?.use { it.write(bytes) } ?: error("无法写入所选文件")
      }

  private suspend fun readBytesFromUri(uri: Uri): ByteArray =
      withContext(Dispatchers.IO) {
        val resolver = getApplication<Application>().contentResolver
        resolver.openInputStream(uri)?.use { BackupArchiveCodec.readBytesBounded(it) }
            ?: error("无法读取所选文件")
      }

  override fun onCleared() {
    shell.close()
  }
}
