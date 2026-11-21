package org.srx.manager.ui.coui

import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.suqi8.coui.kmp.basic.Text
import org.srx.manager.ui.theme.srxAccentColor

/**
 * 与 Miuix `SectionTitle` 同规格的 COUI 分组标题：左缩进 6dp、下留白 8dp、13sp 粗体主色。 COUI 的 PreferenceCategory 默认左缩进
 * 32dp，与 16dp 边距的卡片错位，故统一改用本组件。
 */
@Composable
fun CouiSectionTitle(text: String, modifier: Modifier = Modifier) {
  Text(
      text = text,
      modifier = modifier.padding(start = 6.dp, bottom = 8.dp),
      color = srxAccentColor(),
      fontSize = 14.sp,
      fontWeight = FontWeight.Bold,
  )
}
