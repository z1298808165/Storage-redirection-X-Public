package org.srx.manager.ui.liquid

import android.os.Build
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.GraphicsLayerScope
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.unit.Density
import org.srx.manager.ui.theme.isSrxBlurEffectEnabled
import top.yukonga.miuix.kmp.blur.BlendColorEntry
import top.yukonga.miuix.kmp.blur.Backdrop
import top.yukonga.miuix.kmp.blur.BlurColors
import top.yukonga.miuix.kmp.blur.LayerBackdrop
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.blur.rememberLayerBackdrop
import top.yukonga.miuix.kmp.blur.textureBlur
import top.yukonga.miuix.kmp.theme.MiuixTheme

private const val SrxFixedBlurRadius = 25f

@Stable
class CombinedBackdrop(
    val first: Backdrop,
    val second: Backdrop,
) : Backdrop {
    override val isCoordinatesDependent: Boolean = first.isCoordinatesDependent || second.isCoordinatesDependent
    override val offsetResidualX: Float get() = first.offsetResidualX
    override val offsetResidualY: Float get() = first.offsetResidualY

    override fun DrawScope.drawBackdrop(
        density: Density,
        coordinates: LayoutCoordinates?,
        layerBlock: (GraphicsLayerScope.() -> Unit)?,
        downscaleFactor: Int,
    ) {
        with(first) { drawBackdrop(density, coordinates, layerBlock, downscaleFactor) }
        with(second) { drawBackdrop(density, coordinates, layerBlock, downscaleFactor) }
    }
}

@Composable
fun rememberCombinedBackdrop(first: Backdrop, second: Backdrop): Backdrop =
    remember(first, second) { CombinedBackdrop(first, second) }

@Stable
class LiveGlassBackdropScene internal constructor(
    val enabled: Boolean,
    val backgroundBackdrop: LayerBackdrop,
    val contentBackdrop: LayerBackdrop,
    val backdrop: Backdrop,
) {
    val activeBackdrop: Backdrop? get() = if (enabled) backdrop else null
    val activeBackgroundBackdrop: Backdrop? get() = if (enabled) backgroundBackdrop else null
}

@Composable
fun rememberLiveGlassBackdropScene(
    enabled: Boolean,
    backgroundColor: Color = MiuixTheme.colorScheme.surface,
): LiveGlassBackdropScene {
    val backgroundBackdrop = rememberLayerBackdrop {
        drawRect(backgroundColor)
        drawContent()
    }
    val contentBackdrop = rememberLayerBackdrop {
        drawContent()
    }
    val combinedBackdrop = rememberCombinedBackdrop(backgroundBackdrop, contentBackdrop)
    return remember(enabled, backgroundBackdrop, contentBackdrop, combinedBackdrop) {
        LiveGlassBackdropScene(
            enabled = enabled,
            backgroundBackdrop = backgroundBackdrop,
            contentBackdrop = contentBackdrop,
            backdrop = combinedBackdrop,
        )
    }
}

fun Modifier.liveGlassBackgroundLayer(scene: LiveGlassBackdropScene): Modifier =
    then(if (scene.enabled) Modifier.layerBackdrop(scene.backgroundBackdrop) else Modifier)

fun Modifier.liveGlassContentLayer(scene: LiveGlassBackdropScene): Modifier =
    then(if (scene.enabled) Modifier.layerBackdrop(scene.contentBackdrop) else Modifier)

@Composable
fun rememberBlurBackdrop(enabled: Boolean = isSrxBlurEffectEnabled()): LayerBackdrop? {
    if (!enabled || Build.VERSION.SDK_INT < Build.VERSION_CODES.S) return null
    val surface = MiuixTheme.colorScheme.surface
    return rememberLayerBackdrop {
        drawRect(surface)
        drawContent()
    }
}

@Composable
fun BlurredBar(
    backdrop: LayerBackdrop?,
    enabled: Boolean = isSrxBlurEffectEnabled(),
    content: @Composable () -> Unit,
) {
    Box(
        modifier = if (enabled && backdrop != null) {
            Modifier.textureBlur(
                backdrop = backdrop,
                shape = RectangleShape,
                blurRadius = SrxFixedBlurRadius,
                colors = BlurColors(
                    blendColors = listOf(
                        BlendColorEntry(color = MiuixTheme.colorScheme.surface.copy(alpha = 0.87f)),
                    ),
                ),
            )
        } else {
            Modifier
        },
    ) {
        content()
    }
}
