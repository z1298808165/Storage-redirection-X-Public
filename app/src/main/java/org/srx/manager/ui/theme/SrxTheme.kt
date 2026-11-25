package org.srx.manager.ui.theme

import android.app.Activity
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.core.view.WindowInsetsControllerCompat
import io.github.suqi8.coui.kmp.blur.layerBackdrop
import io.github.suqi8.coui.kmp.theme.COUITheme
import io.github.suqi8.coui.kmp.theme.darkColorScheme
import io.github.suqi8.coui.kmp.theme.lightColorScheme
import org.srx.manager.data.UiColorSpec
import org.srx.manager.data.UiColorStyle
import org.srx.manager.data.UiThemeMode
import org.srx.manager.data.UiThemeStyle
import top.yukonga.miuix.kmp.theme.ColorSchemeMode
import top.yukonga.miuix.kmp.theme.Colors as MiuixColors
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.ThemeColorSpec
import top.yukonga.miuix.kmp.theme.ThemeController
import top.yukonga.miuix.kmp.theme.ThemePaletteStyle
import top.yukonga.miuix.kmp.theme.lightColorScheme as miuixLightColorScheme

private val LocalSrxDarkTheme = staticCompositionLocalOf { false }
private val LocalSrxLiquidGlass = staticCompositionLocalOf { true }
private val LocalSrxBlurEffect = staticCompositionLocalOf { true }
private val LocalSrxThemeStyle = staticCompositionLocalOf { UiThemeStyle.Miuix }

/** 是否启用动态取色：面板色需要据此决定跟随壁纸还是用 COUI 原生固定色。 */
val LocalSrxDynamicColor = staticCompositionLocalOf { false }

/** 主题切换会 dispose 内容子树；把 SaveableStateHolder 提到主题之外， 由它在子树重建时托管 rememberSaveable 状态（滚动位置等）。 */
val LocalSrxSaveableStateHolder =
    staticCompositionLocalOf<androidx.compose.runtime.saveable.SaveableStateHolder?> { null }

/**
 * 两个主题共享的配色真值源：按用户动态取色 / 强调色 / 色彩风格 / 色彩标准算出的完整色板。
 *
 * COUI 分支没有 MiuixTheme 外壳，页面背景与共用颜色工具若各自读库默认色板，切换主题时 颜色、取色、主题色就会不一致。统一从这里取色，两个主题只是组件不同、颜色完全相同。
 */
val LocalSrxColorScheme = staticCompositionLocalOf<MiuixColors> { miuixLightColorScheme() }

@Composable
fun SrxTheme(
    dynamicColor: Boolean = false,
    accentColor: Int = 0,
    colorStyle: UiColorStyle = UiColorStyle.TonalSpot,
    colorSpec: UiColorSpec = UiColorSpec.Spec2025,
    themeMode: UiThemeMode = UiThemeMode.System,
    themeStyle: UiThemeStyle = UiThemeStyle.Miuix,
    liquidGlass: Boolean = true,
    blurEffect: Boolean = true,
    content: @Composable () -> Unit,
) {
  val context = LocalContext.current
  val systemDark = isSystemInDarkTheme()
  val dark =
      when (themeMode) {
        UiThemeMode.Light -> false
        UiThemeMode.Dark -> true
        UiThemeMode.System -> systemDark
      }
  // COUI 主题下「动态取色」这个开关是隐藏的，也不该生效：COUI 有自己的原生色板，
  // 壁纸取色会把页面底/卡片色染上壁纸色（深色下表现为页面发蓝），与 Eta 的 COUI 观感不符。
  val effectiveDynamicColor = dynamicColor && themeStyle == UiThemeStyle.Miuix
  // 动态取色但未选强调色时，必须显式把系统壁纸色喂给 ThemeController。
  // 传 null 会让它退回库内固定种子（偏青灰），这就是"取色成青灰"的原因。
  val systemSeed =
      if (effectiveDynamicColor && accentColor == 0)
          top.yukonga.miuix.kmp.theme.platformDynamicColors(dark).primary
      else null
  val keyColor =
      when {
        effectiveDynamicColor && accentColor == 0 -> systemSeed
        effectiveDynamicColor && accentColor != 0 -> Color(accentColor)
        else -> Color(0xFF8EA8F8)
      }
  val miuixController =
      remember(dark, effectiveDynamicColor, keyColor, colorStyle, colorSpec) {
        ThemeController(
            colorSchemeMode =
                when (themeMode) {
                  UiThemeMode.Light ->
                      if (effectiveDynamicColor) ColorSchemeMode.MonetLight
                      else ColorSchemeMode.Light
                  UiThemeMode.Dark ->
                      if (effectiveDynamicColor) ColorSchemeMode.MonetDark else ColorSchemeMode.Dark
                  UiThemeMode.System ->
                      if (effectiveDynamicColor) ColorSchemeMode.MonetSystem
                      else ColorSchemeMode.System
                },
            keyColor = keyColor,
            isDark = dark,
            paletteStyle =
                when (colorStyle) {
                  UiColorStyle.TonalSpot -> ThemePaletteStyle.TonalSpot
                  UiColorStyle.Neutral -> ThemePaletteStyle.Neutral
                  UiColorStyle.Vibrant -> ThemePaletteStyle.Vibrant
                  UiColorStyle.Expressive -> ThemePaletteStyle.Expressive
                  UiColorStyle.Rainbow -> ThemePaletteStyle.Rainbow
                  UiColorStyle.FruitSalad -> ThemePaletteStyle.FruitSalad
                  UiColorStyle.Monochrome -> ThemePaletteStyle.Monochrome
                  UiColorStyle.Fidelity -> ThemePaletteStyle.Fidelity
                  UiColorStyle.Content -> ThemePaletteStyle.Content
                },
            colorSpec =
                when (colorSpec) {
                  UiColorSpec.Spec2021 -> ThemeColorSpec.Spec2021
                  UiColorSpec.Spec2025 -> ThemeColorSpec.Spec2025
                },
        )
      }
  // 唯一真值源：按用户配色偏好算出的完整色板，两个主题共享同一份。
  val seed = miuixController.currentColors()
  val scopedContent: @Composable () -> Unit = {
    LaunchedEffect(dark) {
      val window = (context as? Activity)?.window ?: return@LaunchedEffect
      WindowInsetsControllerCompat(window, window.decorView).apply {
        isAppearanceLightStatusBars = !dark
        isAppearanceLightNavigationBars = !dark
      }
    }
    val saveableHolder = LocalSrxSaveableStateHolder.current
    val contentWithState: @Composable () -> Unit = {
      if (saveableHolder != null) {
        saveableHolder.SaveableStateProvider("srx-app") { content() }
      } else {
        content()
      }
    }
    CompositionLocalProvider(
        LocalSrxDarkTheme provides dark,
        LocalSrxLiquidGlass provides liquidGlass,
        LocalSrxBlurEffect provides blurEffect,
        LocalSrxThemeStyle provides themeStyle,
        LocalSrxDynamicColor provides effectiveDynamicColor,
        LocalSrxColorScheme provides seed,
    ) {
      contentWithState()
    }
  }
  when (themeStyle) {
    UiThemeStyle.Miuix -> MiuixTheme(controller = miuixController) { scopedContent() }
    UiThemeStyle.Coui -> {
      // COUI 2.0.0 的 ThemeController 依赖 materialkolor 5.x 的 com.materialkolor.scheme.*
      // 包，而 Miuix 依赖 6.x 的 com.materialkolor.dynamiccolor.*，两者在同一 APK 中
      // 无法共存（单一版本必有一方缺类崩溃）。这里把同一份按用户配色偏好算好的
      // Miuix 色值逐项注入 COUI 官方明暗色板，让动态取色/强调色/色彩风格/色彩标准
      // 在 COUI 下全部生效且与 Miuix 颜色完全一致，同时绕开 COUI ThemeController
      // 的 scheme 包依赖。注意不能只注入 primary 等少数字段：COUI 官方色板把页面
      // 背景、卡片、主题色文字等写死为库自带蓝/灰，漏掉的槽位不会跟随取色。
      val couiNative = if (dark) darkColorScheme() else lightColorScheme()
      // 默认口径（未开动态取色、未选自定义强调色）直接沿用 COUI 原生色板，
      // 与 Eta 完全一致；只有用户显式开启取色/选色时才从 seed 注入强调色。
      val couiUseNativeAccent = !effectiveDynamicColor && accentColor == 0
      val couiColors = if (couiUseNativeAccent) couiNative else couiColorsFromSeed(seed, dark)
      // Eta 的做法：COUI 下把共享色板逐槽覆盖为 COUI 原生色（背景/卡片/文字/分隔线），
      // 这样同一套页面组件在 COUI 下渲染出来就是 COUI 的观感，而不是"Miuix 配色换个圆角"。
      // 动态取色开启时保留 seed 的中性色（否则壁纸取色会被 COUI 固定灰蓝覆盖，
      // 表现就是"动态取色的色值不对"）；只有非动态取色才用 COUI 原生中性色。
      // 与 Eta 的 AgentAppTheme 一致：共享色板（MiuixTheme + LocalSrxColorScheme）从
      // **同一个 COUI 色板** couiColors 派生，避免两处各写一遍映射后再次分叉。
      val couiScopedColors =
          seed.copy(
              primary = couiColors.primary,
              onPrimary = couiColors.labelOnColor,
              primaryContainer = couiColors.primary.copy(alpha = if (dark) 0.22f else 0.12f),
              onPrimaryContainer = couiColors.primary,
              secondaryContainer = couiColors.container12,
              onSecondaryContainer = couiColors.labelPrimary,
              tertiaryContainer = couiColors.primary.copy(alpha = if (dark) 0.22f else 0.12f),
              onTertiaryContainer = couiColors.primary,
              background = couiColors.backgroundWithCard,
              surface = couiColors.backgroundWithCard,
              surfaceContainer = couiColors.cardBackground,
              surfaceContainerHigh = couiColors.cardPressed,
              surfaceContainerHighest = couiColors.containerSolid,
              onBackground = couiColors.labelPrimary,
              onSurface = couiColors.labelPrimary,
              onSurfaceContainer = couiColors.labelPrimary,
              onSurfaceVariantSummary = couiColors.labelSecondary,
              onSurfaceVariantActions = couiColors.labelSecondary,
              outline = couiColors.divider,
              error = couiColors.error,
              onError = couiColors.onError,
          )
      // 关键：COUI 分支同样要套一层 MiuixTheme。页面里大量组件读的是
      // MiuixTheme.colorScheme（值文本、选中态、图标 tint…），没有这层外壳时
      // 它们会拿到库默认蓝色，表现为"COUI 下这些文字还是蓝的"。
      top.yukonga.miuix.kmp.theme.MiuixTheme(colors = couiScopedColors) {
        COUITheme(colors = couiColors) {
          // COUI 17 的材质模糊/边缘光/触摸高光统一由该 Local 控制（默认 true）。
          // 把用户的「材质模糊」开关接进来，关闭时组件回落为不透明填充。
          androidx.compose.runtime.CompositionLocalProvider(
              io.github.suqi8.coui.kmp.effect.LocalCOUIMaterialEffectEnabled provides blurEffect,
              // 页面 backdrop 录制与弹窗/菜单的模糊面板走项目自己的 Local；
              // 与上面的组件材质开关共用同一个用户设置，关闭时一并回落为不透明表面。
              org.srx.manager.ui.coui.LocalCouiBlurEnabled provides blurEffect,
          ) {
            androidx.compose.runtime.CompositionLocalProvider(
                LocalSrxColorScheme provides couiScopedColors,
            ) {
              // 文档规则 1 的 Scaffold 宿主由根部的 SrxRootScaffold 提供（COUI 分支已是
              // 官方 Scaffold），这里不再套第二层，否则浮层宿主与 insets 都会重复。
              run {
                // 文档规则 5：页面要录制到 backdrop，顶栏光晕/对话框/菜单才能采样到
                // 背后的毛玻璃；不支持运行时着色器时 rememberPageBackdrop 返回 null，
                // 各组件自动回落到不透明表面。
                val pageBackdrop = org.srx.manager.ui.coui.rememberPageBackdrop()
                Box(
                    modifier =
                        Modifier.fillMaxSize()
                            .then(
                                if (pageBackdrop != null) Modifier.layerBackdrop(pageBackdrop)
                                else Modifier
                            )
                ) {
                  androidx.compose.runtime.CompositionLocalProvider(
                      org.srx.manager.ui.coui.LocalPageBackdrop provides pageBackdrop
                  ) {
                    scopedContent()
                  }
                }
              }
            }
          }
        }
      }
    }
  }
}

@Composable @ReadOnlyComposable fun isSrxDarkTheme(): Boolean = LocalSrxDarkTheme.current

@Composable @ReadOnlyComposable fun isSrxLiquidGlassEnabled(): Boolean = LocalSrxLiquidGlass.current

@Composable @ReadOnlyComposable fun isSrxBlurEffectEnabled(): Boolean = LocalSrxBlurEffect.current

@Composable
@ReadOnlyComposable
fun isSrxThemeCoui(): Boolean = LocalSrxThemeStyle.current == UiThemeStyle.Coui

/**
 * 用同一份 Miuix 色板填充 COUI 的语义槽位。
 *
 * COUI 的 light/darkColorScheme 把页面背景、卡片、栏位、主题色文字等写死为库自带 蓝/灰（例如 backgroundWithCard =
 * 0xFFF0F1F2、primaryText = 0xFF0080FF）。若只 注入 primary 等少数字段，COUI 主题下用户选的强调色与壁纸取色只会局部生效， 与 Miuix
 * 主题呈现不同颜色。这里逐项注入，两个主题因此共享完全一致的配色。
 *
 * 明暗差异由 lightColorScheme()/darkColorScheme() 的其余槽位保留（分隔线、按压态、 半透明叠加等），只覆盖需要跟随取色的语义色。
 */
private fun couiColorsFromSeed(seed: MiuixColors, dark: Boolean) =
    (if (dark) darkColorScheme() else lightColorScheme()).copy(
        // 按 KernelSU / stelliberty 等参考项目的做法：只喂"强调色"给主题，
        // 页面背景、卡片、弹窗、底栏、文字层级全部沿用 COUI 官方色板。
        // 之前把整份 Miuix 色板逐槽注入 COUI，导致每个面板都要手工调色补偿
        // （couiPanelSurface / panelBlur 染色 / glow baseColor），越补越不像原生。
        primary = seed.primary,
        onPrimary = seed.onPrimary,
        primaryText = seed.primary,
        primaryTextOnPopup = seed.primary,
        labelTheme = seed.primary,
        link = seed.primary,
        tips = seed.primary,
        containerTheme = seed.primary,
        containerThemeHalftone = seed.primary.copy(alpha = 0.15f),
        textColorHighlight = seed.primary.copy(alpha = 0.15f),
        // 中性面回归 COUI 官方色板的原生值（backgroundWithCard / cardBackground / bar…），
        // 与 Eta 一致：COUI 下页面底、卡片、底栏都用 COUI 自己的中性灰，
        // 只有强调色系从 seed 注入。之前把中性面改写成 Miuix surface，
        // 才会有"同色不同观感"的偏差。
        labelOnColor = seed.onPrimary,
    )
