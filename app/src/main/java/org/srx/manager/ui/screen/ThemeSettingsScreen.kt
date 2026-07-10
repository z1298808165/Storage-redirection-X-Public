package org.srx.manager.ui.screen

import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.roundToInt
import org.srx.manager.BackPageHeader
import org.srx.manager.CenteredDialog
import org.srx.manager.GlassCard
import org.srx.manager.GlassTextButton
import org.srx.manager.SectionTitle
import org.srx.manager.appMeshBackground
import org.srx.manager.data.UiColorSpec
import org.srx.manager.data.UiColorStyle
import org.srx.manager.data.UiPreferences
import org.srx.manager.data.UiThemeMode
import org.srx.manager.subtleFieldLabelColor
import top.yukonga.miuix.kmp.basic.Slider
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.basic.TextFieldDefaults
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.utils.overScrollVertical

private const val PageScaleMinPercent = 80
private const val PageScaleMaxPercent = 110

@Composable
internal fun ThemeSettingsScreen(
    prefs: UiPreferences,
    onBack: () -> Unit,
    onFloating: (Boolean) -> Unit,
    onLiquid: (Boolean) -> Unit,
    onBlurEffect: (Boolean) -> Unit,
    onDynamicColor: (Boolean) -> Unit,
    onAccentColor: (Int) -> Unit,
    onColorStyle: (UiColorStyle) -> Unit,
    onColorSpec: (UiColorSpec) -> Unit,
    onThemeMode: (UiThemeMode) -> Unit,
    onPredictiveBack: (Boolean) -> Unit,
    onPageScale: (Float) -> Unit,
) {
  var showColorStylePicker by remember { mutableStateOf(false) }
  var showColorSpecPicker by remember { mutableStateOf(false) }
  var showPageScaleEditor by remember { mutableStateOf(false) }

  Box(modifier = Modifier.fillMaxSize()) {
    LazyColumn(
        modifier = Modifier.fillMaxSize().overScrollVertical(),
        contentPadding =
            PaddingValues(
                top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 18.dp,
                bottom =
                    WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 28.dp,
                start = 16.dp,
                end = 16.dp,
            ),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
      item { BackPageHeader(title = "主题与外观", onBack = onBack) }
      item { ThemePreview(modifier = Modifier.fillMaxWidth()) }
      item {
        SectionTitle("主题模式")
        ThemeModeSelector(
            mode = prefs.themeMode,
            onMode = onThemeMode,
            modifier = Modifier.fillMaxWidth(),
        )
      }
      item {
        SectionTitle("颜色")
        GlassCard(alpha = 0.58f) {
          CompactSwitchRow(
              title = "动态取色",
              summary = "跟随系统壁纸生成界面配色，关闭后使用固定主题色",
              checked = prefs.dynamicColor,
              onCheckedChange = onDynamicColor,
              showDivider = prefs.dynamicColor,
          )
          if (prefs.dynamicColor) {
            AccentColorPalette(
                selected = prefs.accentColor,
                onSelect = onAccentColor,
                showDivider = prefs.accentColor != 0,
            )
          }
          if (prefs.dynamicColor && prefs.accentColor != 0) {
            SettingSelectRow(
                title = "色彩风格",
                summary = "调整主题色板的明度与饱和度倾向",
                value = colorStyleLabel(prefs.colorStyle),
                onClick = { showColorStylePicker = true },
            )
            SettingSelectRow(
                title = "色彩标准",
                summary = "选择主题色生成算法版本",
                value = colorSpecLabel(prefs.colorSpec),
                onClick = { showColorSpecPicker = true },
                showDivider = false,
            )
          }
        }
      }
      item {
        SectionTitle("显示")
        PageScaleCard(
            scale = prefs.pageScale,
            onOpenEditor = { showPageScaleEditor = true },
            onScaleChange = onPageScale,
        )
      }
      item {
        SectionTitle("视觉效果")
        GlassCard(alpha = 0.58f) {
          val showPredictiveBack = Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE
          CompactSwitchRow(
              title = "悬浮底栏",
              summary = "让底部导航悬浮于页面内容之上",
              checked = prefs.floatingBottomBar,
              onCheckedChange = onFloating,
          )
          CompactSwitchRow(
              title = "液态玻璃",
              summary = "启用玻璃高光、透镜放大与底栏跟随形变",
              checked = prefs.liquidGlass,
              onCheckedChange = onLiquid,
          )
          CompactSwitchRow(
              title = "材质模糊",
              summary = "为浮动面板和弹窗保留背景模糊",
              checked = prefs.blurEffect,
              onCheckedChange = onBlurEffect,
              showDivider = showPredictiveBack,
          )
          if (showPredictiveBack) {
            CompactSwitchRow(
                title = "预测性返回手势",
                summary = "返回二级页面时显示上级页面预览",
                checked = prefs.predictiveBack,
                onCheckedChange = onPredictiveBack,
                showDivider = false,
            )
          }
        }
      }
    }
  }
  SettingOptionDialog(
      show = showColorStylePicker,
      title = "色彩风格",
      options = ColorStyleOptions,
      selected = prefs.colorStyle,
      onDismiss = { showColorStylePicker = false },
      onSelect = {
        onColorStyle(it)
        showColorStylePicker = false
      },
  )
  SettingOptionDialog(
      show = showColorSpecPicker,
      title = "色彩标准",
      options = ColorSpecOptions,
      selected = prefs.colorSpec,
      onDismiss = { showColorSpecPicker = false },
      onSelect = {
        onColorSpec(it)
        showColorSpecPicker = false
      },
  )
  PageScaleDialog(
      show = showPageScaleEditor,
      scale = prefs.pageScale,
      onDismiss = { showPageScaleEditor = false },
      onSave = {
        onPageScale(it)
        showPageScaleEditor = false
      },
  )
}

@Composable
private fun ThemePreview(modifier: Modifier = Modifier) {
  val colors = MiuixTheme.colorScheme
  val frameShape = RoundedCornerShape(28.dp)
  val rowShape = RoundedCornerShape(12.dp)
  Box(modifier = modifier, contentAlignment = Alignment.Center) {
    Box(
        modifier =
            Modifier.fillMaxWidth(0.54f)
                .widthIn(max = 210.dp)
                .aspectRatio(9f / 16f)
                .clip(frameShape)
                .appMeshBackground()
                .border(1.dp, colors.onSurface.copy(alpha = 0.12f), frameShape),
    ) {
      Column(
          modifier =
              Modifier.fillMaxSize()
                  .padding(start = 16.dp, top = 24.dp, end = 16.dp, bottom = 54.dp),
      ) {
        Row(
            modifier = Modifier.fillMaxWidth().height(22.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
          Box(
              Modifier.fillMaxWidth(0.42f)
                  .height(7.dp)
                  .clip(RoundedCornerShape(2.dp))
                  .background(colors.onSurface),
          )
          Box(
              Modifier.size(18.dp)
                  .clip(CircleShape)
                  .background(colors.surfaceContainerHigh)
                  .border(1.dp, colors.onSurface.copy(alpha = 0.1f), CircleShape),
          )
        }
        Spacer(Modifier.height(12.dp))
        Box(
            Modifier.fillMaxWidth()
                .weight(1.2f)
                .clip(RoundedCornerShape(16.dp))
                .background(colors.primary.copy(alpha = 0.88f)),
        )
        Spacer(Modifier.height(12.dp))
        Column(
            modifier = Modifier.fillMaxWidth().weight(1f),
            verticalArrangement = Arrangement.spacedBy(7.dp),
        ) {
          repeat(3) {
            Box(
                Modifier.fillMaxWidth()
                    .weight(1f)
                    .clip(rowShape)
                    .background(colors.surfaceContainer)
                    .border(1.dp, colors.onSurface.copy(alpha = 0.07f), rowShape),
            )
          }
        }
      }
      Row(
          modifier =
              Modifier.align(Alignment.BottomCenter)
                  .fillMaxWidth()
                  .padding(horizontal = 16.dp, vertical = 12.dp)
                  .height(36.dp)
                  .clip(RoundedCornerShape(14.dp))
                  .background(colors.surfaceContainerHigh)
                  .border(
                      1.dp,
                      colors.onSurface.copy(alpha = 0.08f),
                      RoundedCornerShape(14.dp),
                  )
                  .padding(horizontal = 12.dp),
          horizontalArrangement = Arrangement.SpaceBetween,
          verticalAlignment = Alignment.CenterVertically,
      ) {
        repeat(4) { index ->
          Box(
              Modifier.size(12.dp)
                  .clip(RoundedCornerShape(5.dp))
                  .background(
                      if (index == 0) colors.primary
                      else colors.onSurfaceVariantSummary.copy(alpha = 0.42f)
                  ),
          )
        }
      }
    }
  }
}

@Composable
private fun PageScaleCard(
    scale: Float,
    onOpenEditor: () -> Unit,
    onScaleChange: (Float) -> Unit,
) {
  var sliderValue by remember(scale) { mutableFloatStateOf(scale.coerceIn(0.8f, 1.1f)) }
  GlassCard(alpha = 0.58f) {
    Row(
        modifier =
            Modifier.fillMaxWidth()
                .clickable(onClick = onOpenEditor)
                .padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
      Column(
          modifier = Modifier.weight(1f),
          verticalArrangement = Arrangement.spacedBy(4.dp),
      ) {
        Text(
            text = "界面缩放",
            fontSize = 16.sp,
            lineHeight = 20.sp,
            fontWeight = FontWeight.Bold,
        )
        Text(
            text = "调整页面密度，范围 80% - 110%",
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            fontSize = 13.sp,
            lineHeight = 18.sp,
        )
      }
      Text(
          text = pageScalePercentLabel(sliderValue),
          color = MiuixTheme.colorScheme.primary,
          fontSize = 14.sp,
          lineHeight = 18.sp,
          fontWeight = FontWeight.Bold,
      )
    }
    Slider(
        value = sliderValue,
        onValueChange = { sliderValue = it },
        onValueChangeFinished = { onScaleChange(sliderValue) },
        valueRange = 0.8f..1.1f,
        modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 14.dp),
    )
  }
}

internal fun themeModeLabel(mode: UiThemeMode): String =
    when (mode) {
      UiThemeMode.Light -> "浅色"
      UiThemeMode.Dark -> "深色"
      UiThemeMode.System -> "跟随系统"
    }

private fun pageScalePercent(scale: Float): Int =
    (scale.coerceIn(PageScaleMinPercent / 100f, PageScaleMaxPercent / 100f) * 100).roundToInt()

private fun pageScalePercentLabel(scale: Float): String = "${pageScalePercent(scale)}%"

@Composable
private fun PageScaleDialog(
    show: Boolean,
    scale: Float,
    onDismiss: () -> Unit,
    onSave: (Float) -> Unit,
) {
  var input by remember(show, scale) { mutableStateOf(pageScalePercent(scale).toString()) }
  val parsed = input.toIntOrNull()
  val clamped = parsed?.coerceIn(PageScaleMinPercent, PageScaleMaxPercent)
  val isInvalid = input.isNotBlank() && parsed == null
  CenteredDialog(
      title = "界面缩放",
      summary = "降低比例可以缓解 DPI 或字体放大后的文本截断；范围 80% - 110%。",
      show = show,
      onDismiss = onDismiss,
  ) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
      TextField(
          value = input,
          onValueChange = { value -> input = value.filter(Char::isDigit).take(3) },
          label = "缩放百分比",
          colors = TextFieldDefaults.textFieldColors(labelColor = subtleFieldLabelColor()),
          useLabelAsPlaceholder = true,
          singleLine = true,
          modifier = Modifier.fillMaxWidth(),
      )
      Text(
          text =
              when {
                isInvalid || parsed == null -> "请输入 80 - 110"
                clamped != parsed -> "将保存为 ${clamped}%"
                else -> "当前为 ${clamped}%"
              },
          color =
              if (isInvalid || parsed == null) {
                MiuixTheme.colorScheme.error
              } else {
                MiuixTheme.colorScheme.onSurfaceVariantSummary
              },
          fontSize = 12.sp,
          lineHeight = 16.sp,
      )
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        GlassTextButton("取消", onDismiss, modifier = Modifier.weight(1f))
        GlassTextButton(
            "保存",
            {
              input.toIntOrNull()?.coerceIn(PageScaleMinPercent, PageScaleMaxPercent)?.let {
                onSave(it / 100f)
              }
            },
            modifier = Modifier.weight(1f),
            primary = true,
        )
      }
    }
  }
}
