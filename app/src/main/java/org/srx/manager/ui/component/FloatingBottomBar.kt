package org.srx.manager.ui.component

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.EaseOut
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.IntrinsicSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.util.fastCoerceIn
import androidx.compose.ui.util.fastRoundToInt
import androidx.compose.ui.util.lerp
import com.kyant.backdrop.Backdrop as KyantBackdrop
import com.kyant.backdrop.backdrops.layerBackdrop as kyantLayerBackdrop
import com.kyant.backdrop.backdrops.rememberCombinedBackdrop as rememberKyantCombinedBackdrop
import com.kyant.backdrop.backdrops.rememberLayerBackdrop as rememberKyantLayerBackdrop
import com.kyant.backdrop.drawBackdrop as kyantDrawBackdrop
import com.kyant.backdrop.effects.blur as kyantBlur
import com.kyant.backdrop.effects.lens as kyantLens
import com.kyant.backdrop.effects.vibrancy as kyantVibrancy
import com.kyant.backdrop.highlight.Highlight as KyantHighlight
import com.kyant.backdrop.shadow.InnerShadow as KyantInnerShadow
import com.kyant.backdrop.shadow.Shadow as KyantShadow
import kotlin.math.abs
import kotlin.math.sign
import kotlinx.coroutines.launch
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.blur.Backdrop
import top.yukonga.miuix.kmp.blur.blur
import top.yukonga.miuix.kmp.blur.drawBackdrop
import top.yukonga.miuix.kmp.shader.isRenderEffectSupported
import top.yukonga.miuix.kmp.theme.MiuixTheme

val LocalFloatingBottomBarTabScale = staticCompositionLocalOf { { 1f } }

@Composable
fun RowScope.FloatingBottomBarItem(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable ColumnScope.() -> Unit,
) {
  val scale = LocalFloatingBottomBarTabScale.current
  Column(
      modifier
          .clip(CircleShape)
          .clickable(
              interactionSource = null,
              indication = null,
              role = Role.Tab,
              onClick = onClick,
          )
          .fillMaxHeight()
          .weight(1f)
          .graphicsLayer {
            val s = scale()
            scaleX = s
            scaleY = s
          },
      verticalArrangement = Arrangement.spacedBy(1.dp, Alignment.CenterVertically),
      horizontalAlignment = Alignment.CenterHorizontally,
      content = content,
  )
}

@Composable
fun FloatingBottomBar(
    modifier: Modifier = Modifier,
    selectedIndex: Int,
    onSelected: (Int) -> Unit,
    backdrop: Backdrop,
    kyantBackdrop: KyantBackdrop? = null,
    tabsCount: Int,
    isBlurEnabled: Boolean = true,
    isLiquidGlassEnabled: Boolean = true,
    enableDrag: Boolean = true,
    content: @Composable RowScope.() -> Unit,
) {
  val isDark = isSrxDarkTheme()
  val effectsSupported = isRenderEffectSupported()
  val liquid = isLiquidGlassEnabled && effectsSupported && kyantBackdrop != null
  val blurEnabled = isBlurEnabled && effectsSupported
  val pillShape = CircleShape
  val accent = MiuixTheme.colorScheme.primary
  val surface = MiuixTheme.colorScheme.surfaceContainer
  val solidSurface = MiuixTheme.colorScheme.surfaceContainerHigh
  val container =
      when {
        liquid && blurEnabled -> surface.copy(alpha = 0.4f)
        liquid -> solidSurface.copy(alpha = if (isDark) 0.86f else 0.9f)
        blurEnabled -> surface.copy(alpha = if (isDark) 0.82f else 0.88f)
        else -> solidSurface
      }
  val tabsBackdrop = rememberKyantLayerBackdrop()
  val density = LocalDensity.current
  val barHeight = 64.dp
  val barInset = 4.dp
  val selectedHeight = 56.dp
  val barInsetPx = with(density) { barInset.toPx() }
  val isLtr = LocalLayoutDirection.current == LayoutDirection.Ltr
  val scope = rememberCoroutineScope()

  var tabWidthPx by remember { mutableFloatStateOf(0f) }
  var totalWidthPx by remember { mutableFloatStateOf(0f) }
  val offsetAnimation = remember { Animatable(0f) }
  val rubberBandPx = with(density) { 4.dp.toPx() }
  val panelOffset by
      remember(rubberBandPx) {
        derivedStateOf {
          if (totalWidthPx == 0f) 0f
          else {
            val fraction = (offsetAnimation.value / totalWidthPx).fastCoerceIn(-1f, 1f)
            rubberBandPx * fraction.sign * EaseOut.transform(abs(fraction))
          }
        }
      }
  var currentIndex by remember { mutableIntStateOf(selectedIndex) }
  class Holder {
    var instance: DampedDragAnimation? = null
  }
  val holder = remember { Holder() }
  val drag =
      remember(scope, tabsCount, density, isLtr, enableDrag) {
        DampedDragAnimation(
                animationScope = scope,
                initialValue = selectedIndex.toFloat(),
                valueRange = 0f..(tabsCount - 1).toFloat(),
                visibilityThreshold = 0.001f,
                initialScale = 1f,
                pressedScale = 78f / 56f,
                canDrag = { offset ->
                  if (!enableDrag) return@DampedDragAnimation false
                  val anim = holder.instance ?: return@DampedDragAnimation true
                  if (tabWidthPx == 0f) return@DampedDragAnimation false
                  val indicatorX = anim.value * tabWidthPx
                  val padding = with(density) { 4.dp.toPx() }
                  val touchX =
                      if (isLtr) padding + indicatorX + offset.x
                      else totalWidthPx - padding - tabWidthPx - indicatorX + offset.x
                  touchX in 0f..totalWidthPx
                },
                onDragStarted = {},
                onDragStopped = {
                  val target =
                      if (enableDrag) targetValue.fastRoundToInt().fastCoerceIn(0, tabsCount - 1)
                      else selectedIndex.fastCoerceIn(0, tabsCount - 1)
                  val changed = enableDrag && currentIndex != target
                  if (changed) {
                    currentIndex = target
                    onSelected(target)
                  }
                  animateToValue(target.toFloat())
                  scope.launch { offsetAnimation.animateTo(0f, spring(1f, 300f, 0.5f)) }
                },
                onDrag = { _, dragAmount ->
                  if (tabWidthPx > 0) {
                    updateValue(
                        (targetValue + dragAmount.x / tabWidthPx * if (isLtr) 1f else -1f)
                            .fastCoerceIn(0f, (tabsCount - 1).toFloat())
                    )
                    scope.launch { offsetAnimation.snapTo(offsetAnimation.value + dragAmount.x) }
                  }
                },
            )
            .also { holder.instance = it }
      }

  LaunchedEffect(selectedIndex, drag) {
    if (currentIndex != selectedIndex) {
      currentIndex = selectedIndex
      drag.animateToValue(selectedIndex.toFloat())
    }
  }

  val interactiveHighlight =
      remember(scope, tabWidthPx) {
        InteractiveHighlight(
            animationScope = scope,
            position = { size, _ ->
              Offset(
                  if (isLtr) (drag.value + 0.5f) * tabWidthPx + panelOffset
                  else size.width - (drag.value + 0.5f) * tabWidthPx + panelOffset,
                  size.height / 2f,
              )
            },
        )
      }
  val combinedBackdrop = kyantBackdrop?.let { rememberKyantCombinedBackdrop(it, tabsBackdrop) }

  Box(modifier = modifier.width(IntrinsicSize.Min), contentAlignment = Alignment.CenterStart) {
    Row(
        Modifier.onGloballyPositioned { coords ->
              totalWidthPx = coords.size.width.toFloat()
              tabWidthPx = ((totalWidthPx - barInsetPx * 2f) / tabsCount).coerceAtLeast(0f)
            }
            .graphicsLayer { translationX = panelOffset }
            .dropShadow(
                shape = pillShape,
                shadow =
                    Shadow(radius = 10.dp, color = Color.Black, alpha = if (isDark) 0.2f else 0.1f),
            )
            .clickable(remember { MutableInteractionSource() }, null) {}
            .then(
                if (liquid) {
                  Modifier.kyantDrawBackdrop(
                      backdrop = requireNotNull(kyantBackdrop),
                      shape = { pillShape },
                      effects = {
                        kyantVibrancy()
                        if (blurEnabled) kyantBlur(8.dp.toPx())
                        kyantLens(24.dp.toPx(), 24.dp.toPx())
                      },
                      highlight = { KyantHighlight.Default.copy(alpha = 0.75f) },
                      shadow = {
                        KyantShadow(
                            radius = 10.dp,
                            color = Color.Black.copy(alpha = if (isDark) 0.2f else 0.1f),
                        )
                      },
                      layerBlock = {
                        val width = size.width.coerceAtLeast(1f)
                        val s = lerp(1f, 1f + 16.dp.toPx() / width, drag.pressProgress)
                        scaleX = s
                        scaleY = s
                      },
                      onDrawSurface = { drawRect(container) },
                  )
                } else if (blurEnabled) {
                  Modifier.drawBackdrop(
                      backdrop = backdrop,
                      shape = { pillShape },
                      effects = { if (blurEnabled) blur(4.dp.toPx(), 4.dp.toPx()) },
                      highlight = null,
                      onDrawSurface = { drawRect(container) },
                  )
                } else {
                  Modifier.background(container, pillShape)
                }
            )
            .then(if (liquid) interactiveHighlight.modifier else Modifier)
            .height(barHeight)
            .padding(barInset),
        verticalAlignment = Alignment.CenterVertically,
        content = content,
    )

    if (liquid) {
      CompositionLocalProvider(
          LocalFloatingBottomBarTabScale provides { lerp(1f, 1.2f, drag.pressProgress) },
      ) {
        Row(
            Modifier.clearAndSetSemantics {}
                .alpha(0f)
                .kyantLayerBackdrop(tabsBackdrop)
                .graphicsLayer { translationX = panelOffset }
                .kyantDrawBackdrop(
                    backdrop = requireNotNull(kyantBackdrop),
                    shape = { pillShape },
                    effects = {
                      kyantVibrancy()
                      if (blurEnabled) kyantBlur(8.dp.toPx())
                      kyantLens(24.dp.toPx(), 24.dp.toPx())
                    },
                    highlight = { KyantHighlight.Default.copy(alpha = 0.75f) },
                    shadow = null,
                    onDrawSurface = { drawRect(container) },
                )
                .then(interactiveHighlight.modifier)
                .height(selectedHeight)
                .padding(horizontal = barInset)
                .graphicsLayer(colorFilter = ColorFilter.tint(accent)),
            verticalAlignment = Alignment.CenterVertically,
            content = content,
        )
      }
    }

    if (tabWidthPx > 0f) {
      val tabWidth = with(density) { tabWidthPx.toDp() }
      if (liquid) {
        Box(
            Modifier.padding(horizontal = barInset)
                .graphicsLayer {
                  val progressOffset = drag.value * tabWidthPx
                  translationX =
                      if (isLtr) progressOffset + panelOffset else -progressOffset + panelOffset
                }
                .then(
                    if (enableDrag) interactiveHighlight.gestureModifier.then(drag.modifier)
                    else Modifier
                )
                .kyantDrawBackdrop(
                    backdrop = requireNotNull(combinedBackdrop),
                    shape = { pillShape },
                    effects = {
                      val progress = drag.pressProgress
                      kyantLens(
                          10.dp.toPx() * progress,
                          14.dp.toPx() * progress,
                          chromaticAberration = true,
                      )
                    },
                    highlight = { KyantHighlight.Default.copy(alpha = drag.pressProgress) },
                    shadow = {
                      KyantShadow(
                          radius = 8.dp,
                          color = Color.Black.copy(alpha = 0.12f * drag.pressProgress),
                      )
                    },
                    innerShadow = {
                      KyantInnerShadow(
                          radius = 8.dp * drag.pressProgress,
                          alpha = drag.pressProgress,
                      )
                    },
                    layerBlock = {
                      scaleX = drag.scaleX
                      scaleY = drag.scaleY
                      val v = drag.velocity / 10f
                      scaleX /= 1f - (v * 0.75f).fastCoerceIn(-0.2f, 0.2f)
                      scaleY *= 1f - (v * 0.25f).fastCoerceIn(-0.2f, 0.2f)
                    },
                    onDrawSurface = {
                      val progress = drag.pressProgress
                      drawRect(
                          color =
                              if (isDark) Color.White.copy(alpha = 0.1f)
                              else Color.Black.copy(alpha = 0.1f),
                          alpha = 1f - progress,
                      )
                      drawRect(Color.Black.copy(alpha = 0.03f * progress))
                    },
                )
                .height(selectedHeight)
                .width(tabWidth),
        )
      } else {
        Box(
            Modifier.padding(horizontal = barInset)
                .graphicsLayer {
                  val progressOffset = drag.value * tabWidthPx
                  translationX =
                      if (isLtr) progressOffset + panelOffset else -progressOffset + panelOffset
                }
                .then(if (enableDrag) drag.modifier else Modifier)
                .clip(pillShape)
                .background(accent.copy(alpha = 0.15f), pillShape)
                .height(selectedHeight)
                .width(tabWidth),
        )
      }
    }
  }
}
