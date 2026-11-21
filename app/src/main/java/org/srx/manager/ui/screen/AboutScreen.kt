package org.srx.manager.ui.screen

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.GlassCard
import org.srx.manager.RoundIconAction
import org.srx.manager.srxRememberListState
import org.srx.manager.ui.effect.BgEffectBackground
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.Back
import top.yukonga.miuix.kmp.material.isRuntimeShaderSupported
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.utils.overScrollVertical

@Composable
internal fun AboutScreen(onBack: () -> Unit) {
  val uriHandler = LocalUriHandler.current
  val effectBackground =
      isRuntimeShaderSupported() &&
          android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.VANILLA_ICE_CREAM
  val listState = srxRememberListState("about")
  val topInset = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
  val bottomInset = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
  BgEffectBackground(
      dynamicBackground = effectBackground,
      modifier = Modifier.fillMaxSize(),
      isFullSize = true,
      effectBackground = effectBackground,
  ) {
    LazyColumn(
        state = listState,
        modifier = Modifier.fillMaxSize().overScrollVertical(),
        contentPadding =
            PaddingValues(
                top = topInset + 72.dp,
                bottom = bottomInset + 56.dp,
                start = 16.dp,
                end = 16.dp,
            ),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
      item { AboutTitle("开源协议引用") }
      items(LicenseItems) { item -> LicenseCard(item, uriHandler::openUri) }
    }
    Box(
        modifier =
            Modifier.align(Alignment.TopStart).padding(start = 16.dp, top = topInset + 12.dp),
    ) {
      RoundIconAction(MiuixIcons.Glass.Back, "返回", onBack)
    }
  }
}

internal data class LicenseItem(
    val group: String,
    val name: String,
    val license: String,
    val url: String,
)

internal val LicenseItems =
    listOf(
        LicenseItem(
            "模块核心",
            "SRX Core",
            "GPL-3.0-or-later",
            "https://github.com/z1298808165/storage-redirect-x",
        ),
        LicenseItem(
            "模块核心",
            "Storage-redirection-X-Public",
            "GPL-3.0-or-later",
            "https://github.com/Kindness-Kismet/Storage-redirection-X-Public",
        ),
        LicenseItem("模块核心", "srx_hook", "MIT", "https://github.com/Kindness-Kismet/srx_hook"),
        LicenseItem(
            "模块核心",
            "srx_inline_hook",
            "MIT",
            "https://github.com/Kindness-Kismet/srx_inline_hook",
        ),
        LicenseItem(
            "模块核心",
            "fusefixer",
            "MIT",
            "https://github.com/MaterialCleaner/Media-Provider-FuseFixer",
        ),
        LicenseItem(
            "模块核心",
            "Fuse-Proxy",
            "GPL-3.0",
            "https://github.com/huniangitb/Fuse-Proxy",
        ),
        LicenseItem(
            "Root / WebUI",
            "KernelSU",
            "GPL 3.0 / GPL 2.0",
            "https://github.com/tiann/KernelSU",
        ),
        LicenseItem("Hook 与 DEX", "LSPlant", "LGPL 3.0", "https://github.com/LSPosed/LSPlant"),
        LicenseItem(
            "Hook 与 DEX",
            "DexBuilder",
            "Apache 2.0",
            "https://android.googlesource.com/platform/tools/dexter",
        ),
        LicenseItem(
            "Hook 与 DEX",
            "parallel-hashmap",
            "Apache 2.0",
            "https://github.com/greg7mdp/parallel-hashmap",
        ),
        LicenseItem(
            "Hook 与 DEX",
            "abseil-cpp",
            "Apache 2.0",
            "https://github.com/abseil/abseil-cpp",
        ),
        LicenseItem("Rust 依赖", "libc", "MIT / Apache 2.0", "https://github.com/rust-lang/libc"),
        LicenseItem("Rust 依赖", "jni-sys", "MIT / Apache 2.0", "https://github.com/jni-rs/jni-sys"),
        LicenseItem("Rust 依赖", "serde", "MIT / Apache 2.0", "https://github.com/serde-rs/serde"),
        LicenseItem(
            "Rust 依赖",
            "serde_json",
            "MIT / Apache 2.0",
            "https://github.com/serde-rs/json",
        ),
        LicenseItem(
            "Rust 依赖",
            "once_cell",
            "MIT / Apache 2.0",
            "https://github.com/matklad/once_cell",
        ),
        LicenseItem("Rust 依赖", "log", "MIT / Apache 2.0", "https://github.com/rust-lang/log"),
        LicenseItem(
            "APP / UI",
            "AndroidX / Jetpack Compose",
            "Apache 2.0",
            "https://github.com/androidx/androidx",
        ),
        LicenseItem("APP / UI", "Miuix", "Apache 2.0", "https://github.com/compose-miuix-ui/miuix"),
        LicenseItem("APP / UI", "COUI", "Apache 2.0", "https://github.com/suqi8/coui"),
        LicenseItem(
            "APP / UI",
            "AndroidLiquidGlass / Backdrop",
            "Apache 2.0",
            "https://github.com/Kyant0/AndroidLiquidGlass",
        ),
        LicenseItem(
            "APP / UI",
            "AppIconLoader",
            "Apache 2.0",
            "https://github.com/zhanghai/AppIconLoader",
        ),
        LicenseItem(
            "APP / Kotlin",
            "kotlinx.coroutines",
            "Apache 2.0",
            "https://github.com/Kotlin/kotlinx.coroutines",
        ),
        LicenseItem(
            "APP / Kotlin",
            "kotlinx.serialization",
            "Apache 2.0",
            "https://github.com/Kotlin/kotlinx.serialization",
        ),
        LicenseItem(
            "APP / 系统兼容",
            "AndroidHiddenApiBypass",
            "Apache 2.0",
            "https://github.com/LSPosed/AndroidHiddenApiBypass",
        ),
    )

@Composable
private fun LicenseCard(item: LicenseItem, onOpen: (String) -> Unit) {
  GlassCard(
      modifier = Modifier.fillMaxWidth().heightIn(min = 68.dp).clickable { onOpen(item.url) },
      insideMargin = PaddingValues(horizontal = 16.dp, vertical = 11.dp),
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
