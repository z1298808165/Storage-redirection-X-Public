package org.srx.manager.ui.component

import android.content.pm.ApplicationInfo
import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import org.srx.manager.ui.AppIconCache
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
fun AppIconImage(
    modifier: Modifier = Modifier,
    appInfo: ApplicationInfo?,
    label: String,
) {
    if (appInfo == null) {
        PlaceholderIcon(label, modifier)
        return
    }
    val density = LocalDensity.current
    val context = LocalContext.current
    val targetSizePx = with(density) { 48.dp.roundToPx() }
    val cached = remember(appInfo.packageName, appInfo.uid, appInfo.sourceDir) { AppIconCache.get(appInfo) }
    var bitmap by remember(appInfo.packageName, appInfo.uid, appInfo.sourceDir) { mutableStateOf(cached) }
    LaunchedEffect(appInfo.packageName, appInfo.uid, appInfo.sourceDir) {
        if (bitmap == null) bitmap = AppIconCache.load(context, appInfo, targetSizePx)
    }
    Crossfade(targetState = bitmap, animationSpec = tween(150), label = "AppIconFade") { icon ->
        if (icon == null) PlaceholderIcon(label, modifier) else {
            val shape = RoundedCornerShape(17.dp)
            val dark = isSrxDarkTheme()
            Image(
                bitmap = icon.asImageBitmap(),
                contentDescription = label,
                contentScale = ContentScale.Fit,
                modifier = modifier
                    .dropShadow(shape, Shadow(radius = 10.dp, color = if (dark) Color.Black else MiuixTheme.colorScheme.primary, alpha = if (dark) 0.14f else 0.08f))
                    .clip(shape)
                    .background(MiuixTheme.colorScheme.surfaceContainerHigh.copy(alpha = if (dark) 0.46f else 0.62f))
                    .padding(4.dp),
            )
        }
    }
}

@Composable
private fun PlaceholderIcon(label: String, modifier: Modifier) {
    val initial = label.trim().firstOrNull()?.uppercaseChar()?.toString() ?: "?"
    val dark = isSrxDarkTheme()
    val shape = RoundedCornerShape(17.dp)
    Box(
        modifier = modifier
            .padding(2.dp)
            .dropShadow(shape, Shadow(radius = 10.dp, color = if (dark) Color.Black else MiuixTheme.colorScheme.primary, alpha = if (dark) 0.14f else 0.08f))
            .clip(shape)
            .background(
                Brush.linearGradient(
                    listOf(
                        MiuixTheme.colorScheme.primary,
                        MiuixTheme.colorScheme.primary.copy(alpha = 0.72f),
                        MiuixTheme.colorScheme.error.copy(alpha = 0.72f),
                    ),
                ),
            ),
        contentAlignment = androidx.compose.ui.Alignment.Center,
    ) {
        Text(
            text = initial,
            color = Color.White,
            fontWeight = FontWeight.Bold,
            style = MiuixTheme.textStyles.title4,
        )
    }
}
