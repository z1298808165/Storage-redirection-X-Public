package org.srx.manager.ui.screen

import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.CenteredDialog
import org.srx.manager.GlassCard
import org.srx.manager.GlassTextButton
import org.srx.manager.PageHeader
import org.srx.manager.R
import org.srx.manager.data.GlobalConfig
import org.srx.manager.data.ModuleStatus
import org.srx.manager.data.formatCompactRuntimeActivationCount
import org.srx.manager.glassPanel
import org.srx.manager.glassSurfaceColor
import org.srx.manager.isSrxGlassBackdropEnabled
import org.srx.manager.srxSuccessColor
import org.srx.manager.srxWarningColor
import org.srx.manager.ui.AppUiState
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.CardDefaults
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Info
import top.yukonga.miuix.kmp.icon.extended.Refresh
import top.yukonga.miuix.kmp.icon.extended.Update
import top.yukonga.miuix.kmp.theme.MiuixTheme
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
            "快速重启 MediaProvider",
            "清除媒体进程缓存并重新加载模块 hook",
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

private data class StatusUi(
    val label: String,
    val color: Color,
    val backgroundAlpha: Float = 0.13f,
)

@Composable
private fun moduleStatusUi(status: ModuleStatus): StatusUi =
    when (status) {
      ModuleStatus.Enabled -> StatusUi("模块已激活", srxSuccessColor())
      ModuleStatus.Disabled ->
          StatusUi("模块已停止", MiuixTheme.colorScheme.onSurfaceVariantSummary, 0.12f)
      ModuleStatus.RebootRequired -> StatusUi("需要重启", srxWarningColor())
      ModuleStatus.Unknown -> StatusUi("状态未知", srxWarningColor())
    }

@Composable
@OptIn(ExperimentalLayoutApi::class)
private fun ModuleStatusCard(
    status: ModuleStatus,
    globalConfig: GlobalConfig,
    enabledApps: Int,
    runtimeActivations: String,
    onToggleModule: (Boolean) -> Unit,
    onRuntimeActivationClick: () -> Unit,
    onRuntimeActivationLongClick: () -> Unit,
) {
  val colors = MiuixTheme.colorScheme
  val statusUi = moduleStatusUi(status)
  val canToggle = status == ModuleStatus.Enabled || status == ModuleStatus.Disabled
  val shape = RoundedCornerShape(34.dp)
  val useBackdrop = isSrxGlassBackdropEnabled()
  Card(
      modifier =
          Modifier.fillMaxWidth()
              .heightIn(min = 348.dp)
              .glassPanel(shape, shadowAlpha = 0.1f, surfaceAlpha = 0.72f),
      cornerRadius = 34.dp,
      insideMargin = PaddingValues(horizontal = 24.dp, vertical = 30.dp),
      colors =
          CardDefaults.defaultColors(
              color = if (useBackdrop) Color.Transparent else glassSurfaceColor(0.72f)
          ),
  ) {
    Column(
        modifier = Modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(14.dp, Alignment.CenterVertically),
    ) {
      Image(
          painter = painterResource(R.drawable.srx_logo_vector),
          contentDescription = null,
          modifier = Modifier.size(72.dp).clip(RoundedCornerShape(18.dp)),
          contentScale = ContentScale.Fit,
      )
      Text(
          stringResource(R.string.app_name),
          style = MiuixTheme.textStyles.title3,
          fontWeight = FontWeight.Black,
          fontSize = 20.sp,
          lineHeight = 24.sp,
      )
      ModuleStatusPill(
          statusUi = statusUi,
          enabled = canToggle,
          onClick = { onToggleModule(status != ModuleStatus.Enabled) },
      )
      FlowRow(
          horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
          verticalArrangement = Arrangement.spacedBy(8.dp),
          modifier = Modifier.fillMaxWidth(),
      ) {
        FeatureChip("文件监控", globalConfig.fileMonitorEnabled, Modifier.widthIn(min = 98.dp))
        FeatureChip("FuseFixer", globalConfig.fuseFixEnabled, Modifier.widthIn(min = 106.dp))
        FeatureChip("详细日志", globalConfig.verboseLoggingEnabled, Modifier.widthIn(min = 92.dp))
      }
      Box(
          modifier =
              Modifier.fillMaxWidth()
                  .padding(top = 2.dp)
                  .height(1.dp)
                  .background(
                      colors.onSurface.copy(alpha = if (isSrxDarkTheme()) 0.055f else 0.07f)
                  ),
      )
      Row(modifier = Modifier.fillMaxWidth()) {
        MetricBox("已启用应用", enabledApps.toString(), Modifier.weight(1f))
        Box(
            Modifier.width(1.dp)
                .height(52.dp)
                .background(colors.onSurface.copy(alpha = if (isSrxDarkTheme()) 0.045f else 0.06f)),
        )
        MetricBox(
            "生效次数",
            runtimeActivations,
            Modifier.weight(1f)
                .combinedClickable(
                    onClickLabel = "查看精确生效次数",
                    onLongClickLabel = "清除生效次数",
                    onLongClick = onRuntimeActivationLongClick,
                    onClick = onRuntimeActivationClick,
                ),
        )
      }
    }
  }
}

@Composable
private fun ModuleStatusPill(
    statusUi: StatusUi,
    enabled: Boolean,
    onClick: () -> Unit,
) {
  Text(
      text = statusUi.label,
      modifier =
          Modifier.clip(CircleShape)
              .clickable(
                  enabled = enabled,
                  interactionSource = null,
                  indication = null,
                  onClick = onClick,
              )
              .background(statusUi.color.copy(alpha = statusUi.backgroundAlpha), CircleShape)
              .padding(horizontal = 14.dp, vertical = 5.dp),
      color = statusUi.color,
      fontSize = 13.sp,
      lineHeight = 16.sp,
      fontWeight = FontWeight.Black,
      textAlign = TextAlign.Center,
      maxLines = 1,
      overflow = TextOverflow.Ellipsis,
  )
}

@Composable
private fun ModuleToggleConfirmDialog(
    enable: Boolean,
    show: Boolean,
    onDismiss: () -> Unit,
    onConfirm: () -> Unit,
) {
  val message =
      if (enable) {
        "启动模块会恢复配置应用和 MediaProvider hook，受保护管理器会跳过。是否继续？"
      } else {
        "停止模块会结束相关进程并退出 hook，受保护管理器会跳过。是否继续？"
      }
  CenteredDialog(
      show = show,
      onDismiss = onDismiss,
  ) {
    Column(
        modifier = Modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(22.dp),
    ) {
      Text(
          text = message,
          modifier = Modifier.fillMaxWidth(),
          color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
          fontSize = 15.sp,
          lineHeight = 22.sp,
          fontWeight = FontWeight.Medium,
          textAlign = TextAlign.Center,
      )
      Row(
          modifier = Modifier.fillMaxWidth(),
          horizontalArrangement = Arrangement.spacedBy(14.dp),
      ) {
        GlassTextButton("取消", onDismiss, modifier = Modifier.weight(1f))
        GlassTextButton("确认", onConfirm, modifier = Modifier.weight(1f), primary = true)
      }
    }
  }
}

@Composable
private fun RestartMediaProviderConfirmDialog(
    show: Boolean,
    onDismiss: () -> Unit,
    onConfirm: () -> Unit,
) {
  CenteredDialog(
      show = show,
      onDismiss = onDismiss,
  ) {
    Column(
        modifier = Modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(22.dp),
    ) {
      Text(
          text = "快速重启会结束 MediaProvider 进程并触发系统重新拉起，期间媒体访问可能短暂不可用。是否继续？",
          modifier = Modifier.fillMaxWidth(),
          color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
          fontSize = 15.sp,
          lineHeight = 22.sp,
          fontWeight = FontWeight.Medium,
          textAlign = TextAlign.Center,
      )
      Row(
          modifier = Modifier.fillMaxWidth(),
          horizontalArrangement = Arrangement.spacedBy(14.dp),
      ) {
        GlassTextButton("取消", onDismiss, modifier = Modifier.weight(1f))
        GlassTextButton("确认", onConfirm, modifier = Modifier.weight(1f), primary = true)
      }
    }
  }
}

@Composable
private fun RuntimeActivationDetailsDialog(exactValue: String, onDismiss: () -> Unit) {
  CenteredDialog(show = true, onDismiss = onDismiss) {
    Column(
        modifier = Modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(22.dp),
    ) {
      Text(
          text = "生效次数",
          color = MiuixTheme.colorScheme.onSurface,
          fontSize = 18.sp,
          fontWeight = FontWeight.Bold,
      )
      Text(
          text = exactValue,
          modifier = Modifier.fillMaxWidth(),
          color = MiuixTheme.colorScheme.onSurface,
          fontSize = if (exactValue.length > 16) 17.sp else 22.sp,
          fontWeight = FontWeight.Bold,
          textAlign = TextAlign.Center,
          maxLines = 1,
      )
      GlassTextButton("关闭", onDismiss, modifier = Modifier.fillMaxWidth(), primary = true)
    }
  }
}

@Composable
private fun ResetRuntimeStatsConfirmDialog(onDismiss: () -> Unit, onConfirm: () -> Unit) {
  CenteredDialog(show = true, onDismiss = onDismiss) {
    Column(
        modifier = Modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(22.dp),
    ) {
      Text(
          text = "清除当前生效次数并从 0 重新统计？此操作不会修改应用配置或重定向状态。",
          modifier = Modifier.fillMaxWidth(),
          color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
          fontSize = 15.sp,
          lineHeight = 22.sp,
          fontWeight = FontWeight.Medium,
          textAlign = TextAlign.Center,
      )
      Row(
          modifier = Modifier.fillMaxWidth(),
          horizontalArrangement = Arrangement.spacedBy(14.dp),
      ) {
        GlassTextButton("取消", onDismiss, modifier = Modifier.weight(1f))
        GlassTextButton("清除", onConfirm, modifier = Modifier.weight(1f), primary = true)
      }
    }
  }
}

@Composable
private fun MetricBox(label: String, value: String, modifier: Modifier) {
  Box(
      modifier = modifier.padding(vertical = 8.dp),
      contentAlignment = Alignment.Center,
  ) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
      Text(value, style = MiuixTheme.textStyles.title2, fontWeight = FontWeight.Bold, maxLines = 1)
      Text(label, color = MiuixTheme.colorScheme.onSurfaceVariantSummary, fontSize = 12.sp)
    }
  }
}

@Composable
private fun FeatureChip(label: String, enabled: Boolean, modifier: Modifier) {
  val color = if (enabled) srxSuccessColor() else MiuixTheme.colorScheme.onSurfaceVariantSummary
  Row(
      modifier =
          modifier
              .clip(CircleShape)
              .background(glassSurfaceColor(0.62f), CircleShape)
              .padding(horizontal = 13.dp, vertical = 9.dp),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.Center,
  ) {
    Box(Modifier.size(8.dp).clip(CircleShape).background(color))
    Spacer(Modifier.width(8.dp))
    Text(
        label,
        color = if (enabled) color else MiuixTheme.colorScheme.onSurfaceVariantSummary,
        fontWeight = FontWeight.SemiBold,
        fontSize = 12.sp,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
  }
}

@Composable
private fun ActionRow(title: String, summary: String, icon: ImageVector, onClick: () -> Unit) {
  Row(
      modifier = Modifier.fillMaxWidth().clickable(onClick = onClick).padding(16.dp),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(12.dp),
  ) {
    GlassIcon(icon)
    Column(Modifier.weight(1f)) {
      Text(title, fontWeight = FontWeight.SemiBold)
      Text(summary, color = MiuixTheme.colorScheme.onSurfaceVariantSummary, fontSize = 12.sp)
    }
  }
}

@Composable
private fun GlassIcon(icon: ImageVector, modifier: Modifier = Modifier) {
  val shape = RoundedCornerShape(17.dp)
  val accent = MiuixTheme.colorScheme.primary
  Box(
      modifier =
          modifier
              .size(44.dp)
              .glassPanel(shape, shadowAlpha = 0.05f)
              .clip(shape)
              .background(
                  Brush.linearGradient(
                      listOf(
                          accent.copy(alpha = if (isSrxDarkTheme()) 0.22f else 0.18f),
                          srxSuccessColor().copy(alpha = if (isSrxDarkTheme()) 0.16f else 0.12f),
                          glassSurfaceColor(if (isSrxDarkTheme()) 0.4f else 0.72f),
                      ),
                  ),
              ),
      contentAlignment = Alignment.Center,
  ) {
    Icon(
        icon,
        contentDescription = null,
        tint = MiuixTheme.colorScheme.primary,
        modifier = Modifier.size(24.dp),
    )
  }
}

@Composable
private fun SectionTitle(text: String) {
  Text(
      text,
      modifier = Modifier.padding(start = 2.dp, top = 2.dp, bottom = 4.dp),
      color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
      fontSize = 13.sp,
      fontWeight = FontWeight.Bold,
  )
}
