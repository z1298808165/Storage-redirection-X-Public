package org.srx.manager.ui.screen

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.statusBars
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.BackPageHeader
import org.srx.manager.BuildConfig
import org.srx.manager.GlassCard
import org.srx.manager.GlassTextButton
import org.srx.manager.SectionTitle
import org.srx.manager.appMeshBackground
import org.srx.manager.data.UpdateChannel
import org.srx.manager.data.UiPreferences
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.utils.overScrollVertical

@Composable
internal fun UpdateScreen(
    prefs: UiPreferences,
    moduleVersion: String,
    updateCheckRunning: Boolean,
    onBack: () -> Unit,
    onAutoCheckUpdates: (Boolean) -> Unit,
    onUpdateChannel: (UpdateChannel) -> Unit,
    onCheckNow: () -> Unit,
) {
    val uriHandler = LocalUriHandler.current
    var showChannelPicker by remember { mutableStateOf(false) }
    val releaseRepositoryUrl = remember { "https://github.com/${BuildConfig.RELEASE_REPOSITORY}" }
    val officialRepositoryUrl = remember { "https://github.com/${BuildConfig.OFFICIAL_RELEASE_REPOSITORY}" }
    LazyUpdateLayout(
        header = {
            BackPageHeader(
                title = "检查更新",
                onBack = onBack,
            )
        },
    ) {
        item {
            SectionTitle("更新")
            GlassCard(alpha = 0.58f) {
                CompactSwitchRow(
                    title = "启动时检查更新",
                    summary = "打开应用后自动检查所选通道的新版本",
                    checked = prefs.autoCheckUpdates,
                    onCheckedChange = onAutoCheckUpdates,
                )
                SettingSelectRow(
                    title = "更新通道",
                    summary = "选择正式版、测试版或所有通道中的最新版本",
                    value = updateChannelLabel(prefs.updateChannel),
                    onClick = { showChannelPicker = true },
                    showDivider = false,
                )
            }
        }
        item {
            GlassCard(insideMargin = PaddingValues(16.dp), alpha = 0.58f) {
                Text(
                    text = "当前模块版本 ${moduleVersion.ifBlank { "--" }}",
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    fontSize = 13.sp,
                    lineHeight = 18.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                Spacer(Modifier.height(12.dp))
                GlassTextButton(
                    text = if (updateCheckRunning) "正在检查" else "立即检查",
                    onClick = { if (!updateCheckRunning) onCheckNow() },
                    modifier = Modifier.fillMaxWidth(),
                    primary = true,
                )
            }
        }
        item {
            SectionTitle("发布仓库")
            RepositoryCard(
                title = "当前检查仓库",
                url = releaseRepositoryUrl,
                onClick = { runCatching { uriHandler.openUri(releaseRepositoryUrl) } },
            )
        }
        item {
            RepositoryCard(
                title = "官方发布仓库",
                url = officialRepositoryUrl,
                onClick = { runCatching { uriHandler.openUri(officialRepositoryUrl) } },
            )
        }
    }
    SettingOptionDialog(
        show = showChannelPicker,
        title = "更新通道",
        options = UpdateChannelOptions,
        selected = prefs.updateChannel,
        onDismiss = { showChannelPicker = false },
        onSelect = {
            onUpdateChannel(it)
            showChannelPicker = false
        },
    )
}

@Composable
private fun RepositoryCard(
    title: String,
    url: String,
    onClick: () -> Unit,
) {
    GlassCard(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onClick),
        insideMargin = PaddingValues(horizontal = 16.dp, vertical = 12.dp),
        alpha = 0.58f,
    ) {
        Text(
            text = title,
            color = MiuixTheme.colorScheme.onSurface,
            fontSize = 14.sp,
            lineHeight = 19.sp,
            fontWeight = FontWeight.Bold,
        )
        Spacer(Modifier.height(4.dp))
        Text(
            text = url,
            color = MiuixTheme.colorScheme.primary,
            fontSize = 12.sp,
            lineHeight = 17.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

@Composable
private fun LazyUpdateLayout(
    header: @Composable () -> Unit,
    content: androidx.compose.foundation.lazy.LazyListScope.() -> Unit,
) {
    androidx.compose.foundation.lazy.LazyColumn(
        modifier = Modifier
            .fillMaxSize()
            .appMeshBackground()
            .overScrollVertical(),
        contentPadding = PaddingValues(
            top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 18.dp,
            bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 28.dp,
            start = 16.dp,
            end = 16.dp,
        ),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        item { header() }
        content()
    }
}

internal val UpdateChannelOptions = listOf(
    UpdateChannel.Stable to "正式版",
    UpdateChannel.Beta to "测试版",
    UpdateChannel.All to "全通道最新版",
)

internal fun updateChannelLabel(channel: UpdateChannel): String =
    UpdateChannelOptions.firstOrNull { it.first == channel }?.second ?: "正式版"
