package org.srx.manager.ui.effect

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalWindowInfo
import androidx.compose.ui.unit.dp
import kotlin.math.floor
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import org.srx.manager.ui.theme.LocalSrxColorScheme
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.material.isRuntimeShaderSupported

@Composable
fun BgEffectBackground(
    dynamicBackground: Boolean,
    modifier: Modifier = Modifier,
    isFullSize: Boolean = false,
    effectBackground: Boolean = true,
    alpha: () -> Float = { 1f },
    content: @Composable BoxScope.() -> Unit,
) {
  if (!isRuntimeShaderSupported()) {
    // 无 runtime shader 的机型只保留 surface 底色，与 KernelSU manager 的行为一致
    Box(modifier = modifier.background(LocalSrxColorScheme.current.surface), content = content)
    return
  }

  Box(modifier = modifier) {
    val surface = LocalSrxColorScheme.current.surface
    val deviceType = if (shouldShowSplitPane()) DeviceType.Pad else DeviceType.Phone
    val isDarkTheme = isSrxDarkTheme()
    val painter = remember { BgEffectPainter() }
    val preset = remember(deviceType, isDarkTheme) { BgEffectConfig.get(deviceType, isDarkTheme) }
    val colorStage = remember { Animatable(0f) }

    LaunchedEffect(dynamicBackground, preset) {
      if (!dynamicBackground) return@LaunchedEffect
      val animatesColors = preset.colors1 !== preset.colors2 || preset.colors2 !== preset.colors3
      if (!animatesColors) return@LaunchedEffect
      var targetStage = floor(colorStage.value) + 1f
      while (isActive) {
        delay((preset.colorInterpPeriod * 500).toLong())
        colorStage.animateTo(
            targetValue = targetStage,
            animationSpec = spring(dampingRatio = 0.9f, stiffness = 35f),
        )
        targetStage += 1f
      }
    }

    Spacer(
        modifier =
            Modifier.fillMaxSize()
                .bgEffectDraw(
                    painter = painter,
                    preset = preset,
                    deviceType = deviceType,
                    isDarkTheme = isDarkTheme,
                    surface = surface,
                    effectBackground = effectBackground,
                    isFullSize = isFullSize,
                    playing = dynamicBackground,
                    colorStage = { colorStage.value },
                    alpha = alpha,
                ),
    )
    content()
  }
}

@Composable
private fun shouldShowSplitPane(): Boolean {
  val windowInfo = LocalWindowInfo.current
  val density = LocalDensity.current
  return with(density) {
    val widthDp = windowInfo.containerSize.width.toDp()
    val heightDp = windowInfo.containerSize.height.toDp()
    val ratio = heightDp / widthDp
    widthDp >= 840.dp || (widthDp >= 600.dp && ratio < 1.2f)
  }
}
