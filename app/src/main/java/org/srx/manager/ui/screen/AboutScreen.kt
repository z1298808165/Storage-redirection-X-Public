package org.srx.manager.ui.screen

import android.os.Build
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.BackPageHeader
import org.srx.manager.GlassCard
import org.srx.manager.ui.effect.BgEffectBackground
import org.srx.manager.ui.theme.isSrxBlurEffectEnabled
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.blur.isRuntimeShaderSupported
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.utils.overScrollVertical

@Composable
internal fun AboutScreen(onBack: () -> Unit) {
    val uriHandler = LocalUriHandler.current
    val dynamicBackground = isSrxBlurEffectEnabled() &&
        isRuntimeShaderSupported() &&
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM
    BgEffectBackground(
        dynamicBackground = dynamicBackground,
        modifier = Modifier.fillMaxSize(),
        bgModifier = Modifier.aboutFlowingBackground(),
        isFullSize = true,
        effectBackground = dynamicBackground,
    ) {
        LazyColumn(
            modifier = Modifier
                .fillMaxSize()
                .overScrollVertical(),
            contentPadding = PaddingValues(
                top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 18.dp,
                bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 56.dp,
                start = 16.dp,
                end = 16.dp,
            ),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                BackPageHeader(
                    title = "关于",
                    onBack = onBack,
                )
            }
            item { AboutTitle("开源协议引用") }
            items(LicenseItems) { item ->
                LicenseCard(item, uriHandler::openUri)
            }
        }
    }
}

private data class LicenseItem(val group: String, val name: String, val license: String, val url: String)

private val LicenseItems = listOf(
    LicenseItem("模块核心", "SRX Core", "GPL-3.0-or-later", "https://github.com/z1298808165/storage-redirect-x"),
    LicenseItem("模块核心", "Storage-redirection-X-Public", "GPL-3.0-or-later", "https://github.com/Kindness-Kismet/Storage-redirection-X-Public"),
    LicenseItem("模块核心", "srx_hook", "MIT", "https://github.com/Kindness-Kismet/srx_hook"),
    LicenseItem("模块核心", "srx_inline_hook", "MIT", "https://github.com/Kindness-Kismet/srx_inline_hook"),
    LicenseItem("模块核心", "fusefixer", "MIT", "https://github.com/MaterialCleaner/Media-Provider-FuseFixer"),
    LicenseItem("Root / WebUI", "KernelSU", "GPL 3.0 / GPL 2.0", "https://github.com/tiann/KernelSU"),
    LicenseItem("Hook 与 DEX", "LSPlant", "LGPL 3.0", "https://github.com/LSPosed/LSPlant"),
    LicenseItem("Hook 与 DEX", "DexBuilder", "Apache 2.0", "https://android.googlesource.com/platform/tools/dexter"),
    LicenseItem("Hook 与 DEX", "parallel-hashmap", "Apache 2.0", "https://github.com/greg7mdp/parallel-hashmap"),
    LicenseItem("Hook 与 DEX", "abseil-cpp", "Apache 2.0", "https://github.com/abseil/abseil-cpp"),
    LicenseItem("Rust 依赖", "libc", "MIT / Apache 2.0", "https://github.com/rust-lang/libc"),
    LicenseItem("Rust 依赖", "jni-sys", "MIT / Apache 2.0", "https://github.com/jni-rs/jni-sys"),
    LicenseItem("Rust 依赖", "serde", "MIT / Apache 2.0", "https://github.com/serde-rs/serde"),
    LicenseItem("Rust 依赖", "serde_json", "MIT / Apache 2.0", "https://github.com/serde-rs/json"),
    LicenseItem("Rust 依赖", "once_cell", "MIT / Apache 2.0", "https://github.com/matklad/once_cell"),
    LicenseItem("Rust 依赖", "log", "MIT / Apache 2.0", "https://github.com/rust-lang/log"),
    LicenseItem("APP / UI", "AndroidX / Jetpack Compose", "Apache 2.0", "https://github.com/androidx/androidx"),
    LicenseItem("APP / UI", "Miuix", "Apache 2.0", "https://github.com/compose-miuix-ui/miuix"),
    LicenseItem("APP / UI", "AppIconLoader", "Apache 2.0", "https://github.com/zhanghai/AppIconLoader"),
    LicenseItem("APP / Kotlin", "kotlinx.coroutines", "Apache 2.0", "https://github.com/Kotlin/kotlinx.coroutines"),
    LicenseItem("APP / Kotlin", "kotlinx.serialization", "Apache 2.0", "https://github.com/Kotlin/kotlinx.serialization"),
    LicenseItem("APP / 系统兼容", "AndroidHiddenApiBypass", "Apache 2.0", "https://github.com/LSPosed/AndroidHiddenApiBypass"),
)

@Composable
private fun LicenseCard(item: LicenseItem, onOpen: (String) -> Unit) {
    GlassCard(
        modifier = Modifier
            .fillMaxWidth()
            .heightIn(min = 68.dp)
            .clickable { onOpen(item.url) },
        cornerRadius = 20.dp,
        insideMargin = PaddingValues(horizontal = 16.dp, vertical = 11.dp),
        alpha = 0.62f,
    ) {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Column(
                modifier = Modifier.weight(1f),
                verticalArrangement = Arrangement.spacedBy(3.dp),
            ) {
                Text(
                    text = item.name,
                    fontSize = 16.sp,
                    lineHeight = 19.sp,
                    fontWeight = FontWeight.Black,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = item.group,
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    fontSize = 11.sp,
                    lineHeight = 13.sp,
                    fontWeight = FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text(
                text = item.license,
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                fontSize = 13.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
            )
        }
    }
}

@Composable
private fun AboutTitle(text: String) {
    Text(
        text = text,
        modifier = Modifier.padding(start = 6.dp, bottom = 2.dp),
        color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
        fontSize = 13.sp,
        fontWeight = FontWeight.Bold,
    )
}

@Composable
private fun Modifier.aboutFlowingBackground(): Modifier {
    val dark = isSrxDarkTheme()
    val transition = rememberInfiniteTransition(label = "AboutFlow")
    val flow by transition.animateFloat(
        initialValue = 0f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 8800, easing = LinearEasing),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "AboutFlowShift",
    )
    val drift by transition.animateFloat(
        initialValue = 1f,
        targetValue = 0f,
        animationSpec = infiniteRepeatable(
            animation = tween(durationMillis = 13200, easing = LinearEasing),
            repeatMode = RepeatMode.Reverse,
        ),
        label = "AboutFlowDrift",
    )
    return drawBehind {
        val w = size.width.coerceAtLeast(1f)
        val h = size.height.coerceAtLeast(1f)
        drawRect(
            brush = if (dark) {
                Brush.linearGradient(
                    listOf(Color(0xFF08101C), Color(0xFF1C1730), Color(0xFF112C28)),
                    start = Offset.Zero,
                    end = Offset(w, h),
                )
            } else {
                Brush.linearGradient(
                    listOf(Color(0xFFFCFBFF), Color(0xFFEFF8FF), Color(0xFFFFF4FB), Color(0xFFEFFFF8)),
                    start = Offset.Zero,
                    end = Offset(w, h),
                )
            },
        )
        drawRect(
            brush = Brush.radialGradient(
                listOf(
                    if (dark) Color(0x883A84FF) else Color(0x88A9D6FF),
                    if (dark) Color(0x443A84FF) else Color(0x50BFD6FF),
                    Color.Transparent,
                ),
                center = Offset(w * (0.12f + 0.46f * flow), h * (0.16f + 0.18f * drift)),
                radius = w * 0.72f,
            ),
        )
        drawRect(
            brush = Brush.radialGradient(
                listOf(
                    if (dark) Color(0x77D75BC3) else Color(0x72F3B6EA),
                    if (dark) Color(0x33D75BC3) else Color(0x3DF7D9F0),
                    Color.Transparent,
                ),
                center = Offset(w * (0.78f - 0.36f * drift), h * (0.14f + 0.38f * flow)),
                radius = w * 0.64f,
            ),
        )
        drawRect(
            brush = Brush.radialGradient(
                listOf(
                    if (dark) Color(0x6630C5A5) else Color(0x86CFF6E7),
                    if (dark) Color(0x3330C5A5) else Color(0x4EEBFFF6),
                    Color.Transparent,
                ),
                center = Offset(w * (0.86f - 0.3f * flow), h * (0.78f - 0.28f * drift)),
                radius = w * 0.7f,
            ),
        )
        drawRect(
            brush = Brush.radialGradient(
                listOf(
                    if (dark) Color(0x42DF8E3E) else Color(0x5CFFCE7D),
                    Color.Transparent,
                ),
                center = Offset(w * (0.28f + 0.36f * drift), h * (0.9f - 0.42f * flow)),
                radius = w * 0.52f,
            ),
        )
        drawRect(
            brush = Brush.linearGradient(
                listOf(
                    Color.Transparent,
                    if (dark) Color(0x2C89B4FF) else Color.White.copy(alpha = 0.58f),
                    Color.Transparent,
                    if (dark) Color(0x24FF6CBE) else Color(0x40FF74C2),
                    Color.Transparent,
                ),
                start = Offset(w * (1f - flow), h * 0.08f),
                end = Offset(w * flow, h * 0.95f),
            ),
        )
    }
}
