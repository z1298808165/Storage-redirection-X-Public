package org.srx.manager.ui.component

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.interaction.InteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.isSpecified
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.util.lerp
import org.srx.manager.LocalSrxBackdrop
import org.srx.manager.ui.liquid.lens
import org.srx.manager.ui.liquid.vibrancy
import org.srx.manager.ui.theme.isSrxBlurEffectEnabled
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import top.yukonga.miuix.kmp.blur.blur
import top.yukonga.miuix.kmp.blur.drawBackdrop
import top.yukonga.miuix.kmp.blur.highlight.BloomStroke
import top.yukonga.miuix.kmp.blur.highlight.Highlight
import top.yukonga.miuix.kmp.shader.isRenderEffectSupported

/**
 * 控件级液态玻璃表面。
 *
 * 按钮、输入框、胶囊等小尺寸控件的折射半径远小于卡片和底栏，这里统一按控件尺寸给出更浅的 折射高度与幅度，避免小控件被过度弯曲。液态玻璃关闭时退回纯色背景，背景模糊单独关闭时
 * 只保留折射与高光，行为与卡片、底栏一致。
 */
@Composable
internal fun Modifier.liquidGlassControl(
    shape: Shape,
    tint: Color,
    refractionHeight: Dp = 9.dp,
    refractionAmount: Dp = 11.dp,
    blurRadius: Dp = 3.dp,
    chromaticAberration: Float = 0.28f,
    highlightAlpha: Float = 0.6f,
): Modifier {
  val effectsSupported = isRenderEffectSupported()
  val liquid = isSrxLiquidGlassEnabled() && effectsSupported
  val blurEnabled = isSrxBlurEffectEnabled() && effectsSupported
  val backdrop = LocalSrxBackdrop.current
  if (!liquid || backdrop == null) {
    return this.clip(shape)
        .then(if (tint.isSpecified) Modifier.background(tint, shape) else Modifier)
  }
  val dark = isSrxDarkTheme()
  return this.drawBackdrop(
      backdrop = backdrop,
      shape = { shape },
      effects = {
        vibrancy()
        if (blurEnabled) blur(blurRadius.toPx(), blurRadius.toPx())
        lens(
            refractionHeight = refractionHeight.toPx(),
            refractionAmount = refractionAmount.toPx(),
            depthEffect = true,
            chromaticAberration = chromaticAberration,
        )
      },
      highlight = { controlHighlight(dark, highlightAlpha) },
      onDrawSurface = { if (tint.isSpecified) drawRect(tint) },
  )
}

/** 按压时的液态回弹。液态玻璃开启时按压会轻微放大并压暗折射层，关闭时不产生额外动画， 保持原有的即时反馈。 */
@Composable
internal fun Modifier.liquidPressScale(
    interactionSource: InteractionSource,
    pressedScale: Float = 0.97f,
): Modifier {
  val liquid = isSrxLiquidGlassEnabled() && isRenderEffectSupported()
  val pressed by interactionSource.collectIsPressedAsState()
  val progress by
      animateFloatAsState(
          targetValue = if (pressed) 1f else 0f,
          animationSpec = spring(dampingRatio = 0.62f, stiffness = 620f),
          label = "liquidPressScale",
      )
  if (!liquid) return this
  return this.graphicsLayer {
    val scale = lerp(1f, pressedScale, progress)
    scaleX = scale
    scaleY = scale
  }
}

private fun controlHighlight(dark: Boolean, alpha: Float): Highlight =
    Highlight(
        width = 1.dp,
        alpha = alpha,
        style =
            BloomStroke(
                color = Color.White.copy(alpha = if (dark) 0.1f else 0.16f),
                innerBlurRadius = 1.5.dp,
            ),
    )
