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
import kotlinx.coroutines.launch
import org.srx.manager.ui.liquid.InnerShadow
import org.srx.manager.ui.liquid.innerShadow
import org.srx.manager.ui.liquid.lens
import org.srx.manager.ui.liquid.rememberCombinedBackdrop
import org.srx.manager.ui.liquid.vibrancy
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.blur.Backdrop
import top.yukonga.miuix.kmp.blur.blur
import top.yukonga.miuix.kmp.blur.drawBackdrop
import top.yukonga.miuix.kmp.blur.highlight.BloomStroke
import top.yukonga.miuix.kmp.blur.highlight.Highlight
import top.yukonga.miuix.kmp.blur.highlight.LightPosition
import top.yukonga.miuix.kmp.blur.highlight.LightSource
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.blur.rememberLayerBackdrop
import top.yukonga.miuix.kmp.blur.sensor.rememberDeviceTilt
import top.yukonga.miuix.kmp.theme.MiuixTheme
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.sign
import kotlin.math.sin
import kotlin.math.sqrt

val LocalFloatingBottomBarTabScale = staticCompositionLocalOf { { 1f } }

private val IndicatorHighlight: Highlight = Highlight(
    width = 1.dp,
    alpha = 1f,
    style = BloomStroke(
        color = Color.White.copy(alpha = 0.12f),
        innerBlurRadius = 2.dp,
        primaryLight = LightSource(
            position = LightPosition(0.5f, -0.3f, -0.05f),
            color = Color.White,
            intensity = 1f,
        ),
        secondaryLight = LightSource(
            position = LightPosition(0.5f, 0.8f, -0.5f),
            color = Color.White,
            intensity = 0.4f,
        ),
        dualPeak = true,
    ),
)

@Composable
private fun rememberGravityRotatedHighlight(base: Highlight, extraDegrees: Float = 0f): Highlight {
    val baseStyle = base.style as BloomStroke
    val tilt by rememberDeviceTilt()
    val rotatedPrimary = remember(tilt, baseStyle.primaryLight, extraDegrees) {
        val gx = tilt.gravityX
        val gy = tilt.gravityY
        val magSq = gx * gx + gy * gy
        val (lx0, ly0) = if (magSq > 0.01f) {
            val inv = 1f / sqrt(magSq)
            gx * inv to gy * inv
        } else {
            0f to -1f
        }
        val rad = extraDegrees * PI / 180.0
        val c = cos(rad).toFloat()
        val s = sin(rad).toFloat()
        baseStyle.primaryLight.copy(
            position = LightPosition(
                x = 0.5f + c * lx0 - s * ly0,
                y = 0.7f + s * lx0 + c * ly0,
                z = baseStyle.primaryLight.position.z,
            ),
        )
    }
    return remember(base, rotatedPrimary) { base.copy(style = baseStyle.copy(primaryLight = rotatedPrimary)) }
}

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
    tabsCount: Int,
    isBlurEnabled: Boolean = true,
    enableDrag: Boolean = true,
    content: @Composable RowScope.() -> Unit,
) {
    val isDark = isSrxDarkTheme()
    val pillShape = CircleShape
    val accent = MiuixTheme.colorScheme.primary
    val surface = MiuixTheme.colorScheme.surfaceContainer
    val container = if (isBlurEnabled) surface.copy(alpha = 0.4f) else surface
    val tabsBackdrop = rememberLayerBackdrop()
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
    val panelOffset by remember(rubberBandPx) {
        derivedStateOf {
            if (totalWidthPx == 0f) 0f else {
                val fraction = (offsetAnimation.value / totalWidthPx).fastCoerceIn(-1f, 1f)
                rubberBandPx * fraction.sign * EaseOut.transform(abs(fraction))
            }
        }
    }
    var currentIndex by remember { mutableIntStateOf(selectedIndex) }
    class Holder { var instance: DampedDragAnimation? = null }
    val holder = remember { Holder() }
    val drag = remember(scope, tabsCount, density, isLtr, enableDrag) {
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
                val touchX = if (isLtr) padding + indicatorX + offset.x else totalWidthPx - padding - tabWidthPx - indicatorX + offset.x
                touchX in 0f..totalWidthPx
            },
            onDragStarted = {},
            onDragStopped = {
                val target = if (enableDrag) targetValue.fastRoundToInt().fastCoerceIn(0, tabsCount - 1) else selectedIndex.fastCoerceIn(0, tabsCount - 1)
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
                    updateValue((targetValue + dragAmount.x / tabWidthPx * if (isLtr) 1f else -1f).fastCoerceIn(0f, (tabsCount - 1).toFloat()))
                    scope.launch { offsetAnimation.snapTo(offsetAnimation.value + dragAmount.x) }
                }
            },
        ).also { holder.instance = it }
    }

    LaunchedEffect(selectedIndex, drag) {
        if (currentIndex != selectedIndex) {
            currentIndex = selectedIndex
            drag.animateToValue(selectedIndex.toFloat())
        }
    }

    val interactiveHighlight = remember(scope, tabWidthPx) {
        InteractiveHighlight(
            animationScope = scope,
            position = { size, _ ->
                Offset(
                    if (isLtr) (drag.value + 0.5f) * tabWidthPx + panelOffset else size.width - (drag.value + 0.5f) * tabWidthPx + panelOffset,
                    size.height / 2f,
                )
            },
        )
    }
    val baseHighlight = rememberGravityRotatedHighlight(IndicatorHighlight, -45f)
    val pillHighlight = rememberGravityRotatedHighlight(IndicatorHighlight, 90f)
    val combinedBackdrop = rememberCombinedBackdrop(backdrop, tabsBackdrop)

    Box(modifier = modifier.width(IntrinsicSize.Min), contentAlignment = Alignment.CenterStart) {
        Row(
            Modifier
                .onGloballyPositioned { coords ->
                    totalWidthPx = coords.size.width.toFloat()
                    tabWidthPx = ((totalWidthPx - barInsetPx * 2f) / tabsCount).coerceAtLeast(0f)
                }
                .graphicsLayer { translationX = panelOffset }
                .dropShadow(
                    shape = pillShape,
                    shadow = Shadow(radius = 10.dp, color = Color.Black, alpha = if (isDark) 0.2f else 0.1f),
                )
                .clickable(remember { MutableInteractionSource() }, null) {}
                .then(
                    if (isBlurEnabled) {
                        Modifier.drawBackdrop(
                            backdrop = backdrop,
                            shape = { pillShape },
                            effects = {
                                vibrancy()
                                blur(4.dp.toPx(), 4.dp.toPx())
                                lens(refractionHeight = 24.dp.toPx(), refractionAmount = 24.dp.toPx())
                            },
                            highlight = { baseHighlight.copy(alpha = 0.75f) },
                            layerBlock = {
                                val width = size.width.coerceAtLeast(1f)
                                val s = lerp(1f, 1f + 16.dp.toPx() / width, drag.pressProgress)
                                scaleX = s
                                scaleY = s
                            },
                            onDrawSurface = { drawRect(container) },
                        )
                    } else {
                        Modifier.background(container, pillShape)
                    }
                )
                .then(if (isBlurEnabled) interactiveHighlight.modifier else Modifier)
                .height(barHeight)
                .padding(barInset),
            verticalAlignment = Alignment.CenterVertically,
            content = content,
        )

        if (isBlurEnabled) {
            CompositionLocalProvider(
                LocalFloatingBottomBarTabScale provides {
                    lerp(1f, 1.2f, drag.pressProgress)
                },
            ) {
                Row(
                    Modifier
                        .clearAndSetSemantics {}
                        .alpha(0f)
                        .layerBackdrop(tabsBackdrop)
                        .graphicsLayer { translationX = panelOffset }
                        .drawBackdrop(
                            backdrop = backdrop,
                            shape = { pillShape },
                            effects = {
                                vibrancy()
                                blur(4.dp.toPx(), 4.dp.toPx())
                                lens(refractionHeight = 24.dp.toPx(), refractionAmount = 24.dp.toPx())
                            },
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
            if (isBlurEnabled) {
                Box(
                    Modifier
                        .padding(horizontal = barInset)
                        .graphicsLayer {
                            val progressOffset = drag.value * tabWidthPx
                            translationX = if (isLtr) progressOffset + panelOffset else -progressOffset + panelOffset
                        }
                        .then(if (enableDrag) interactiveHighlight.gestureModifier.then(drag.modifier) else Modifier)
                        .drawBackdrop(
                            backdrop = combinedBackdrop,
                            shape = { pillShape },
                            effects = {
                                val progress = drag.pressProgress
                                lens(
                                    refractionHeight = 10.dp.toPx() * progress,
                                    refractionAmount = 14.dp.toPx() * progress,
                                    depthEffect = true,
                                    chromaticAberration = 0.5f,
                                )
                            },
                            highlight = { pillHighlight.copy(alpha = drag.pressProgress) },
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
                                    color = if (isDark) Color.White.copy(alpha = 0.1f) else Color.Black.copy(alpha = 0.1f),
                                    alpha = 1f - progress,
                                )
                                drawRect(Color.Black.copy(alpha = 0.03f * progress))
                            },
                        )
                        .innerShadow(pillShape) {
                            InnerShadow(
                                radius = 8.dp * drag.pressProgress,
                                color = Color.Black.copy(alpha = 0.15f),
                                alpha = drag.pressProgress,
                            )
                        }
                        .height(selectedHeight)
                        .width(tabWidth),
                )
            } else {
                Box(
                    Modifier
                        .padding(horizontal = barInset)
                        .graphicsLayer {
                            val progressOffset = drag.value * tabWidthPx
                            translationX = if (isLtr) progressOffset + panelOffset else -progressOffset + panelOffset
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
