package org.srx.manager.ui.screen

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.CenteredDialog
import org.srx.manager.GlassTextButton
import org.srx.manager.data.ReleaseUpdate
import org.srx.manager.data.UpdateChannel
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
internal fun UpdateFoundDialog(
    update: ReleaseUpdate,
    currentVersion: String,
    onDismiss: () -> Unit,
    onOpen: () -> Unit,
) {
  CenteredDialog(show = true, onDismiss = onDismiss) {
    Column(
        modifier = Modifier.fillMaxWidth(),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
      Text(
          "发现新版本",
          modifier = Modifier.fillMaxWidth(),
          fontWeight = FontWeight.Black,
          fontSize = 17.sp,
          lineHeight = 21.sp,
      )
      Text(
          "当前模块版本 ${currentVersion.ifBlank { "--" }}，发现 ${update.title.ifBlank { update.tagName.ifBlank { "新版本" } }}。",
          modifier = Modifier.fillMaxWidth(),
          color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
          fontSize = 14.sp,
          lineHeight = 23.sp,
          textAlign = TextAlign.Center,
      )
      FlowRow(
          modifier = Modifier.fillMaxWidth(),
          horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
          verticalArrangement = Arrangement.spacedBy(8.dp),
      ) {
        UpdateMetaBadge(updateChannelBadge(update))
        if (update.tagName.isNotBlank()) {
          UpdateMetaBadge(update.tagName)
        }
      }
      Row(
          modifier = Modifier.fillMaxWidth().padding(top = 4.dp),
          horizontalArrangement = Arrangement.spacedBy(12.dp),
      ) {
        GlassTextButton("取消", onDismiss, modifier = Modifier.weight(1f))
        GlassTextButton("打开", onOpen, modifier = Modifier.weight(1f), primary = true)
      }
    }
  }
}

@Composable
private fun UpdateMetaBadge(text: String, modifier: Modifier = Modifier) {
  Text(
      text,
      modifier =
          modifier
              .clip(CircleShape)
              .background(MiuixTheme.colorScheme.primary.copy(alpha = 0.12f), CircleShape)
              .padding(horizontal = 10.dp, vertical = 6.dp),
      color = MiuixTheme.colorScheme.primary,
      fontSize = 11.sp,
      lineHeight = 13.sp,
      fontWeight = FontWeight.Black,
      maxLines = 1,
      overflow = TextOverflow.Ellipsis,
  )
}

private fun updateChannelBadge(update: ReleaseUpdate): String =
    if (update.channel == UpdateChannel.Beta || update.prerelease) "测试版" else "正式版"
