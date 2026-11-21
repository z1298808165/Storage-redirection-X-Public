package org.srx.manager.ui.coui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import io.github.suqi8.coui.kmp.blur.Backdrop
import io.github.suqi8.coui.kmp.blur.LayerBackdrop
import io.github.suqi8.coui.kmp.blur.MaterialBlendMode
import io.github.suqi8.coui.kmp.blur.MaterialBlurStyle
import io.github.suqi8.coui.kmp.blur.ViewBlurStyle
import io.github.suqi8.coui.kmp.blur.couiMaterialBlur
import io.github.suqi8.coui.kmp.blur.couiViewBlur
import io.github.suqi8.coui.kmp.blur.isRuntimeShaderSupported
import io.github.suqi8.coui.kmp.blur.rememberLayerBackdrop
import io.github.suqi8.coui.kmp.effect.MaterialBackdropProvider
import io.github.suqi8.coui.kmp.effect.MaterialBackdropSpec
import io.github.suqi8.coui.kmp.effect.MaterialBlurBlendMode
import io.github.suqi8.coui.kmp.effect.MaterialBlurProvider
import io.github.suqi8.coui.kmp.effect.MaterialBlurSpec
import io.github.suqi8.coui.kmp.theme.COUITheme
import org.srx.manager.ui.theme.srxSurfaceColor

/** 是否启用 COUI 17 材质模糊（由用户设置控制）。 */
val LocalCouiBlurEnabled = staticCompositionLocalOf { true }

/** 录制当前页面的 backdrop，由 [CouiPageScaffold] 提供给页面内打开的对话框、面板和菜单采样。 */
val LocalPageBackdrop = staticCompositionLocalOf<LayerBackdrop?> { null }

/** 返回在窗口背景之上录制页面的 backdrop；关闭模糊时返回 null。 */
@Composable
fun rememberPageBackdrop(): LayerBackdrop? {
  if (!LocalCouiBlurEnabled.current || !isRuntimeShaderSupported()) return null
  val background = srxSurfaceColor()
  return rememberLayerBackdrop {
    drawRect(background)
    drawContent()
  }
}

/**
 * 把 backdrop 桥接到 coui-ui 的 `LocalMaterialBackdrop`（按钮、悬浮栏、悬浮按钮、工具栏）。 backdrop
 * 只能录制这些元素背后的内容，绝不能录制元素自身。
 */
@Composable
fun rememberMaterialBackdropProvider(backdrop: Backdrop?): MaterialBackdropProvider? {
  val active = LocalCouiBlurEnabled.current && backdrop != null && isRuntimeShaderSupported()
  return remember(backdrop, active) {
    if (active && backdrop != null) {
      MaterialBackdropProvider { spec, shape ->
        Modifier.couiViewBlur(backdrop = backdrop, style = spec.toViewBlurStyle(), shape = shape)
      }
    } else {
      null
    }
  }
}

/** 把 backdrop 桥接到 coui-ui 的 `LocalPopupMaterialBlur`（下拉菜单、列表弹窗、上下文菜单）。 */
@Composable
fun rememberPopupMaterialBlur(backdrop: Backdrop?): MaterialBlurProvider? {
  val active = LocalCouiBlurEnabled.current && backdrop != null && isRuntimeShaderSupported()
  return remember(backdrop, active) {
    if (active && backdrop != null) {
      MaterialBlurProvider { spec, shape ->
        Modifier.couiMaterialBlur(
            backdrop = backdrop,
            style = spec.toMaterialBlurStyle(),
            shape = shape,
        )
      }
    } else {
      null
    }
  }
}

/**
 * 对话框或底部面板的 COUI 17 模糊面板：把 [backgroundModifier] 与 [backgroundColor] 传给组件的 `backgroundModifier` /
 * `backgroundColor`。
 */
@Immutable class PanelBlur(val backgroundModifier: Modifier, val backgroundColor: Color)

/**
 * 返回背后页面的面板模糊配置；不可用时返回 null（保留不透明表面）。
 *
 * @param style 例如 `MaterialBlurDefaults::dialog` 或 `MaterialBlurDefaults::bottomSheet`。
 */
@Composable
fun panelBlur(style: (isDark: Boolean) -> MaterialBlurStyle): PanelBlur? {
  val backdrop = LocalPageBackdrop.current
  if (!LocalCouiBlurEnabled.current || backdrop == null || !isRuntimeShaderSupported()) return null
  val dark = COUITheme.colorScheme.isDark
  // 直接用 COUI 官方的 Dialog/Popup 模糊配方，不再把 mix/blend 改成主题色：
  // 官方配方是"中性灰 + 页面自身色彩"，改了反而让弹窗偏色、与卡片对不上。
  return PanelBlur(
      backgroundModifier = Modifier.couiMaterialBlur(backdrop = backdrop, style = style(dark)),
      backgroundColor = srxSurfaceColor(),
  )
}

private fun MaterialBackdropSpec.toViewBlurStyle(): ViewBlurStyle =
    ViewBlurStyle(
        blurRadiusPx = blurRadiusPx,
        layer1 = layer1,
        mode1 = mode1,
        layer2 = layer2,
        mode2 = mode2,
    )

private fun MaterialBlurSpec.toMaterialBlurStyle(): MaterialBlurStyle =
    MaterialBlurStyle(
        blurRadiusPx = blurRadiusPx,
        mixColor = mixColor,
        blendColor = blendColor,
        blendMode =
            when (blendMode) {
              MaterialBlurBlendMode.Overlay -> MaterialBlendMode.Overlay
              MaterialBlurBlendMode.ColorDodge -> MaterialBlendMode.ColorDodge
              MaterialBlurBlendMode.GlowOverlay -> MaterialBlendMode.GlowOverlay
              MaterialBlurBlendMode.GlowColorDodge -> MaterialBlendMode.GlowColorDodge
            },
    )
