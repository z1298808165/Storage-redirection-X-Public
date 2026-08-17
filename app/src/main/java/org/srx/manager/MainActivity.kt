package org.srx.manager

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.navigation3.rememberViewModelStoreNavEntryDecorator
import androidx.navigation3.runtime.NavBackStack
import androidx.navigation3.runtime.NavKey
import androidx.navigation3.runtime.entryProvider
import androidx.navigation3.runtime.rememberSaveableStateHolderNavEntryDecorator
import androidx.navigation3.ui.NavDisplay
import androidx.navigationevent.NavigationEventInfo
import androidx.navigationevent.compose.NavigationBackHandler
import androidx.navigationevent.compose.NavigationEventState
import androidx.navigationevent.compose.rememberNavigationEventState
import org.srx.manager.data.AppFilter
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.data.UiPreferences
import org.srx.manager.ui.AppUiState
import org.srx.manager.ui.SrxViewModel
import org.srx.manager.ui.liquid.liveGlassBackgroundLayer
import org.srx.manager.ui.liquid.liveGlassContentLayer
import org.srx.manager.ui.liquid.rememberBlurBackdrop
import org.srx.manager.ui.liquid.rememberLiveGlassBackdropScene
import org.srx.manager.ui.screen.AboutScreen
import org.srx.manager.ui.screen.AppConfigScreen
import org.srx.manager.ui.screen.AppsScreen
import org.srx.manager.ui.screen.DashboardScreen
import org.srx.manager.ui.screen.LogsScreen
import org.srx.manager.ui.screen.SettingsScreen
import org.srx.manager.ui.screen.ThemeSettingsScreen
import org.srx.manager.ui.screen.UpdateFoundDialog
import org.srx.manager.ui.screen.UpdateScreen
import org.srx.manager.ui.theme.SrxTheme
import top.yukonga.miuix.kmp.basic.InfiniteProgressIndicator
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.GridView
import top.yukonga.miuix.kmp.icon.extended.ListView
import top.yukonga.miuix.kmp.icon.extended.Notes
import top.yukonga.miuix.kmp.icon.extended.Settings
import top.yukonga.miuix.kmp.shader.isRenderEffectSupported

class MainActivity : ComponentActivity() {
  private val viewModel: SrxViewModel by viewModels()

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    enableEdgeToEdge()
    setContent {
      val prefs by viewModel.uiPreferences.collectAsStateWithLifecycle()
      val systemDensity = LocalDensity.current
      val pageScale = prefs.pageScale.coerceIn(0.8f, 1.1f)
      val scaledDensity =
          remember(systemDensity, pageScale) {
            Density(systemDensity.density * pageScale, systemDensity.fontScale)
          }
      CompositionLocalProvider(LocalDensity provides scaledDensity) {
        SrxTheme(
            dynamicColor = prefs.dynamicColor,
            accentColor = prefs.accentColor,
            colorStyle = prefs.colorStyle,
            colorSpec = prefs.colorSpec,
            themeMode = prefs.themeMode,
            liquidGlass = prefs.liquidGlass,
            blurEffect = prefs.blurEffect,
        ) {
          SrxManagerApp(viewModel, prefs)
        }
      }
    }
  }
}

internal enum class Page(val label: String, val icon: ImageVector) {
  Dashboard("概览", MiuixIcons.GridView),
  Apps("应用", MiuixIcons.ListView),
  Logs("监视", MiuixIcons.Notes),
  Settings("设置", MiuixIcons.Settings),
}

private sealed interface SrxRoute : NavKey {
  data object Main : SrxRoute

  data object About : SrxRoute

  data object Update : SrxRoute

  data object Theme : SrxRoute

  data class AppConfig(val packageName: String) : SrxRoute
}

private fun toggleSelectedPackage(selected: List<String>, packageName: String): List<String> =
    if (packageName in selected) selected - packageName else selected + packageName

private fun filteredAppPackagesForSelection(state: AppUiState): List<String> {
  val query = state.search.trim().lowercase()
  return state.apps
      .filter { app ->
        val filterOk =
            when (state.filter) {
              AppFilter.User -> !app.isSystem
              AppFilter.System -> app.isSystem
              AppFilter.Configured -> app.isConfigured
            }
        filterOk &&
            (query.isBlank() ||
                app.label.lowercase().contains(query) ||
                app.packageName.lowercase().contains(query))
      }
      .map { it.packageName }
}

private fun openExternalUrl(context: Context, url: String): Boolean =
    try {
      val intent =
          Intent(Intent.ACTION_VIEW, Uri.parse(url)).apply {
            addCategory(Intent.CATEGORY_BROWSABLE)
          }
      context.startActivity(intent)
      true
    } catch (_: ActivityNotFoundException) {
      false
    }

@Composable
private fun SrxManagerApp(
    viewModel: SrxViewModel,
    prefs: UiPreferences,
) {
  val context = LocalContext.current
  val state by viewModel.state.collectAsStateWithLifecycle()
  var page by rememberSaveable { mutableStateOf(Page.Dashboard) }
  var pageHistory by rememberSaveable { mutableStateOf<List<Page>>(emptyList()) }
  var themeRouteOpen by rememberSaveable { mutableStateOf(false) }
  val navBackStack = remember {
    NavBackStack<SrxRoute>(SrxRoute.Main).apply { if (themeRouteOpen) add(SrxRoute.Theme) }
  }
  var showRootDialog by remember { mutableStateOf(false) }
  var toast by remember { mutableStateOf<String?>(null) }
  var pendingRestoreUri by remember { mutableStateOf<Uri?>(null) }
  var selectedAppPackages by rememberSaveable { mutableStateOf<List<String>>(emptyList()) }
  var pendingBatchTemplate by remember { mutableStateOf<ConfigTemplate?>(null) }
  var startupUpdateCheckDone by rememberSaveable { mutableStateOf(false) }
  val appsListState = rememberLazyListState()
  val logsListState = rememberLazyListState()
  val backupExportLauncher =
      rememberLauncherForActivityResult(
          ActivityResultContracts.CreateDocument("application/zip"),
      ) { uri ->
        if (uri != null) viewModel.exportBackupToUri(uri)
      }
  val backupImportLauncher =
      rememberLauncherForActivityResult(
          ActivityResultContracts.OpenDocument(),
      ) { uri ->
        pendingRestoreUri = uri
      }
  val diagnosticExportLauncher =
      rememberLauncherForActivityResult(
          ActivityResultContracts.OpenDocumentTree(),
      ) { uri ->
        if (uri != null) viewModel.exportDiagnosticArchiveToDirectory(uri)
      }

  LaunchedEffect(state.rootChecked, state.rootGranted) {
    if (state.rootChecked && !state.rootGranted) {
      showRootDialog = true
    } else if (state.rootGranted) {
      showRootDialog = false
    }
  }
  val predictiveBackEnabled = prefs.predictiveBack
  LaunchedEffect(state.snackbar) {
    state.snackbar?.let {
      toast = it
      viewModel.clearSnackbar()
    }
  }
  LaunchedEffect(prefs.autoCheckUpdates, prefs.updateChannel, state.dashboard.version) {
    if (!startupUpdateCheckDone && prefs.autoCheckUpdates && state.dashboard.version.isNotBlank()) {
      startupUpdateCheckDone = true
      viewModel.checkForUpdates(manual = false)
    }
  }
  LaunchedEffect(page) {
    if (navBackStack.lastOrNull() == SrxRoute.Main) {
      when (page) {
        Page.Dashboard -> viewModel.refreshDashboardCounts()
        Page.Logs -> viewModel.refreshLogs()
        else -> Unit
      }
    }
  }
  LifecycleEventEffect(Lifecycle.Event.ON_RESUME) {
    if (page == Page.Dashboard && navBackStack.lastOrNull() == SrxRoute.Main) {
      viewModel.refreshDashboardCounts()
    }
  }
  LaunchedEffect(page, navBackStack.size) {
    if (page != Page.Apps || navBackStack.lastOrNull() != SrxRoute.Main)
        selectedAppPackages = emptyList()
  }
  LaunchedEffect(page, state.rootGranted) {
    if (!state.rootGranted) return@LaunchedEffect
    when (page) {
      Page.Apps -> viewModel.ensureAppsLoaded()
      Page.Settings -> viewModel.refreshTemplates()
      else -> Unit
    }
  }

  val blurAvailable = isRenderEffectSupported()
  val backdropEnabled = blurAvailable && (prefs.blurEffect || prefs.liquidGlass)
  val blurBackdrop = rememberBlurBackdrop(prefs.blurEffect && blurAvailable)
  val glassScene = rememberLiveGlassBackdropScene(enabled = backdropEnabled)
  val backEventState = rememberNavigationEventState(NavigationEventInfo.None)
  fun popNavBackStack() {
    if (navBackStack.size > 1) {
      val top = navBackStack.last()
      navBackStack.removeAt(navBackStack.lastIndex)
      if (top == SrxRoute.Theme) themeRouteOpen = false
      if (top is SrxRoute.AppConfig) {
        viewModel.closeAppConfig()
      }
    }
  }
  fun pushRoute(route: SrxRoute) {
    if (navBackStack.lastOrNull() != route) {
      navBackStack.add(route)
      if (route == SrxRoute.Theme) themeRouteOpen = true
    }
  }
  fun navigateToPage(target: Page) {
    if (target != page) {
      val existingIndex = pageHistory.lastIndexOf(target)
      pageHistory =
          if (existingIndex >= 0) {
            pageHistory.take(existingIndex)
          } else {
            pageHistory + page
          }
      page = target
    }
    while (navBackStack.size > 1) {
      navBackStack.removeAt(navBackStack.lastIndex)
    }
    themeRouteOpen = false
  }
  fun popPageHistory() {
    val previous = pageHistory.lastOrNull() ?: Page.Dashboard
    pageHistory = pageHistory.dropLast(1)
    page = previous
  }
  val navBackAction: (() -> Unit)? =
      if (navBackStack.size > 1) {
        { popNavBackStack() }
      } else {
        null
      }
  val pageBackAction: (() -> Unit)? =
      if (navBackStack.size == 1 && page != Page.Dashboard) {
        { popPageHistory() }
      } else {
        null
      }
  val classicBackAction = navBackAction ?: pageBackAction

  LaunchedEffect(state.currentApp?.packageName) {
    val packageName = state.currentApp?.packageName
    if (packageName == null) {
      if (navBackStack.lastOrNull() is SrxRoute.AppConfig) {
        popNavBackStack()
      }
    } else if (navBackStack.lastOrNull() !is SrxRoute.AppConfig) {
      pushRoute(SrxRoute.AppConfig(packageName))
    }
  }

  SrxBackHandler(
      enabled = !state.busy && pageBackAction != null,
      predictiveEnabled = predictiveBackEnabled,
      state = backEventState,
  ) {
    pageBackAction?.invoke()
  }
  BackHandler(enabled = !state.busy && !predictiveBackEnabled && classicBackAction != null) {
    classicBackAction?.invoke()
  }
  BackHandler(enabled = !state.busy && selectedAppPackages.isNotEmpty()) {
    selectedAppPackages = emptyList()
  }
  BackHandler(enabled = state.busy) {
    // 忙碌操作会重启系统进程，因此在操作完成前保留当前任务弹窗。
  }

  Scaffold(
      containerColor = Color.Transparent,
      bottomBar = {
        Box(Modifier.fillMaxWidth()) {
          if (navBackStack.lastOrNull() == SrxRoute.Main) {
            if (page == Page.Apps && selectedAppPackages.isNotEmpty()) {
              AppBatchActionBar(
                  selectedCount = selectedAppPackages.size,
                  enabled = !state.busy,
                  templates = state.templates,
                  onApplyTemplate = { pendingBatchTemplate = it },
                  onSelectAll = { selectedAppPackages = filteredAppPackagesForSelection(state) },
                  onCancel = { selectedAppPackages = emptyList() },
                  prefs = prefs,
                  blurBackdrop = blurBackdrop,
                  backdrop = glassScene.backdrop,
                  dialogBackdrop = glassScene.activeBackdrop,
              )
            } else {
              BottomNavigation(
                  page = page,
                  onPageChange = ::navigateToPage,
                  enabled = !state.busy,
                  prefs = prefs,
                  blurBackdrop = blurBackdrop,
                  backdrop = glassScene.backdrop,
              )
            }
          }
          if (state.busy) {
            Box(
                modifier =
                    Modifier.matchParentSize()
                        .background(busyScrimColor())
                        .clickable(
                            interactionSource = remember { MutableInteractionSource() },
                            indication = null,
                            onClick = {},
                        ),
            )
          }
        }
      },
  ) { padding ->
    @Composable
    fun PageContent(targetPage: Page) {
      when (targetPage) {
        Page.Dashboard ->
            DashboardScreen(
                state = state,
                bottomPadding = padding.calculateBottomPadding(),
                onToggleModule = { viewModel.setModuleEnabled(it) },
                onRestartMediaProvider = viewModel::restartMediaProvider,
                onResetRuntimeStats = viewModel::resetRuntimeStats,
                onOpenAbout = { pushRoute(SrxRoute.About) },
                onOpenUpdate = { pushRoute(SrxRoute.Update) },
            )

        Page.Apps ->
            AppsScreen(
                state = state,
                listState = appsListState,
                bottomPadding = padding.calculateBottomPadding(),
                selectedPackages = selectedAppPackages.toSet(),
                onRefresh = { viewModel.refreshApps(force = true) },
                onSearch = viewModel::setSearch,
                onFilter = viewModel::setFilter,
                onUser = viewModel::selectUser,
                onOpenApp = { app ->
                  if (selectedAppPackages.isNotEmpty()) {
                    selectedAppPackages =
                        toggleSelectedPackage(selectedAppPackages, app.packageName)
                  } else {
                    viewModel.openApp(app)
                    pushRoute(SrxRoute.AppConfig(app.packageName))
                  }
                },
                onLongPressApp = { app ->
                  selectedAppPackages =
                      toggleSelectedPackage(selectedAppPackages, app.packageName).ifEmpty {
                        listOf(app.packageName)
                      }
                },
            )

        Page.Logs ->
            LogsScreen(
                state = state,
                logs = state.logs,
                apps =
                    remember(state.apps, state.logApps) {
                      (state.logApps.associateBy { it.packageName } +
                              state.apps.associateBy { it.packageName })
                          .values
                          .toList()
                    },
                listState = logsListState,
                bottomPadding = padding.calculateBottomPadding(),
                onRefresh = viewModel::refreshLogs,
                onClear = viewModel::clearLogs,
                onOpenApp = { app ->
                  viewModel.openApp(app)
                  pushRoute(SrxRoute.AppConfig(app.packageName))
                },
                onSaveFilters = { filters, silent ->
                  viewModel.saveFileMonitorFilters(filters, silent)
                },
            )

        Page.Settings ->
            SettingsScreen(
                state = state,
                prefs = prefs,
                bottomPadding = padding.calculateBottomPadding(),
                onGlobal = viewModel::updateGlobalConfig,
                onSaveTemplate = viewModel::saveTemplate,
                onDeleteTemplate = viewModel::deleteTemplate,
                onOpenTheme = { pushRoute(SrxRoute.Theme) },
                onBackupExport = { backupExportLauncher.launch(viewModel.backupFileName()) },
                onBackupImport = {
                  backupImportLauncher.launch(
                      arrayOf("application/zip", "application/json", "text/*", "*/*")
                  )
                },
                onDiagnosticExport = { diagnosticExportLauncher.launch(null) },
                onListDirectories = viewModel::listStorageDirectories,
            )
      }
    }

    Box(modifier = Modifier.fillMaxSize()) {
      Box(
          modifier = Modifier.fillMaxSize().appMeshBackground().liveGlassBackgroundLayer(glassScene)
      )
      Box(modifier = Modifier.fillMaxSize()) {
        CompositionLocalProvider(LocalSrxBackdrop provides glassScene.activeBackgroundBackdrop) {
          Box(
              modifier = Modifier.fillMaxSize().liveGlassContentLayer(glassScene),
          ) {
            NavDisplay(
                backStack = navBackStack,
                modifier = Modifier.fillMaxSize(),
                entryDecorators =
                    listOf(
                        rememberSaveableStateHolderNavEntryDecorator(),
                        rememberViewModelStoreNavEntryDecorator(),
                    ),
                onBack = { if (!state.busy) popNavBackStack() },
                entryProvider =
                    entryProvider {
                      entry<SrxRoute.Main> { PageContent(page) }
                      entry<SrxRoute.About> {
                        AboutScreen(
                            onBack = ::popNavBackStack,
                        )
                      }
                      entry<SrxRoute.Update> {
                        UpdateScreen(
                            prefs = prefs,
                            moduleVersion = state.dashboard.version,
                            updateCheckRunning = state.updateCheckRunning,
                            onBack = ::popNavBackStack,
                            onAutoCheckUpdates = viewModel::setAutoCheckUpdates,
                            onUpdateChannel = viewModel::setUpdateChannel,
                            onCheckNow = { viewModel.checkForUpdates(manual = true) },
                        )
                      }
                      entry<SrxRoute.Theme> {
                        ThemeSettingsScreen(
                            prefs = prefs,
                            onBack = ::popNavBackStack,
                            onFloating = viewModel::setFloatingBottomBar,
                            onLiquid = viewModel::setLiquidGlass,
                            onBlurEffect = viewModel::setBlurEffect,
                            onDynamicColor = viewModel::setDynamicColor,
                            onAccentColor = viewModel::setAccentColor,
                            onColorStyle = viewModel::setColorStyle,
                            onColorSpec = viewModel::setColorSpec,
                            onThemeMode = viewModel::setThemeMode,
                            onPredictiveBack = { enabled ->
                              themeRouteOpen = true
                              startupUpdateCheckDone = true
                              viewModel.setPredictiveBack(enabled)
                              syncPredictiveBackEnabled(context.applicationInfo, enabled)
                              (context as? ComponentActivity)?.recreate()
                            },
                            onPageScale = viewModel::setPageScale,
                        )
                      }
                      entry<SrxRoute.AppConfig> {
                        val currentApp = state.currentApp
                        val currentConfig = state.currentConfig
                        if (
                            currentApp != null &&
                                currentConfig != null &&
                                currentApp.packageName == it.packageName
                        ) {
                          AppConfigScreen(
                              state = state,
                              app = currentApp,
                              config = currentConfig,
                              prefs = prefs,
                              onBack = ::popNavBackStack,
                              onSave = { viewModel.saveCurrentConfig() },
                              onDelete = viewModel::deleteCurrentConfig,
                              onSaveTemplate = viewModel::saveCurrentConfigAsTemplate,
                              onApplyTemplate = viewModel::applyTemplateToCurrentApp,
                              onProfileChange = viewModel::updateProfile,
                              onAddAllowed = viewModel::addAllowedPath,
                              onAddSandbox = viewModel::addSandboxPath,
                              onUpdateAllowed = viewModel::updateAllowedPath,
                              onUpdateSandbox = viewModel::updateSandboxPath,
                              onRemoveAllowed = viewModel::removeAllowedPath,
                              onRemoveSandbox = viewModel::removeSandboxPath,
                              onSetReadOnlyEnabled = viewModel::setReadOnlyEnabled,
                              onAddReadOnly = viewModel::addReadOnlyPath,
                              onUpdateReadOnly = viewModel::updateReadOnlyPath,
                              onRemoveReadOnly = viewModel::removeReadOnlyPath,
                              onAddMapping = viewModel::addMapping,
                              onUpdateMapping = viewModel::updateMapping,
                              onRemoveMapping = viewModel::removeMapping,
                              onListDirectories = viewModel::listStorageDirectories,
                          )
                        } else {
                          Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                            InfiniteProgressIndicator()
                          }
                        }
                      }
                    },
            )
          }
        }

        CompositionLocalProvider(LocalSrxBackdrop provides glassScene.activeBackdrop) {
          AnimatedVisibility(
              visible = state.busy,
              modifier = Modifier.fillMaxSize(),
              enter = fadeIn(),
              exit = fadeOut(),
          ) {
            Box(
                modifier =
                    Modifier.fillMaxSize()
                        .background(busyScrimColor())
                        .clickable(
                            interactionSource = remember { MutableInteractionSource() },
                            indication = null,
                            onClick = {},
                        ),
                contentAlignment = Alignment.Center,
            ) {
              BusyOverlay(state.busyMessage ?: "处理中", state.busyProgress)
            }
          }

          toast?.let {
            LaunchedEffect(it) {
              kotlinx.coroutines.delay(2200)
              toast = null
            }
            ToastPill(
                it,
                Modifier.align(Alignment.BottomCenter)
                    .padding(bottom = padding.calculateBottomPadding() + 18.dp),
            )
          }

          CenteredDialog(
              title = "需要 root 权限",
              summary = "存储重定向X 管理应用需要读取和修改模块配置。请在 root 管理器里授予本应用 root 权限后重新检查。",
              show = showRootDialog,
              onDismiss = { showRootDialog = false },
          ) {
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
              GlassTextButton("稍后", { showRootDialog = false }, modifier = Modifier.weight(1f))
              GlassTextButton(
                  "重新检查",
                  { viewModel.refreshRootAndAll() },
                  modifier = Modifier.weight(1f),
                  primary = true,
              )
            }
          }

          CenteredDialog(
              title = "还原配置",
              summary = "将用备份覆盖当前全局设置、全部应用配置、外观偏好和检查更新设置。还原后建议重启相关应用或媒体进程。",
              show = pendingRestoreUri != null,
              onDismiss = { pendingRestoreUri = null },
          ) {
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
              GlassTextButton("取消", { pendingRestoreUri = null }, modifier = Modifier.weight(1f))
              GlassTextButton(
                  "还原",
                  {
                    pendingRestoreUri?.let(viewModel::restoreBackupFromUri)
                    pendingRestoreUri = null
                  },
                  modifier = Modifier.weight(1f),
                  primary = true,
              )
            }
          }

          pendingBatchTemplate?.let { template ->
            CenteredDialog(
                title = "批量应用模板",
                summary = "将模板“${template.name}”应用到 ${selectedAppPackages.size} 个应用，现有配置会被覆盖。",
                show = true,
                onDismiss = { pendingBatchTemplate = null },
            ) {
              Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                GlassTextButton(
                    "取消",
                    { pendingBatchTemplate = null },
                    modifier = Modifier.weight(1f),
                )
                GlassTextButton(
                    "应用",
                    {
                      viewModel.applyTemplateToApps(template.id, selectedAppPackages)
                      selectedAppPackages = emptyList()
                      pendingBatchTemplate = null
                    },
                    modifier = Modifier.weight(1f),
                    primary = true,
                )
              }
            }
          }

          state.pendingUpdate?.let { update ->
            val currentModuleVersion = state.dashboard.version.ifBlank { "--" }
            UpdateFoundDialog(
                update = update,
                currentVersion = currentModuleVersion,
                onDismiss = viewModel::clearPendingUpdate,
                onOpen = {
                  val opened = openExternalUrl(context, update.htmlUrl)
                  if (!opened) toast = "未找到可用浏览器"
                  viewModel.clearPendingUpdate()
                },
            )
          }
        }
      }
    }
  }
}

@Composable
private fun <T : NavigationEventInfo> SrxBackHandler(
    enabled: Boolean,
    predictiveEnabled: Boolean,
    state: NavigationEventState<T>,
    onBack: () -> Unit,
) {
  val currentOnBack by rememberUpdatedState(onBack)
  NavigationBackHandler(
      state = state,
      isBackEnabled = enabled && predictiveEnabled,
      onBackCompleted = { currentOnBack() },
  )
}
