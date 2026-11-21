package org.srx.manager.ui.screen

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.GlassCard
import org.srx.manager.SectionTitle
import org.srx.manager.SrxProgressiveListPage
import org.srx.manager.data.formatCompactRuntimeActivationCount
import org.srx.manager.ui.AppUiState
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.Info
import top.yukonga.miuix.kmp.icon.glass.Refresh
import top.yukonga.miuix.kmp.icon.glass.Update
import top.yukonga.miuix.kmp.theme.MiuixTheme

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
  var showHostPolicyDetails by remember { mutableStateOf(false) }
  var pendingRuntimeStatsReset by remember { mutableStateOf(false) }
  SrxProgressiveListPage(
      title = "概览",
      bottomPadding = bottomPadding,
      verticalArrangement = Arrangement.spacedBy(16.dp),
      actions = {
        Text(
            text = state.dashboard.version.ifBlank { "--" },
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
        )
      },
  ) {
    item {
      ModuleStatusCard(
          status = state.dashboard.status,
          globalConfig = state.dashboard.globalConfig,
          enabledApps = state.dashboard.enabledApps,
          runtimeActivations =
              formatCompactRuntimeActivationCount(state.dashboard.runtimeActivations),
          storageBackend = state.dashboard.storageBackend,
          onToggleModule = { pendingModuleToggle = it },
          onRuntimeActivationClick = { showRuntimeActivationDetails = true },
          onRuntimeActivationLongClick = { pendingRuntimeStatsReset = true },
          onHostPolicyClick = { showHostPolicyDetails = true },
      )
    }
    item {
      SectionTitle("快速入口")
      GlassCard(
          insideMargin = PaddingValues(0.dp),
      ) {
        ActionRow(
            "重新挂载运行中应用",
            MiuixIcons.Glass.Refresh,
            { pendingMediaProviderRestart = true },
        )
        ActionRow("检查更新", MiuixIcons.Glass.Update, onOpenUpdate)
        ActionRow("关于与开源协议", MiuixIcons.Glass.Info, onOpenAbout)
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
  if (showHostPolicyDetails) {
    HostPolicyDetailsDialog(
        storageBackend = state.dashboard.storageBackend,
        onDismiss = { showHostPolicyDetails = false },
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
