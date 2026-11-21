package org.srx.manager.ui.theme

import androidx.compose.runtime.Composable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import top.yukonga.miuix.kmp.material.isRenderEffectSupported

/**
 * 主题取色的唯一入口。
 *
 * 两个主题现在共用同一份色板（[LocalSrxColorScheme]）：强调色来自 seed.primary， 中性面来自 seed.surface 系列，COUI
 * 侧只把这些槽位映射过去。因此组件里不该再出现 `if (themeStyle == Coui) COUITheme.colorScheme.x else
 * LocalSrxColorScheme.y` 这种分支 ——那等于把同一件事实现了两遍，改一处漏一处，正是面板颜色越调越乱的来源。
 */
@Composable @ReadOnlyComposable fun srxSurfaceColor(): Color = LocalSrxColorScheme.current.surface

/** 强调色：图标、选中态、主按钮统一走这里，不再分别读 COUI/Miuix 的槽位。 */
@Composable @ReadOnlyComposable fun srxAccentColor(): Color = LocalSrxColorScheme.current.primary

/** 面板上的次级文字（摘要行、说明文案）。 */
@Composable
@ReadOnlyComposable
fun srxSecondaryTextColor(): Color = LocalSrxColorScheme.current.onSurfaceVariantSummary

/** 本机是否支持 RenderEffect（模糊/透镜）。原先各组件各自调 isRenderEffectSupported()。 */
@Composable fun srxGlassSupported(): Boolean = remember { isRenderEffectSupported() }

/** 模糊实际生效 = 用户开关 && 设备支持。 */
@Composable fun srxBlurActive(): Boolean = isSrxBlurEffectEnabled() && srxGlassSupported()

/** 液态玻璃实际生效 = 用户开关 && 设备支持。 */
@Composable fun srxLiquidActive(): Boolean = isSrxLiquidGlassEnabled() && srxGlassSupported()

/** 面板主文案色（标题、行标题）。 */
@Composable
@ReadOnlyComposable
fun srxPrimaryTextColor(): Color = LocalSrxColorScheme.current.onSurface

/** 更弱的一级文案色（占位、禁用说明）。 */
@Composable
@ReadOnlyComposable
fun srxTertiaryTextColor(): Color = LocalSrxColorScheme.current.disabledOnSurface

/** 错误/危险色。 */
@Composable @ReadOnlyComposable fun srxErrorColor(): Color = LocalSrxColorScheme.current.error

/** 强调色上的前景色（主按钮文字、选中态文字）。 */
@Composable
@ReadOnlyComposable
fun srxOnAccentColor(): Color = LocalSrxColorScheme.current.onPrimary

/** 抬升面（搜索框、圆钮 fallback、顶栏 glow 底）。 */
@Composable
@ReadOnlyComposable
fun srxElevatedSurfaceColor(): Color = LocalSrxColorScheme.current.surfaceContainerHigh

/**
 * 页面底色：比卡片（surface）暗一档，形成 KernalSU / Eta 那种"卡片浮在底色上"的层次。 卡片仍用 surface，两者刻意取不同值 —— 之前把两者都指向
 * surface，导致卡片与页面糊在一起。
 */
@Composable
fun srxPageBackgroundColor(): Color {
  val surface = LocalSrxColorScheme.current.surface
  if (isSrxThemeCoui()) {
    // COUI：底色带一层强调色相（Eta 那种"浅蓝白"），与 Miuix 的中性灰一眼可分。
    val accent = LocalSrxColorScheme.current.primary
    return if (isSrxDarkTheme()) lerp(lerp(surface, Color.Black, 0.18f), accent, 0.14f)
    else lerp(lerp(surface, Color.Black, 0.03f), accent, 0.07f)
  }
  return if (isSrxDarkTheme()) lerp(surface, Color.Black, 0.22f)
  else lerp(surface, Color.Black, 0.045f)
}

/** 列表行度量：Eta 两套风格的密度不同 —— COUI 更紧凑（行更矮、图标更小、字号更小）， Miuix 更宽松。布局结构完全一致，只有这些数值按风格取不同档。 */
object SrxRowMetrics {
  @Composable fun minHeight(): Dp = if (isSrxThemeCoui()) 48.dp else 56.dp

  @Composable fun verticalPadding(): Dp = if (isSrxThemeCoui()) 10.dp else 9.dp

  @Composable fun iconSize(): Dp = if (isSrxThemeCoui()) 24.dp else 30.dp

  @Composable fun iconRadius(): Dp = if (isSrxThemeCoui()) 8.dp else 9.dp

  @Composable fun titleSize(): TextUnit = if (isSrxThemeCoui()) 15.sp else 16.sp

  @Composable fun titleLineHeight(): TextUnit = if (isSrxThemeCoui()) 19.sp else 20.sp

  @Composable fun itemSpacing(): Dp = if (isSrxThemeCoui()) 16.dp else 14.dp

  /** 分组之间的纵向间距：Eta COUI 16dp、Miuix 12dp。 */
  @Composable fun groupSpacing(): Dp = if (isSrxThemeCoui()) 16.dp else 12.dp
}
