package org.srx.manager.ui.screen

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import org.srx.manager.GlassCard
import org.srx.manager.PageHeader
import org.srx.manager.data.formatCompactRuntimeActivationCount
import org.srx.manager.ui.AppUiState
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Info
import top.yukonga.miuix.kmp.icon.extended.Refresh
import top.yukonga.miuix.kmp.icon.extended.Update
import top.yukonga.miuix.kmp.utils.overScrollVertical

@Composable
internal fun DashboardScreen(
    state: AppUiState,
    bottomPadding: Dp,
    onToggleModule: (Boolean) -> Unit,
    onRestartMediaProvider: () -> Unit,
    onResetRuntimeStats: () -> Unit,
    onOpenAbout: () -> Unit,
    onOpenUpdate: () -> Unit,
) {
  var pendingModuleToggle by remember { mutableStateOf<Boolean?>(null) }
  var pendingMediaProviderRestart by remember { mutableStateOf(false) }
  var showRuntimeActivationDetails by remember { mutableStateOf(false) }
  var pendingRuntimeStatsReset by remember { mutableStateOf(false) }
  LazyColumn(
      modifier = Modifier.fillMaxSize().overScrollVertical(),
      contentPadding =
          PaddingValues(
              top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 18.dp,
              bottom = bottomPadding + 28.dp,
              start = 16.dp,
              end = 16.dp,
          ),
      verticalArrangement = Arrangement.spacedBy(16.dp),
  ) {
    item {
      PageHeader(
          title = "概览",
          trailing = state.dashboard.version.ifBlank { "--" },
      )
    }
    item {
      ModuleStatusCard(
          status = state.dashboard.status,
          globalConfig = state.dashboard.globalConfig,
          enabledApps = state.dashboard.enabledApps,
          runtimeActivations =
              formatCompactRuntimeActivationCount(state.dashboard.runtimeActivations),
          onToggleModule = { pendingModuleToggle = it },
          onRuntimeActivationClick = { showRuntimeActivationDetails = true },
          onRuntimeActivationLongClick = { pendingRuntimeStatsReset = true },
      )
    }
    item {
      SectionTitle("快速入口")
      GlassCard(
          insideMargin = PaddingValues(0.dp),
          cornerRadius = 28.dp,
      ) {
        ActionRow(
            "重新挂载运行中应用",
            "刷新已配置应用的重定向挂载并同步媒体进程",
            MiuixIcons.Refresh,
            { pendingMediaProviderRestart = true },
        )
        ActionRow("检查更新", "检查可用的新版本", MiuixIcons.Update, onOpenUpdate)
        ActionRow("关于与开源协议", "查看模块来源、依赖项目与开源协议", MiuixIcons.Info, onOpenAbout)
      }
    }
  }
  pendingModuleToggle?.let { enable ->
    ModuleToggleConfirmDialog(
        enable = enable,
        show = true,
        onDismiss = { pendingModuleToggle = null },
        onConfirm = {
          pendingModuleToggle = null
          onToggleModule(enable)
        },
    )
  }
  if (pendingMediaProviderRestart) {
    RestartMediaProviderConfirmDialog(
        show = true,
        onDismiss = { pendingMediaProviderRestart = false },
        onConfirm = {
          pendingMediaProviderRestart = false
          onRestartMediaProvider()
        },
    )
  }
  if (showRuntimeActivationDetails) {
    RuntimeActivationDetailsDialog(
        exactValue = state.dashboard.runtimeActivations,
        onDismiss = { showRuntimeActivationDetails = false },
    )
  }
  if (pendingRuntimeStatsReset) {
    ResetRuntimeStatsConfirmDialog(
        onDismiss = { pendingRuntimeStatsReset = false },
        onConfirm = {
          pendingRuntimeStatsReset = false
          onResetRuntimeStats()
        },
    )
  }
}

internal data class StatusUi(
    val label: String,
    val color: Color,
    val backgroundAlpha: Float = 0.13f,
)
