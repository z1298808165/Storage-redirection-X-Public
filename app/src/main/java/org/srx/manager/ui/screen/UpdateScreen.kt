package org.srx.manager.ui.screen

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.BuildConfig
import org.srx.manager.GlassCard
import org.srx.manager.GlassTextButton
import org.srx.manager.SectionTitle
import org.srx.manager.SettingGroup
import org.srx.manager.SrxProgressiveListPage
import org.srx.manager.data.UiPreferences
import org.srx.manager.data.UpdateChannel
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.Refresh
import top.yukonga.miuix.kmp.icon.glass.SettingsOutline
import top.yukonga.miuix.kmp.theme.MiuixTheme

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
  val releaseRepositoryUrl = remember { "https://github.com/${BuildConfig.RELEASE_REPOSITORY}" }
  val officialRepositoryUrl = remember {
    "https://github.com/${BuildConfig.OFFICIAL_RELEASE_REPOSITORY}"
  }
  val bottomPadding = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
  SrxProgressiveListPage(
      title = "检查更新",
      onBack = onBack,
      bottomPadding = bottomPadding,
      topSpacing = 18.dp,
  ) {
    item {
      SectionTitle("更新")
      SettingGroup() {
        item {
          CompactSwitchRow(
              title = "启动时检查更新",
              icon = MiuixIcons.Glass.Refresh,
              checked = prefs.autoCheckUpdates,
              onCheckedChange = onAutoCheckUpdates,
          )
        }
        item {
          SettingMenuRow(
              title = "更新通道",
              leading = { SettingsRowIcon(MiuixIcons.Glass.SettingsOutline) },
              value = updateChannelLabel(prefs.updateChannel),
              options = UpdateChannelOptions,
              selected = prefs.updateChannel,
              onSelect = onUpdateChannel,
              showDivider = false,
          )
        }
      }
    }
    item {
      SettingGroup(insideMargin = PaddingValues(16.dp)) {
        item {
          Column {
            // App 与模块是两条独立的更新线：两个版本都要显示，用户才知道自己缺的是哪一个。
            Text(
                text = "当前 App 版本 ${BuildConfig.VERSION_NAME}",
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                fontSize = 13.sp,
                lineHeight = 18.sp,
                fontWeight = FontWeight.SemiBold,
            )
            Spacer(Modifier.height(4.dp))
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
}

@Composable
private fun RepositoryCard(
    title: String,
    url: String,
    onClick: () -> Unit,
) {
  GlassCard(
      modifier = Modifier.fillMaxWidth().clickable(onClick = onClick),
      insideMargin = PaddingValues(horizontal = 16.dp, vertical = 12.dp),
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

internal val UpdateChannelOptions =
    listOf(
        UpdateChannel.Stable to "正式版",
        UpdateChannel.Beta to "测试版",
        UpdateChannel.All to "全通道最新版",
    )

internal fun updateChannelLabel(channel: UpdateChannel): String =
    UpdateChannelOptions.firstOrNull { it.first == channel }?.second ?: "正式版"
