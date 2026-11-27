package org.srx.manager

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.PixelCopy
import android.view.Window
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.PagerDefaults
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.navigationevent.NavigationEventInfo
import androidx.navigationevent.compose.NavigationBackHandler
import androidx.navigationevent.compose.NavigationEventState
import androidx.navigationevent.compose.rememberNavigationEventState
import com.kyant.backdrop.backdrops.layerBackdrop as kyantLayerBackdrop
import com.kyant.backdrop.backdrops.rememberCombinedBackdrop as rememberKyantCombinedBackdrop
import com.kyant.backdrop.backdrops.rememberLayerBackdrop as rememberKyantLayerBackdrop
import io.github.suqi8.coui.kmp.basic.InfiniteProgressIndicator as CouiInfiniteProgressIndicator
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable
import org.srx.manager.data.AppFilter
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.data.UiPreferences
import org.srx.manager.data.UiThemeMode
import org.srx.manager.data.UiThemeStyle
import org.srx.manager.ui.AppUiState
import org.srx.manager.ui.SrxViewModel
import org.srx.manager.ui.coui.CouiAppBatchActionBar
import org.srx.manager.ui.coui.SrxRootScaffold
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
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.GridView
import top.yukonga.miuix.kmp.icon.glass.ListView
import top.yukonga.miuix.kmp.icon.glass.NotesOutline
import top.yukonga.miuix.kmp.icon.glass.SettingsOutline
import top.yukonga.miuix.kmp.material.isRenderEffectSupported
import top.yukonga.miuix.kmp.nav.core.NavBackStack
import top.yukonga.miuix.kmp.nav.core.NavDisplay
import top.yukonga.miuix.kmp.nav.core.NavKey
import top.yukonga.miuix.kmp.nav.core.rememberNavBackStack
import top.yukonga.miuix.kmp.nav.transition.NavSwipeDirection
import top.yukonga.miuix.kmp.nav.transition.NavTransitions
import top.yukonga.miuix.kmp.utils.PagerGestureNestedScrollConnection
import top.yukonga.miuix.kmp.utils.PagerInterceptionMode
import top.yukonga.miuix.kmp.utils.PagerNavigationSpringSpec
import top.yukonga.miuix.kmp.utils.pagerGestureOverride
import top.yukonga.miuix.kmp.utils.springAnimateToPage

class MainActivity : ComponentActivity() {
  private val viewModel: SrxViewModel by viewModels()

  private companion object {
    /** 自检入口的 intent extra 名与取值。 */
    const val DIAG_EXTRA = "srx_diag"
    const val DIAG_STORAGE_SHAPE = "storage_shape"
  }

  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    // 零 UI 自检入口：`am start … --es srx_diag storage_shape` 只打印存储形态后立即退出，
    // 不进入界面。用途见 StorageShapeDiag（应用域才能量出真实视角）。
    if (intent?.getStringExtra(DIAG_EXTRA) == DIAG_STORAGE_SHAPE) {
      StorageShapeDiag.run()
      finish()
      return
    }
    enableEdgeToEdge()
    setContent {
      val prefs by viewModel.uiPreferences.collectAsStateWithLifecycle()
      val systemDensity = LocalDensity.current
      val pageScale = prefs.pageScale.coerceIn(0.8f, 1.1f)
      val scaledDensity =
          remember(systemDensity, pageScale) {
            Density(systemDensity.density * pageScale, systemDensity.fontScale)
          }
      // 导航状态提升到主题分支之外：切换 COUI/Miuix 会 dispose 整个 SrxManagerApp
      // 组合子树，若状态留在其中，remember/rememberSaveable 都会重置导致回到概览页。
      // 提到这里由稳定的 setContent 槽位持有，切换主题后原样恢复当前页面。
      val navState = rememberSrxNavState()
      // 滚动位置缓存：与导航状态同处主题之外，切换主题/深浅色后按 key 取回。
      val scrollCache = remember { SrxScrollCache() }
      // 主题切换会 dispose 内容子树，用 SaveableStateHolder 托管 rememberSaveable 状态。
      val saveableHolder = androidx.compose.runtime.saveable.rememberSaveableStateHolder()
      // 主题切换过渡必须放在 SrxTheme 之外：SrxTheme 的 when(themeStyle) 分支切换会
      // dispose 其 content 子树，若过渡层在里面，generation/lastStyle 每次都被重置，
      // 动画永远播不出来。放在这里由稳定槽位持有，遮罩覆盖整个应用。
      val reveal = remember { ThemeRevealSnapshot() }
      // 与 Eta 同法：把整棵内容树做成 movable content，切换主题时是"移动"而不是
      // "销毁重建"，页面滚动位置、输入草稿、展开状态全部原样保留。
      val currentPrefs by androidx.compose.runtime.rememberUpdatedState(prefs)
      val currentViewModel by androidx.compose.runtime.rememberUpdatedState(viewModel)
      val appContent = remember {
        androidx.compose.runtime.movableContentOf {
          SrxManagerApp(currentViewModel, currentPrefs, navState, reveal)
        }
      }
      ThemeSwitchTransition(reveal = reveal) {
        CompositionLocalProvider(
            LocalDensity provides scaledDensity,
            LocalSrxScrollCache provides scrollCache,
            org.srx.manager.ui.theme.LocalSrxSaveableStateHolder provides saveableHolder,
        ) {
          SrxTheme(
              dynamicColor = prefs.dynamicColor,
              accentColor = prefs.accentColor,
              colorStyle = prefs.colorStyle,
              colorSpec = prefs.colorSpec,
              themeMode = prefs.themeMode,
              themeStyle = prefs.themeStyle,
              liquidGlass = prefs.liquidGlass,
              blurEffect = prefs.blurEffect,
          ) {
            appContent()
          }
        }
      }
    }
  }
}

/**
 * 主题切换用的旧界面快照 + 揭示请求：切换前把当前窗口画面抓成 Bitmap，切换后用不断扩大的 透明圆把它挖掉，圆内即逐步显示切换后的目标主题。
 *
 * [requestId] 是"要播一次揭示"的显式信号：主题模式在明暗不变时（如 跟随系统(浅) → 浅色） 只改配置、不播动画，所以不能靠 prefs 变化推断，必须由调用方主动加一。
 *
 * 抓帧用 PixelCopy 而不是 View.draw()：Compose 内容走 RenderNode 硬件绘制，画到软件 Canvas 上常常是空白（实测只有兜底灰底），PixelCopy
 * 读的是窗口 surface。
 */
private class ThemeRevealSnapshot {
  var frame: ImageBitmap? = null

  /** 每次需要播揭示动画时自增；ThemeSwitchTransition 以此为 key。 */
  var requestId by mutableStateOf(0)

  /**
   * 异步抓取窗口画面，完成后回调 [onReady]——调用方必须在回调里才改主题，否则抓到的可能是 已经换成新主题的画面（动画会"自己盖自己"）。
   *
   * [delayMs] 用于避开入口弹窗/对话框自己的关闭动画与遮罩：抓早了，Reveal 扫走的是 "带弹窗的旧画面"，观感很脏。分段控件这类无弹窗的入口传 0 即可。
   */
  fun capture(window: Window, delayMs: Long, onReady: () -> Unit) {
    if (delayMs <= 0L) {
      captureCurrentFrame(window, onReady)
    } else {
      Handler(Looper.getMainLooper()).postDelayed({ captureCurrentFrame(window, onReady) }, delayMs)
    }
  }

  private fun captureCurrentFrame(window: Window, onReady: () -> Unit) {
    val root = window.decorView
    val width = root.width
    val height = root.height
    val bitmap =
        if (width > 0 && height > 0) {
          runCatching { Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888) }.getOrNull()
        } else {
          null
        }
    if (bitmap == null) {
      frame = null
      onReady()
      return
    }
    val requested = runCatching {
      PixelCopy.request(
          window,
          bitmap,
          { result ->
            if (result == PixelCopy.SUCCESS) {
              frame = bitmap.asImageBitmap()
            } else {
              Log.w(TAG, "theme reveal: PixelCopy failed, result=$result")
              frame = null
            }
            onReady()
          },
          Handler(Looper.getMainLooper()),
      )
    }
    if (requested.isFailure) {
      Log.w(TAG, "theme reveal: PixelCopy request threw", requested.exceptionOrNull())
      frame = null
      onReady()
    }
  }

  /** 取走快照，避免同一帧被重复用于下一次切换。 */
  fun take(): ImageBitmap? = frame.also { frame = null }
}

private const val TAG = "SrxManager"

/** 弹窗/对话框类入口要等它的关闭动画走完再抓帧。 */
private const val REVEAL_CAPTURE_DELAY_MS = 240L

/** 揭示时长：慢起、匀速展开，让"从右上角一个小圆点扩到全屏"看得清。 */
private const val THEME_REVEAL_DURATION_MS = 760

/**
 * 揭示过渡：以右上角为圆心，透明圆半径从小扩到覆盖整屏对角线，圆的边界扫过之处旧画面 被抹去、露出切换后的目标主题；边界外再叠几圈细的水波环，看起来像涟漪扩散。
 *
 * 关键点：本组件必须位于 SrxTheme 之外（见 setContent 调用处），否则 when(themeStyle) 切换会 dispose 它的子树，动画永远不播。
 */
@Composable
private fun ThemeSwitchTransition(
    reveal: ThemeRevealSnapshot,
    content: @Composable () -> Unit,
) {
  // content 始终留在同一组合槽位，换主题只重组内容、不重建导航栈。
  content()
  val requestId = reveal.requestId
  if (requestId > 0) {
    androidx.compose.runtime.key(requestId) {
      val frame = remember { reveal.take() }
      val progress = remember { Animatable(0f) }
      var finished by remember { mutableStateOf(false) }
      // 抓帧失败时的兜底色：仍要能看出"有东西被圆扫掉"，故用与页面有反差的颜色。
      val fallbackColor =
          if (org.srx.manager.ui.theme.isSrxDarkTheme()) Color(0xFF14141C) else Color(0xFFF2F2F7)
      LaunchedEffect(Unit) {
        progress.animateTo(
            1f,
            tween(
                THEME_REVEAL_DURATION_MS,
                // 慢起：前段只扩出一个小圆，避免"圆心一上来就很大"。
                easing = androidx.compose.animation.core.CubicBezierEasing(0.32f, 0f, 0.12f, 1f),
            ),
        )
        finished = true
      }
      if (!finished) {
        Box(
            Modifier.fillMaxSize()
                .revealMask(progress = progress, frame = frame, fallback = fallbackColor)
        )
      }
    }
  }
}

/**
 * 旧画面 + 右上角扩大的透明圆 + 边界外的水波环：整个 Box 走离屏合成（Offscreen），这样 BlendMode.Clear
 * 只清掉本图层里的旧画面、不会把下面的新主题一起清空；圆外仍是旧画面， 圆内透出新主题。
 */
private fun Modifier.revealMask(
    progress: Animatable<Float, *>,
    frame: ImageBitmap?,
    fallback: Color,
): Modifier =
    this.graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
        .drawWithContent {
          val fraction = progress.value.coerceIn(0f, 1f)
          if (frame != null) {
            drawImage(frame)
          } else {
            drawRect(fallback)
          }
          // 旧画面随揭示进度轻微压暗：两套主题共用同一份动态配色，卡片样式差异带来的
          // 明暗对比很弱，不加这层几乎看不见扫过的边界；从 0 渐入所以起始帧无跳变。
          drawRect(Color.Black.copy(alpha = 0.18f * fraction))
          // 半径取屏幕对角线，保证圆最终覆盖离圆心最远的左下角。
          val maxRadius = kotlin.math.hypot(size.width, size.height)
          val center = Offset(size.width, 0f)
          val radius = maxRadius * fraction
          drawCircle(
              color = Color.Black,
              radius = radius,
              center = center,
              blendMode = BlendMode.Clear,
          )
          // 边界外的水波：几圈由内向外变淡的细白环，随揭示一起外移并在收尾淡出。
          // 环画在圆外（旧画面一侧），读起来就是涟漪在推着边界往前走。
          val ringCount = 4
          val strokeBase = 2.0.dp.toPx()
          for (index in 1..ringCount) {
            val ringAlpha =
                (0.34f - 0.065f * (index - 1)) * (1f - fraction).coerceIn(0f, 1f) * 1.15f
            if (ringAlpha <= 0.004f) continue
            val spread = maxRadius * (0.012f * index) * (0.55f + fraction * 0.85f)
            drawCircle(
                color = Color.White.copy(alpha = ringAlpha.coerceIn(0f, 1f)),
                radius = radius + spread,
                center = center,
                style =
                    androidx.compose.ui.graphics.drawscope.Stroke(
                        width = strokeBase * (1f - 0.12f * (index - 1)).coerceAtLeast(0.5f)
                    ),
            )
          }
        }

/** 主题模式的"实际明暗"，用于判断切换是否真的改变了外观。 */
private fun UiThemeMode.isDark(systemDark: Boolean): Boolean =
    when (this) {
      UiThemeMode.Light -> false
      UiThemeMode.Dark -> true
      UiThemeMode.System -> systemDark
    }

internal enum class Page(val label: String, val icon: ImageVector) {
  Dashboard("概览", MiuixIcons.Glass.GridView),
  Apps("应用", MiuixIcons.Glass.ListView),
  Logs("监视", MiuixIcons.Glass.NotesOutline),
  Settings("设置", MiuixIcons.Glass.SettingsOutline),
}

// miuix-nav 的回栈依赖 kotlinx.serialization 做状态恢复，路由必须可序列化。
@Serializable
private sealed interface SrxRoute : NavKey {
  @Serializable data object Main : SrxRoute

  @Serializable data object About : SrxRoute

  @Serializable data object Update : SrxRoute

  @Serializable data object Theme : SrxRoute

  @Serializable data class AppConfig(val packageName: String) : SrxRoute
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

/**
 * 跨主题切换存活的导航状态。COUI/Miuix 切换会 dispose 整个 SrxManagerApp 子树， 这些状态必须由主题分支之外的稳定组合槽位（setContent）持有。持有
 * MutableState 引用即可天然双向同步：组合内写入的值，切主题后重建子树读到的还是同一对象。
 */
private class SrxNavState(
    val page: androidx.compose.runtime.MutableState<Page>,
    val pageHistory: androidx.compose.runtime.MutableState<List<Page>>,
    val themeRouteOpen: androidx.compose.runtime.MutableState<Boolean>,
    val selectedAppPackages: androidx.compose.runtime.MutableState<List<String>>,
    val startupUpdateCheckDone: androidx.compose.runtime.MutableState<Boolean>,
    val backStack: NavBackStack,
)

@Composable
private fun rememberSrxNavState(): SrxNavState {
  val page = rememberSaveable { mutableStateOf(Page.Dashboard) }
  val pageHistory = rememberSaveable { mutableStateOf<List<Page>>(emptyList()) }
  val themeRouteOpen = rememberSaveable { mutableStateOf(false) }
  val selected = rememberSaveable { mutableStateOf<List<String>>(emptyList()) }
  val updateDone = rememberSaveable { mutableStateOf(false) }
  val backStack = rememberNavBackStack<SrxRoute>(SrxRoute.Main)
  return remember {
    SrxNavState(page, pageHistory, themeRouteOpen, selected, updateDone, backStack)
  }
}

@Composable
private fun SrxManagerApp(
    viewModel: SrxViewModel,
    prefs: UiPreferences,
    navState: SrxNavState,
    reveal: ThemeRevealSnapshot,
) {
  val context = LocalContext.current
  val systemDark = isSystemInDarkTheme()
  // 统一入口：抓一帧旧画面 → 改配置 → 触发揭示。主题风格入口是弹窗/对话框，要等它自己
  // 关闭再抓（delayMs=REVEAL_CAPTURE_DELAY_MS）；主题模式入口是分段控件，直接抓（0）。
  val applyThemeChangeWithReveal: (Long, () -> Unit) -> Unit = { delayMs, apply ->
    val activityWindow = (context as? ComponentActivity)?.window
    if (activityWindow == null) {
      apply()
    } else {
      reveal.capture(activityWindow, delayMs) {
        // 先让遮罩上屏并开始计时，再改配置：换主题会触发整棵树重组（几十到上百毫秒），
        // 若等重组完才出遮罩，观感是"点完先卡一下，然后才哗地扫过去"。
        reveal.requestId++
        apply()
      }
    }
  }
  val scope = rememberCoroutineScope()
  val state by viewModel.state.collectAsStateWithLifecycle()
  var page by navState.page
  var pageHistory by navState.pageHistory
  var themeRouteOpen by navState.themeRouteOpen
  val mainPagerState =
      rememberPagerState(initialPage = page.ordinal, pageCount = { Page.entries.size })
  val pagerFlingBehavior =
      PagerDefaults.flingBehavior(
          state = mainPagerState,
          snapAnimationSpec = PagerNavigationSpringSpec,
      )
  val layoutDirection = LocalLayoutDirection.current
  val swipeBackDirection =
      if (layoutDirection == LayoutDirection.Rtl) {
        NavSwipeDirection.RightToLeft
      } else {
        NavSwipeDirection.LeftToRight
      }
  val swipeDismiss =
      if (!state.busy && prefs.swipeBackGesture) swipeBackDirection else NavSwipeDirection.None
  val pageGestureEnabled = !state.busy && prefs.pageSwipeGesture
  val pagerGestureMode =
      if (prefs.pageSwipeGesture) PagerInterceptionMode.CrossAxis else PagerInterceptionMode.Native
  val interceptPagerGestures = pagerGestureMode == PagerInterceptionMode.CrossAxis
  val pagerGestureModifier =
      Modifier.pagerGestureOverride(
          pagerState = mainPagerState,
          flingBehavior = pagerFlingBehavior,
          mode = pagerGestureMode,
          enabled = pageGestureEnabled,
      )
  val navBackStack = navState.backStack
  // 进程恢复后同步一次主题路由：栈里已有 Theme 时只回读开关，不得重复入栈
  // （miuix-nav 的 contentKey 去重会对重复路由直接抛异常）。
  LaunchedEffect(Unit) {
    when {
      themeRouteOpen && navBackStack.lastOrNull() != SrxRoute.Theme ->
          navBackStack.add(SrxRoute.Theme)
      !themeRouteOpen && navBackStack.lastOrNull() == SrxRoute.Theme -> themeRouteOpen = true
    }
  }
  var showRootDialog by remember { mutableStateOf(false) }
  var toast by remember { mutableStateOf<String?>(null) }
  var pendingRestoreUri by remember { mutableStateOf<Uri?>(null) }
  var selectedAppPackages by navState.selectedAppPackages
  var pendingBatchTemplate by remember { mutableStateOf<ConfigTemplate?>(null) }
  var startupUpdateCheckDone by navState.startupUpdateCheckDone
  val appsListState = srxRememberListState("apps")
  val logsListState = srxRememberListState("logs")
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
        Page.Dashboard -> viewModel.refreshDashboard()
        Page.Logs -> viewModel.refreshLogs()
        else -> Unit
      }
    }
  }
  LifecycleEventEffect(Lifecycle.Event.ON_RESUME) {
    if (page == Page.Dashboard && navBackStack.lastOrNull() == SrxRoute.Main) {
      viewModel.refreshDashboard()
    }
  }
  LaunchedEffect(page, navBackStack.size) {
    if (page != Page.Apps || navBackStack.lastOrNull() != SrxRoute.Main)
        selectedAppPackages = emptyList()
  }
  // 切换 Android 用户后应用列表整体换了一份，批量选择与滚动位置都属于旧用户，必须重置。
  // 这里无条件滚到顶部：清空列表时 LazyListState 会保留旧索引，数据回填后会停在原位。
  LaunchedEffect(state.selectedUser) {
    selectedAppPackages = emptyList()
    appsListState.scrollToItem(0)
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
  val kyantEnabled = prefs.liquidGlass && blurAvailable
  val kyantBackgroundBackdrop = rememberKyantLayerBackdrop()
  val kyantContentBackdrop = rememberKyantLayerBackdrop()
  val kyantBackdrop = rememberKyantCombinedBackdrop(kyantBackgroundBackdrop, kyantContentBackdrop)
  val activeKyantBackdrop = kyantBackdrop.takeIf { kyantEnabled }
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
      scope.launch { mainPagerState.springAnimateToPage(target.ordinal) }
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
    scope.launch { mainPagerState.springAnimateToPage(previous.ordinal) }
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

  // 手势切换页面时 pager 要滑完才落定（settledPage），底栏高亮会明显滞后；
  // 这里用 currentPage 在滑动过程中即时跟随，静止后再与 page 对齐。
  val barPageIndex =
      if (mainPagerState.isScrollInProgress) mainPagerState.currentPage else page.ordinal

  LaunchedEffect(mainPagerState.settledPage) {
    val target = Page.entries[mainPagerState.settledPage]
    if (target != page) {
      val existingIndex = pageHistory.lastIndexOf(target)
      pageHistory = if (existingIndex >= 0) pageHistory.take(existingIndex) else pageHistory + page
      page = target
      while (navBackStack.size > 1) navBackStack.removeAt(navBackStack.lastIndex)
      themeRouteOpen = false
    }
  }

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
      enabled = !state.busy && classicBackAction != null,
      predictiveEnabled = predictiveBackEnabled,
      state = backEventState,
  ) {
    classicBackAction?.invoke()
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

  SrxRootScaffold(
      useCoui = prefs.themeStyle == UiThemeStyle.Coui,
      bottomBar = {
        Box(Modifier.fillMaxWidth()) {
          if (navBackStack.lastOrNull() == SrxRoute.Main) {
            val coui = prefs.themeStyle == UiThemeStyle.Coui
            if (page == Page.Apps && selectedAppPackages.isNotEmpty()) {
              if (coui) {
                CouiAppBatchActionBar(
                    selectedCount = selectedAppPackages.size,
                    enabled = !state.busy,
                    templates = state.templates,
                    onApplyTemplate = { pendingBatchTemplate = it },
                    onSelectAll = { selectedAppPackages = filteredAppPackagesForSelection(state) },
                    onCancel = { selectedAppPackages = emptyList() },
                    prefs = prefs,
                    backdrop = glassScene.backdrop,
                    kyantBackdrop = activeKyantBackdrop,
                )
              } else {
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
                    kyantBackdrop = activeKyantBackdrop,
                )
              }
            } else if (!state.busy) {
              BottomNavigation(
                  page = page,
                  barSelectedIndex = barPageIndex,
                  onPageChange = ::navigateToPage,
                  enabled = !state.busy,
                  prefs = prefs,
                  blurBackdrop = blurBackdrop,
                  backdrop = glassScene.backdrop,
                  kyantBackdrop = activeKyantBackdrop,
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
                groupedByApp = prefs.logsGroupedByApp,
                onToggleGrouped = { viewModel.setLogsGroupedByApp(!prefs.logsGroupedByApp) },
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
                onDiagnosticExport = {
                  // 不给初始位置时 DocumentsUI 会落在「最近」，那一页要查 MediaProvider 的
                  // 最近文件（实测 1.0–1.6s），目录迟迟列不出来。直接开在 Download。
                  diagnosticExportLauncher.launch(
                      Uri.parse(
                          "content://com.android.externalstorage.documents/document/primary%3ADownload"
                      )
                  )
                },
                onListDirectories = viewModel::listStorageDirectories,
            )
      }
    }

    Box(modifier = Modifier.fillMaxSize()) {
      Box(
          modifier =
              Modifier.fillMaxSize()
                  .then(
                      if (kyantEnabled) Modifier.kyantLayerBackdrop(kyantBackgroundBackdrop)
                      else Modifier
                  )
                  .then(
                      if (blurBackdrop != null) Modifier.layerBackdrop(blurBackdrop) else Modifier
                  )
                  .appMeshBackground()
                  .liveGlassBackgroundLayer(glassScene)
      )
      Box(modifier = Modifier.fillMaxSize()) {
        CompositionLocalProvider(
            LocalSrxBackdrop provides glassScene.activeBackgroundBackdrop,
            LocalSrxBlurBackdrop provides blurBackdrop,
            LocalSrxOverlayBackdrop provides glassScene.activeBackdrop,
            LocalSrxKyantBackdrop provides kyantBackgroundBackdrop.takeIf { kyantEnabled },
        ) {
          Box(
              modifier =
                  Modifier.fillMaxSize()
                      .then(
                          if (kyantEnabled) Modifier.kyantLayerBackdrop(kyantContentBackdrop)
                          else Modifier
                      )
                      .liveGlassContentLayer(glassScene),
          ) {
            NavDisplay(
                backStack = navBackStack,
                modifier = Modifier.fillMaxSize(),
                onBack = { if (!state.busy) popNavBackStack() },
                transition = NavTransitions.MiuixDefault,
            ) {
              entry<SrxRoute.Main> {
                HorizontalPager(
                    state = mainPagerState,
                    modifier = Modifier.fillMaxSize().then(pagerGestureModifier),
                    userScrollEnabled = pageGestureEnabled && !interceptPagerGestures,
                    pageNestedScrollConnection =
                        if (interceptPagerGestures) {
                          PagerGestureNestedScrollConnection
                        } else {
                          PagerDefaults.pageNestedScrollConnection(
                              state = mainPagerState,
                              orientation = Orientation.Horizontal,
                          )
                        },
                    flingBehavior = pagerFlingBehavior,
                    overscrollEffect = null,
                ) { targetPage ->
                  PageContent(Page.entries[targetPage])
                }
              }
              entry<SrxRoute.About>(swipeDismiss = swipeDismiss) {
                AboutScreen(onBack = ::popNavBackStack)
              }
              entry<SrxRoute.Update>(swipeDismiss = swipeDismiss) {
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
              entry<SrxRoute.Theme>(swipeDismiss = swipeDismiss) {
                // 两个主题各有一份主题页实现，参数与回调完全相同：在这里定义一次，避免两处
                // 重复维护（此前 onThemeMode/onThemeStyle/onPredictiveBack 的字面量被复制了
                // 两遍，注释还停留在旧的"淡入覆盖层"说法）。
                val onThemeModeChange: (UiThemeMode) -> Unit = { mode ->
                  // 只在切换前后"实际明暗"不同才播揭示：跟随系统(浅) → 浅色 这类实际
                  // 外观没变，播动画反而突兀。
                  if (mode.isDark(systemDark) != prefs.themeMode.isDark(systemDark)) {
                    applyThemeChangeWithReveal(0L) { viewModel.setThemeMode(mode) }
                  } else {
                    viewModel.setThemeMode(mode)
                  }
                }
                val onThemeStyleChange: (UiThemeStyle) -> Unit = { style ->
                  applyThemeChangeWithReveal(REVEAL_CAPTURE_DELAY_MS) {
                    viewModel.setThemeStyle(style)
                  }
                }
                val onPredictiveBackChange: (Boolean) -> Unit = { enabled ->
                  themeRouteOpen = true
                  startupUpdateCheckDone = true
                  viewModel.setPredictiveBack(enabled)
                  syncPredictiveBackEnabled(context.applicationInfo, enabled)
                  (context as? ComponentActivity)?.recreate()
                }

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
                    onThemeMode = onThemeModeChange,
                    onThemeStyle = onThemeStyleChange,
                    onPredictiveBack = onPredictiveBackChange,
                    onSwipeBack = viewModel::setSwipeBackGesture,
                    onPageSwipe = viewModel::setPageSwipeGesture,
                    onPageScale = viewModel::setPageScale,
                )
              }
              entry<SrxRoute.AppConfig>(swipeDismiss = swipeDismiss) { route ->
                val currentApp = state.currentApp
                val currentConfig = state.currentConfig
                if (
                    currentApp != null &&
                        currentConfig != null &&
                        currentApp.packageName == route.packageName
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
                    if (prefs.themeStyle == UiThemeStyle.Coui) {
                      CouiInfiniteProgressIndicator()
                    } else {
                      InfiniteProgressIndicator()
                    }
                  }
                }
              }
            }
          }
        }

        CompositionLocalProvider(
            LocalSrxBackdrop provides glassScene.activeBackdrop,
            LocalSrxKyantBackdrop provides activeKyantBackdrop,
        ) {
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
              if (prefs.themeStyle == UiThemeStyle.Coui) {
                BusyOverlay(state.busyMessage ?: "处理中", state.busyProgress)
              } else {
                BusyOverlay(state.busyMessage ?: "处理中", state.busyProgress)
              }
            }
          }

          toast?.let {
            LaunchedEffect(it) {
              kotlinx.coroutines.delay(2200)
              toast = null
            }
            if (prefs.themeStyle == UiThemeStyle.Coui) {
              ToastPill(
                  it,
                  Modifier.align(Alignment.BottomCenter)
                      .padding(bottom = padding.calculateBottomPadding() + 18.dp),
              )
            } else {
              ToastPill(
                  it,
                  Modifier.align(Alignment.BottomCenter)
                      .padding(bottom = padding.calculateBottomPadding() + 18.dp),
              )
            }
          }

          if (prefs.themeStyle == UiThemeStyle.Coui) {
            SrxConfirmDialog(
                show = showRootDialog,
                title = "需要 root 权限",
                summary = "存储重定向X 管理应用需要读取和修改模块配置。请在 root 管理器里授予本应用 root 权限后重新检查。",
                cancelText = "稍后",
                confirmText = "重新检查",
                onDismiss = { showRootDialog = false },
                onConfirm = { viewModel.refreshRootAndAll() },
            )
          } else {
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
          }

          if (prefs.themeStyle == UiThemeStyle.Coui) {
            SrxConfirmDialog(
                show = pendingRestoreUri != null,
                title = "还原配置",
                summary = "将用备份覆盖当前全局设置、全部应用配置、外观偏好和检查更新设置。还原后建议重启相关应用或媒体进程。",
                confirmText = "还原",
                onDismiss = { pendingRestoreUri = null },
                onConfirm = {
                  pendingRestoreUri?.let(viewModel::restoreBackupFromUri)
                  pendingRestoreUri = null
                },
            )
          } else {
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
          }

          pendingBatchTemplate?.let { template ->
            if (prefs.themeStyle == UiThemeStyle.Coui) {
              SrxConfirmDialog(
                  show = true,
                  title = "批量应用模板",
                  summary = "将模板“${template.name}”应用到 ${selectedAppPackages.size} 个应用，现有配置会被覆盖。",
                  confirmText = "应用",
                  onDismiss = { pendingBatchTemplate = null },
                  onConfirm = {
                    viewModel.applyTemplateToApps(template.id, selectedAppPackages)
                    selectedAppPackages = emptyList()
                  },
              )
            } else {
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
          }

          state.pendingUpdate?.let { update ->
            val currentModuleVersion = state.dashboard.version.ifBlank { "--" }
            if (prefs.themeStyle == UiThemeStyle.Coui) {
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
            } else {
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
