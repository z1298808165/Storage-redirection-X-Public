package org.srx.manager.ui.screen

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.LocalSrxBackdrop
import org.srx.manager.LocalSrxInsideDialog
import org.srx.manager.data.UserProfile
import org.srx.manager.srxGlassButtonSurface
import org.srx.manager.srxPanelSurface
import org.srx.manager.ui.component.LiquidSwitch
import org.srx.manager.ui.theme.SrxRowMetrics
import org.srx.manager.ui.theme.isSrxThemeCoui
import org.srx.manager.ui.theme.srxAccentColor
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

internal val DisabledDefaultProfile = UserProfile(enabled = false)

/** 带图标时的文字起始内缩：16dp 边距 + 30dp 图标 + 14dp 间距。分隔线从这里画起。 */
internal val RowTextInset: Dp = 60.dp

@Composable
internal fun CompactSwitchRow(
    title: String,
    // 副标题改为可选：Eta 风格的列表行只保留图标 + 标题。
    summary: String? = null,
    icon: ImageVector? = null,
    checked: Boolean,
    onCheckedChange: (Boolean) -> Unit,
    showDivider: Boolean = true,
) {
  val couiHaptics = LocalHapticFeedback.current
  if (isSrxThemeCoui()) {
    // 文档规则 4：直接用与 ColorOS 控件一一对应的官方组件，
    // 按压高光、行内边距、分隔线位置、文字样式都由 COUI 负责。
    io.github.suqi8.coui.kmp.preference.SwitchPreference(
        checked = checked,
        onCheckedChange = { next ->
          couiHaptics.performHapticFeedback(HapticFeedbackType.LongPress)
          onCheckedChange(next)
        },
        title = title,
        summary = summary,
        startAction = icon?.let { { SettingsRowIcon(it) } },
    )
    return
  }
  val interactionSource = remember { MutableInteractionSource() }
  val haptics = LocalHapticFeedback.current
  Column(Modifier.fillMaxWidth()) {
    Row(
        modifier =
            Modifier.fillMaxWidth()
                .heightIn(min = SrxRowMetrics.minHeight())
                .clickable(
                    interactionSource = interactionSource,
                    indication = null,
                ) {
                  haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                  onCheckedChange(!checked)
                }
                .padding(horizontal = 16.dp, vertical = SrxRowMetrics.verticalPadding()),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(SrxRowMetrics.itemSpacing()),
    ) {
      if (icon != null) {
        SettingsRowIcon(icon)
      }
      Column(
          modifier = Modifier.weight(1f),
          verticalArrangement = Arrangement.spacedBy(4.dp),
      ) {
        Text(
            text = title,
            fontSize = SrxRowMetrics.titleSize(),
            lineHeight = SrxRowMetrics.titleLineHeight(),
            // COUI 行标题是常规字重，Miuix 是加粗——这是两套文字风格最直接的区别。
            fontWeight = if (isSrxThemeCoui()) FontWeight.Medium else FontWeight.Bold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (summary != null) {
          Text(
              text = summary,
              color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
              fontSize = 13.sp,
              lineHeight = 18.sp,
          )
        }
      }
      LiquidSwitch(
          checked = checked,
          onCheckedChange = onCheckedChange,
          interactionSource = interactionSource,
      )
    }
    if (showDivider) {
      // Eta：分隔线从文字起始处画起，不再压到图标下面；右侧同样留 16dp。
      ConfigDivider(startInset = if (icon != null) RowTextInset else 16.dp, endInset = 16.dp)
    }
  }
}

/** 设置行左侧的图标底座：强调色 12% 圆角块 + 强调色图标，两个主题共用。 */
@Composable
internal fun SettingsRowIcon(
    icon: ImageVector,
    size: Dp? = null,
    radius: Dp? = null,
) {
  val iconBox = size ?: SrxRowMetrics.iconSize()
  if (isSrxThemeCoui()) {
    // Eta 的 COUI 行是裸图标（24dp、强调色、无底座）；Miuix 行才有圆角底座，
    // 这是两套风格最直观的差异之一。
    Box(Modifier.size(iconBox), contentAlignment = Alignment.Center) {
      Icon(
          imageVector = icon,
          contentDescription = null,
          tint = srxAccentColor(),
          modifier = Modifier.size(iconBox),
      )
    }
    return
  }
  Box(
      modifier =
          Modifier.size(iconBox)
              .clip(RoundedCornerShape(radius ?: SrxRowMetrics.iconRadius()))
              .background(srxAccentColor().copy(alpha = 0.12f)),
      contentAlignment = Alignment.Center,
  ) {
    Icon(
        imageVector = icon,
        contentDescription = null,
        tint = srxAccentColor(),
        modifier = Modifier.size(18.dp),
    )
  }
}

/** 与 [SettingsRowIcon] 同形状的方形图标按钮：用于卡片标题右侧的「＋」等动作，避免出现圆形按钮。 */
@Composable
internal fun RowIconAction(
    icon: ImageVector,
    contentDescription: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
  val interaction = remember { MutableInteractionSource() }
  val glassBackdrop = LocalSrxBackdrop.current
  Box(
      modifier =
          modifier
              .size(32.dp)
              .then(
                  if (glassBackdrop != null && !org.srx.manager.LocalSrxInsideDialog.current)
                      Modifier.srxGlassButtonSurface(
                          backdrop = glassBackdrop,
                          shape = RoundedCornerShape(10.dp),
                          // 参考 Eta 侧栏底栏：很淡的底色 + 更宽的模糊，
                          // 按钮靠透明玻璃而不是实心色块存在。
                          fillAlpha =
                              if (org.srx.manager.ui.theme.isSrxDarkTheme()) 0.14f else 0.30f,
                          blurRadius = 16.dp,
                      )
                  else Modifier.srxPanelSurface(shape = RoundedCornerShape(10.dp), elevation = 4.dp)
              )
              .clickable(interactionSource = interaction, indication = null, onClick = onClick),
      contentAlignment = Alignment.Center,
  ) {
    Icon(
        imageVector = icon,
        contentDescription = contentDescription,
        tint = srxAccentColor(),
        modifier = Modifier.size(18.dp),
    )
  }
}
