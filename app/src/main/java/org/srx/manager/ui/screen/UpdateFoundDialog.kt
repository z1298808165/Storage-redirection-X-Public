package org.srx.manager.ui.screen

import android.text.method.LinkMovementMethod
import android.widget.TextView
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import io.noties.markwon.Markwon
import org.srx.manager.CenteredDialog
import org.srx.manager.GlassTextButton
import org.srx.manager.data.ReleaseUpdate
import org.srx.manager.data.UpdateChannel
import org.srx.manager.data.parseReleaseNoteSections
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
internal fun UpdateFoundDialog(
    update: ReleaseUpdate,
    currentVersion: String,
    onDismiss: () -> Unit,
    onOpen: () -> Unit,
) {
  val noteSections = remember(update.releaseNotes) { parseReleaseNoteSections(update.releaseNotes) }
  val notesMaxHeight =
      (LocalConfiguration.current.screenHeightDp.dp * 0.42f).coerceIn(180.dp, 360.dp)
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
          "当前模块版本 ${currentVersion.ifBlank { "--" }}，有新版本可用。",
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
        val versionName = update.versionName.ifBlank { update.tagName }
        if (versionName.isNotBlank()) {
          UpdateMetaBadge(versionName)
        }
      }
      if (noteSections.isNotEmpty()) {
        Box(
            modifier =
                Modifier.fillMaxWidth()
                    .heightIn(max = notesMaxHeight)
                    .verticalScroll(rememberScrollState()),
        ) {
          Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
            noteSections.forEach { section ->
              Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(
                    section.component.title,
                    color = MiuixTheme.colorScheme.primary,
                    fontSize = 13.sp,
                    lineHeight = 17.sp,
                    fontWeight = FontWeight.Black,
                )
                ReleaseMarkdown(section.markdown)
              }
            }
          }
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
private fun ReleaseMarkdown(markdown: String) {
  val context = LocalContext.current
  val markwon = remember(context) { Markwon.create(context) }
  val textColor = MiuixTheme.colorScheme.onSurface.toArgb()
  AndroidView(
      factory = {
        TextView(it).apply {
          setTextColor(textColor)
          textSize = 13f
          setLineSpacing(0f, 1.18f)
          movementMethod = LinkMovementMethod.getInstance()
        }
      },
      update = {
        it.setTextColor(textColor)
        markwon.setMarkdown(it, markdown)
      },
      modifier = Modifier.fillMaxWidth(),
  )
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
