package org.srx.manager.ui.screen

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.capsuleContainerColor
import org.srx.manager.capsuleSelectedColor
import org.srx.manager.data.UiColorSpec
import org.srx.manager.data.UiColorStyle
import org.srx.manager.data.UiThemeMode
import org.srx.manager.srxPrimaryColor
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

internal val AccentColorOptions =
    listOf(
        0 to "系统取色",
        0xFFF44336.toInt() to "红色",
        0xFFE91E63.toInt() to "粉色",
        0xFF9C27B0.toInt() to "紫色",
        0xFF673AB7.toInt() to "深紫",
        0xFF3F51B5.toInt() to "靛蓝",
        0xFF2196F3.toInt() to "蓝色",
        0xFF00BCD4.toInt() to "青色",
        0xFF009688.toInt() to "水鸭绿",
        0xFF4FAF50.toInt() to "绿色",
        0xFFFFEB3B.toInt() to "黄色",
        0xFFFFC107.toInt() to "琥珀",
        0xFFFF9800.toInt() to "橙色",
        0xFF795548.toInt() to "棕色",
        0xFF607D8F.toInt() to "蓝灰",
        0xFFFF9CA8.toInt() to "樱花",
    )

internal val ColorStyleOptions =
    listOf(
        UiColorStyle.TonalSpot to "柔和",
        UiColorStyle.Neutral to "中性",
        UiColorStyle.Vibrant to "鲜艳",
        UiColorStyle.Expressive to "表现",
        UiColorStyle.Rainbow to "彩虹",
        UiColorStyle.FruitSalad to "果蔬",
        UiColorStyle.Monochrome to "单色",
        UiColorStyle.Fidelity to "保真",
        UiColorStyle.Content to "内容",
    )

internal val ColorSpecOptions =
    listOf(
        UiColorSpec.Spec2025 to "2025",
        UiColorSpec.Spec2021 to "2021",
    )

internal fun accentColorLabel(color: Int): String =
    AccentColorOptions.firstOrNull { it.first == color }?.second ?: "自定义"

internal fun colorStyleLabel(style: UiColorStyle): String =
    ColorStyleOptions.firstOrNull { it.first == style }?.second ?: style.name

internal fun colorSpecLabel(spec: UiColorSpec): String =
    ColorSpecOptions.firstOrNull { it.first == spec }?.second ?: spec.name.removePrefix("Spec")

@Composable
internal fun ThemeModeSelector(
    mode: UiThemeMode,
    onMode: (UiThemeMode) -> Unit,
    modifier: Modifier = Modifier,
) {
  val items =
      listOf(
          UiThemeMode.System to "跟随系统",
          UiThemeMode.Light to "浅色",
          UiThemeMode.Dark to "深色",
      )
  val containerShape = RoundedCornerShape(18.dp)
  val itemShape = RoundedCornerShape(14.dp)
  Row(
      modifier =
          modifier
              .clip(containerShape)
              .background(capsuleContainerColor(), containerShape)
              .padding(4.dp),
      horizontalArrangement = Arrangement.spacedBy(4.dp),
  ) {
    items.forEach { (itemMode, label) ->
      val selected = itemMode == mode
      Box(
          modifier =
              Modifier.weight(1f)
                  .then(
                      if (selected) {
                        Modifier.dropShadow(
                            itemShape,
                            Shadow(
                                radius = 10.dp,
                                color = MiuixTheme.colorScheme.primary,
                                alpha = if (isSrxDarkTheme()) 0.12f else 0.09f,
                            ),
                        )
                      } else {
                        Modifier
                      },
                  )
                  .clip(itemShape)
                  .background(
                      if (selected) capsuleSelectedColor() else Color.Transparent,
                      itemShape,
                  )
                  .clickable { onMode(itemMode) }
                  .padding(vertical = 9.dp),
          contentAlignment = Alignment.Center,
      ) {
        Text(
            text = label,
            color = if (selected) srxPrimaryColor() else MiuixTheme.colorScheme.onSurface,
            fontWeight = if (selected) FontWeight.Black else FontWeight.SemiBold,
            fontSize = 12.sp,
            maxLines = 1,
        )
      }
    }
  }
}

@Composable
internal fun AccentColorPalette(
    selected: Int,
    onSelect: (Int) -> Unit,
    showDivider: Boolean = true,
) {
  Column(modifier = Modifier.fillMaxWidth()) {
    Column(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 13.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
      Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(
            text = "强调色",
            fontSize = 16.sp,
            lineHeight = 20.sp,
            fontWeight = FontWeight.Bold,
        )
        Text(
            text = "选择系统取色，或指定应用主题色",
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            fontSize = 13.sp,
            lineHeight = 18.sp,
        )
      }
      Row(
          modifier = Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
          horizontalArrangement = Arrangement.spacedBy(8.dp),
      ) {
        AccentColorOptions.forEach { (value, label) ->
          AccentColorSwatch(
              color = value,
              selected = selected == value,
              label = label,
              onClick = { onSelect(value) },
          )
        }
      }
    }
    if (showDivider) {
      Box(
          Modifier.fillMaxWidth()
              .padding(start = 16.dp)
              .height(1.dp)
              .background(
                  MiuixTheme.colorScheme.onSurface.copy(
                      alpha = if (isSrxDarkTheme()) 0.035f else 0.045f
                  )
              ),
      )
    }
  }
}

@Composable
private fun AccentColorSwatch(
    color: Int,
    selected: Boolean,
    label: String,
    onClick: () -> Unit,
) {
  val tint = if (color == 0) MiuixTheme.colorScheme.primary else Color(color)
  Box(
      modifier =
          Modifier.size(40.dp)
              .clip(CircleShape)
              .border(
                  width = if (selected) 2.dp else 1.dp,
                  color =
                      if (selected) MiuixTheme.colorScheme.primary
                      else MiuixTheme.colorScheme.onSurface.copy(alpha = 0.08f),
                  shape = CircleShape,
              )
              .clickable(onClick = onClick)
              .padding(5.dp),
      contentAlignment = Alignment.Center,
  ) {
    if (color == 0) {
      Canvas(modifier = Modifier.matchParentSize().clip(CircleShape)) {
        val colors =
            listOf(
                Color(0xFF1677D2),
                Color(0xFF16845B),
                Color(0xFFC23D42),
                Color(0xFF9C6411),
            )
        colors.forEachIndexed { index, sectionColor ->
          drawArc(
              color = sectionColor,
              startAngle = index * 90f - 90f,
              sweepAngle = 90f,
              useCenter = true,
          )
        }
      }
    } else {
      Box(Modifier.matchParentSize().clip(CircleShape).background(tint))
    }
    if (selected) {
      Canvas(modifier = Modifier.size(14.dp)) {
        val markColor = Color.White
        val stroke = 2.dp.toPx()
        drawLine(
            color = markColor,
            start = Offset(size.width * 0.14f, size.height * 0.52f),
            end = Offset(size.width * 0.42f, size.height * 0.78f),
            strokeWidth = stroke,
            cap = StrokeCap.Round,
        )
        drawLine(
            color = markColor,
            start = Offset(size.width * 0.42f, size.height * 0.78f),
            end = Offset(size.width * 0.88f, size.height * 0.2f),
            strokeWidth = stroke,
            cap = StrokeCap.Round,
        )
      }
    }
  }
}

@Composable
internal fun AccentColorPenIcon(color: Int, selected: Boolean = false) {
  val tint = if (color == 0) MiuixTheme.colorScheme.primary else Color(color)
  val shape = CircleShape
  val backgroundAlpha =
      when {
        selected -> if (isSrxDarkTheme()) 0.26f else 0.2f
        isSrxLiquidGlassEnabled() -> 0.16f
        else -> 0.1f
      }
  Box(
      modifier =
          Modifier.size(34.dp)
              .then(
                  if (selected) {
                    Modifier.dropShadow(
                        shape,
                        Shadow(
                            radius = 10.dp,
                            color = tint,
                            alpha = if (isSrxDarkTheme()) 0.18f else 0.12f,
                        ),
                    )
                  } else {
                    Modifier
                  },
              )
              .clip(shape)
              .background(tint.copy(alpha = backgroundAlpha), shape)
              .border(
                  1.dp,
                  tint.copy(alpha = if (selected) 0.34f else if (isSrxDarkTheme()) 0.12f else 0.1f),
                  shape,
              ),
      contentAlignment = Alignment.Center,
  ) {
    Box(
        modifier =
            Modifier.size(17.dp).drawBehind {
              val stroke = 1.6.dp.toPx()
              val tip = Offset(size.width * 0.18f, size.height * 0.84f)
              val lowerShoulder = Offset(size.width * 0.36f, size.height * 0.78f)
              val upperShoulder = Offset(size.width * 0.23f, size.height * 0.64f)
              val bodyEnd = Offset(size.width * 0.78f, size.height * 0.22f)
              val bodyEndLower = Offset(size.width * 0.88f, size.height * 0.32f)
              drawLine(
                  color = tint,
                  start = upperShoulder,
                  end = Offset(size.width * 0.68f, size.height * 0.19f),
                  strokeWidth = stroke,
                  cap = StrokeCap.Round,
              )
              drawLine(
                  color = tint,
                  start = lowerShoulder,
                  end = bodyEndLower,
                  strokeWidth = stroke,
                  cap = StrokeCap.Round,
              )
              drawLine(
                  color = tint,
                  start = upperShoulder,
                  end = lowerShoulder,
                  strokeWidth = stroke,
                  cap = StrokeCap.Round,
              )
              drawLine(
                  color = tint,
                  start = tip,
                  end = lowerShoulder,
                  strokeWidth = stroke,
                  cap = StrokeCap.Round,
              )
              drawLine(
                  color = tint,
                  start = tip,
                  end = upperShoulder,
                  strokeWidth = stroke,
                  cap = StrokeCap.Round,
              )
              drawLine(
                  color = tint,
                  start = Offset(size.width * 0.64f, size.height * 0.18f),
                  end = Offset(size.width * 0.86f, size.height * 0.4f),
                  strokeWidth = stroke,
                  cap = StrokeCap.Round,
              )
              drawLine(
                  color = tint,
                  start = bodyEnd,
                  end = bodyEndLower,
                  strokeWidth = stroke,
                  cap = StrokeCap.Round,
              )
            },
    )
  }
}
