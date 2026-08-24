package org.srx.manager.ui.component

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.interaction.InteractionSource
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.PressInteraction
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.isSpecified
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.util.lerp
import com.kyant.backdrop.backdrops.layerBackdrop as kyantLayerBackdrop
import com.kyant.backdrop.backdrops.rememberCombinedBackdrop as rememberKyantCombinedBackdrop
import com.kyant.backdrop.backdrops.rememberLayerBackdrop as rememberKyantLayerBackdrop
import com.kyant.backdrop.drawBackdrop as kyantDrawBackdrop
import com.kyant.backdrop.effects.blur as kyantBlur
import com.kyant.backdrop.effects.lens as kyantLens
import com.kyant.backdrop.highlight.Highlight as KyantHighlight
import com.kyant.backdrop.shadow.InnerShadow as KyantInnerShadow
import com.kyant.backdrop.shadow.Shadow as KyantShadow
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.launch
import org.srx.manager.LocalSrxBackdrop
import org.srx.manager.LocalSrxKyantBackdrop
import org.srx.manager.ui.liquid.lens
import org.srx.manager.ui.liquid.vibrancy
import org.srx.manager.ui.theme.isSrxBlurEffectEnabled
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import top.yukonga.miuix.kmp.basic.Switch
import top.yukonga.miuix.kmp.blur.blur
import top.yukonga.miuix.kmp.blur.drawBackdrop
import top.yukonga.miuix.kmp.blur.highlight.BloomStroke
import top.yukonga.miuix.kmp.blur.highlight.Highlight
import top.yukonga.miuix.kmp.shader.isRenderEffectSupported
import top.yukonga.miuix.kmp.theme.MiuixTheme

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

/**
 * 参考 AndroidLiquidGlass 的 LiquidToggle：液态玻璃开启且 backdrop 可用时，轨道和滑块使用 小尺寸折射；其余情况直接使用 Miuix
 * 开关。两种路径均保留触觉反馈和无障碍语义。
 */
@Composable
internal fun LiquidSwitch(
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    interactionSource: MutableInteractionSource? = null,
) {
  if (
      !isSrxLiquidGlassEnabled() ||
          !isRenderEffectSupported() ||
          LocalSrxKyantBackdrop.current == null
  ) {
    Switch(
        checked = checked,
        onCheckedChange = onCheckedChange,
        modifier = modifier,
        enabled = enabled,
    )
    return
  }

  val resolvedInteractionSource = interactionSource ?: remember { MutableInteractionSource() }
  val backdrop = requireNotNull(LocalSrxKyantBackdrop.current)
  val hapticFeedback = LocalHapticFeedback.current
  val blurEnabled = isSrxBlurEffectEnabled()
  var visualPressed by remember { mutableStateOf(false) }
  LaunchedEffect(resolvedInteractionSource, enabled) {
    if (!enabled) {
      visualPressed = false
      return@LaunchedEffect
    }
    val activePresses = mutableSetOf<PressInteraction.Press>()
    var pressStartedAt = 0L
    var releaseJob: Job? = null
    resolvedInteractionSource.interactions.collect { interaction ->
      when (interaction) {
        is PressInteraction.Press -> {
          releaseJob?.cancel()
          activePresses += interaction
          pressStartedAt = System.nanoTime()
          visualPressed = true
        }
        is PressInteraction.Release -> activePresses -= interaction.press
        is PressInteraction.Cancel -> activePresses -= interaction.press
      }
      if (interaction is PressInteraction.Release || interaction is PressInteraction.Cancel) {
        if (activePresses.isEmpty()) {
          val elapsedMillis = (System.nanoTime() - pressStartedAt) / 1_000_000L
          releaseJob = launch {
            delay((150L - elapsedMillis).coerceAtLeast(0L))
            if (activePresses.isEmpty()) visualPressed = false
          }
        }
      }
    }
  }
  val pressProgress by
      animateFloatAsState(
          targetValue = if (visualPressed && enabled) 1f else 0f,
          animationSpec = spring(dampingRatio = 0.46f, stiffness = 560f),
          label = "liquidSwitchPress",
      )
  val trackTint by
      animateColorAsState(
          targetValue =
              when {
                checked && visualPressed -> MiuixTheme.colorScheme.primary.copy(alpha = 0.86f)
                checked -> MiuixTheme.colorScheme.primary.copy(alpha = 0.72f)
                visualPressed -> MiuixTheme.colorScheme.secondary.copy(alpha = 0.5f)
                else -> MiuixTheme.colorScheme.secondary.copy(alpha = 0.34f)
              },
          label = "liquidSwitchTrack",
      )
  val thumbTint by
      animateColorAsState(
          targetValue =
              when {
                checked && visualPressed -> MiuixTheme.colorScheme.onPrimary.copy(alpha = 0.98f)
                checked -> MiuixTheme.colorScheme.onPrimary.copy(alpha = 0.92f)
                visualPressed -> MiuixTheme.colorScheme.onSecondary.copy(alpha = 0.94f)
                else -> MiuixTheme.colorScheme.onSecondary.copy(alpha = 0.88f)
              },
          label = "liquidSwitchThumb",
      )
  val accent = MiuixTheme.colorScheme.primary
  val secondary = MiuixTheme.colorScheme.secondary
  val trackBackdrop = rememberKyantLayerBackdrop()
  val thumbBackdrop = rememberKyantCombinedBackdrop(backdrop, trackBackdrop)
  var switchAppeared by remember { mutableStateOf(false) }
  val selectionPulse = remember { Animatable(0f) }
  val offset by
      animateDpAsState(
          targetValue = if (checked) 25.dp else 4.dp,
          animationSpec =
              if (switchAppeared) spring(dampingRatio = 0.56f, stiffness = 720f)
              else spring(dampingRatio = 1f, stiffness = 10_000f),
          label = "liquidSwitchOffset",
      )
  LaunchedEffect(checked) {
    if (!switchAppeared) {
      switchAppeared = true
      selectionPulse.snapTo(0f)
      return@LaunchedEffect
    }
    selectionPulse.snapTo(0f)
    selectionPulse.animateTo(1f, tween(durationMillis = 260))
    selectionPulse.animateTo(0f, tween(durationMillis = 520))
  }
  val bubbleOffsetTarget = if (checked) 21.dp else 0.dp
  val bubbleOffset by
      animateDpAsState(
          targetValue = bubbleOffsetTarget,
          animationSpec =
              if (switchAppeared) spring(dampingRatio = 0.38f, stiffness = 430f)
              else spring(dampingRatio = 1f, stiffness = 10_000f),
          label = "liquidSwitchBubbleOffset",
      )

  Box(
      modifier =
          modifier
              .size(49.dp, 28.dp)
              .graphicsLayer {
                val scale = lerp(1f, 1.06f, pressProgress)
                scaleX = scale
                scaleY = scale
                shadowElevation = (6.dp * pressProgress).toPx()
                shape = CircleShape
                clip = false
                ambientShadowColor = accent.copy(alpha = 0.2f * pressProgress)
                spotShadowColor = accent.copy(alpha = 0.28f * pressProgress)
              }
              .toggleable(
                  value = checked,
                  enabled = enabled,
                  role = Role.Switch,
                  interactionSource = resolvedInteractionSource,
                  indication = null,
                  onValueChange = { value ->
                    onCheckedChange(value)
                    hapticFeedback.performHapticFeedback(
                        if (value) HapticFeedbackType.ToggleOn else HapticFeedbackType.ToggleOff
                    )
                  },
              ),
      contentAlignment = Alignment.CenterStart,
  ) {
    Box(
        Modifier.size(49.dp, 28.dp)
            .kyantLayerBackdrop(trackBackdrop)
            .kyantDrawBackdrop(
                backdrop = backdrop,
                shape = { CircleShape },
                effects = {
                  if (blurEnabled) kyantBlur((1.dp + 1.dp * pressProgress).toPx())
                  kyantLens(
                      (3.dp + 2.dp * pressProgress).toPx(),
                      (4.dp + 3.dp * pressProgress).toPx(),
                      chromaticAberration = true,
                  )
                },
                highlight = { KyantHighlight.Ambient.copy(alpha = 0.48f + 0.34f * pressProgress) },
                shadow = {
                  KyantShadow(
                      radius = 5.dp * pressProgress,
                      color = accent.copy(alpha = 0.16f * pressProgress),
                  )
                },
                innerShadow = {
                  KyantInnerShadow(
                      radius = 5.dp * pressProgress,
                      alpha = 0.55f * pressProgress,
                  )
                },
                onDrawSurface = { drawRect(trackTint) },
            ),
    )
    Box(
        Modifier.offset(x = 1.dp + bubbleOffset)
            .size(26.dp)
            .graphicsLayer {
              val bubbleProgress = maxOf(pressProgress, selectionPulse.value).coerceIn(0f, 1f)
              val scale = lerp(0.72f, 1.28f, bubbleProgress)
              scaleX = scale * if (checked) 1.04f else 1f
              scaleY = scale * if (checked) 0.98f else 1f
              alpha = bubbleProgress * 0.72f
            }
            .kyantDrawBackdrop(
                backdrop = thumbBackdrop,
                shape = { CircleShape },
                effects = {
                  if (blurEnabled) kyantBlur((1.dp + 2.dp * pressProgress).toPx())
                  kyantLens(
                      (4.dp + 5.dp * pressProgress).toPx(),
                      (6.dp + 8.dp * pressProgress).toPx(),
                      chromaticAberration = true,
                  )
                },
                highlight = { KyantHighlight.Ambient.copy(alpha = 0.62f + 0.3f * pressProgress) },
                shadow = {
                  KyantShadow(
                      radius = 3.dp + 5.dp * pressProgress,
                      color = accent.copy(alpha = 0.08f + 0.16f * pressProgress),
                  )
                },
                innerShadow = {
                  KyantInnerShadow(
                      radius = 3.dp + 4.dp * pressProgress,
                      alpha = 0.3f + 0.3f * pressProgress,
                  )
                },
                onDrawSurface = {
                  drawRect(
                      if (checked) accent.copy(alpha = 0.3f) else secondary.copy(alpha = 0.24f)
                  )
                },
            ),
    )
    Box(
        Modifier.offset(x = offset)
            .size(20.dp)
            .graphicsLayer {
              val scale = lerp(1f, 1.16f, pressProgress)
              scaleX = scale
              scaleY = scale
              shadowElevation = (5.dp * pressProgress).toPx()
              shape = CircleShape
              clip = false
              ambientShadowColor = Color.White.copy(alpha = 0.28f * pressProgress)
              spotShadowColor = accent.copy(alpha = 0.26f * pressProgress)
            }
            .kyantDrawBackdrop(
                backdrop = thumbBackdrop,
                shape = { CircleShape },
                effects = {
                  if (blurEnabled) kyantBlur((0.5.dp + 1.dp * pressProgress).toPx())
                  kyantLens(
                      (2.dp + 2.dp * pressProgress).toPx(),
                      (3.dp + 3.dp * pressProgress).toPx(),
                      chromaticAberration = true,
                  )
                },
                highlight = { KyantHighlight.Ambient.copy(alpha = 0.72f + 0.2f * pressProgress) },
                shadow = {
                  KyantShadow(
                      radius = 5.dp * pressProgress,
                      color = accent.copy(alpha = 0.18f * pressProgress),
                  )
                },
                innerShadow = {
                  KyantInnerShadow(
                      radius = 3.dp * pressProgress,
                      alpha = 0.55f * pressProgress,
                  )
                },
                onDrawSurface = { drawRect(thumbTint) },
            ),
    )
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
