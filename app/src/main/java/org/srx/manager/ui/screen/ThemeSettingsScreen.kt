package org.srx.manager.ui.screen

import android.annotation.SuppressLint
import android.os.Build
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.roundToInt
import org.srx.manager.CenteredDialog
import org.srx.manager.GlassTextButton
import org.srx.manager.SectionTitle
import org.srx.manager.SettingGroup
import org.srx.manager.SrxProgressiveListPage
import org.srx.manager.appMeshBackground
import org.srx.manager.data.UiColorSpec
import org.srx.manager.data.UiColorStyle
import org.srx.manager.data.UiPreferences
import org.srx.manager.data.UiThemeMode
import org.srx.manager.data.UiThemeStyle
import org.srx.manager.srxRememberListState
import org.srx.manager.subtleFieldLabelColor
import org.srx.manager.ui.theme.LocalSrxColorScheme
import org.srx.manager.ui.theme.SrxRowMetrics
import org.srx.manager.ui.theme.isSrxThemeCoui
import top.yukonga.miuix.kmp.basic.Slider
import top.yukonga.miuix.kmp.basic.SliderDefaults
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.basic.TextFieldDefaults
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.AddWindow
import top.yukonga.miuix.kmp.icon.glass.Back
import top.yukonga.miuix.kmp.icon.glass.ChevronForward
import top.yukonga.miuix.kmp.icon.glass.File
import top.yukonga.miuix.kmp.icon.glass.GridView
import top.yukonga.miuix.kmp.icon.glass.Import
import top.yukonga.miuix.kmp.icon.glass.Info
import top.yukonga.miuix.kmp.icon.glass.ListView
import top.yukonga.miuix.kmp.icon.glass.Refresh
import top.yukonga.miuix.kmp.icon.glass.SettingsOutline
import top.yukonga.miuix.kmp.icon.glass.Tune
import top.yukonga.miuix.kmp.theme.MiuixTheme

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
    onThemeStyle: (UiThemeStyle) -> Unit,
    onPredictiveBack: (Boolean) -> Unit,
    onSwipeBack: (Boolean) -> Unit,
    onPageSwipe: (Boolean) -> Unit,
    onPageScale: (Float) -> Unit,
) {
  var showPageScaleEditor by remember { mutableStateOf(false) }

  // 这里的 LazyColumn 内容 lambda 不是 @Composable，取不了 CompositionLocal，
  // 所以先在可组合作用域里算好，下面只用布尔值。
  val couiStyle = isSrxThemeCoui()
  val listState = srxRememberListState("theme")
  val bottomPadding = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
  SrxProgressiveListPage(
      title = "主题与外观",
      onBack = onBack,
      listState = listState,
      bottomPadding = bottomPadding,
      topSpacing = 18.dp,
      verticalArrangement = Arrangement.spacedBy(SrxRowMetrics.groupSpacing()),
  ) {
    item { ThemePreview(prefs = prefs, modifier = Modifier.fillMaxWidth()) }
    item {
      SectionTitle("主题模式")
      ThemeModeSelector(
          mode = prefs.themeMode,
          onMode = onThemeMode,
          modifier = Modifier.fillMaxWidth(),
      )
    }
    item {
      SectionTitle("主题风格")
      SettingGroup() {
        item {
          SettingMenuRow(
              title = "主题风格",
              leading = { SettingsRowIcon(MiuixIcons.Glass.SettingsOutline) },
              value = themeStyleLabel(prefs.themeStyle),
              options = ThemeStyleSelectOptions,
              selected = prefs.themeStyle,
              onSelect = onThemeStyle,
          )
        }
      }
    }
    // COUI 下整节颜色设置都不显示：动态取色对 COUI 无效，强调色也由 COUI 原生色板接管。
    if (!couiStyle) {
      item {
        SectionTitle("颜色")
        SettingGroup() {
          item {
            CompactSwitchRow(
                title = "动态取色",
                icon = MiuixIcons.Glass.Refresh,
                checked = prefs.dynamicColor,
                onCheckedChange = onDynamicColor,
                showDivider = prefs.dynamicColor,
            )
          }
          if (prefs.dynamicColor) {
            item {
              SettingMenuRow(
                  title = "强调色",
                  leading = { SettingsRowIcon(MiuixIcons.Glass.Tune) },
                  value = accentColorLabel(prefs.accentColor),
                  options = AccentColorOptions,
                  selected = prefs.accentColor,
                  onSelect = onAccentColor,
                  showDivider = prefs.accentColor != 0,
              )
            }
          }
          if (prefs.dynamicColor && prefs.accentColor != 0) {
            item {
              SettingMenuRow(
                  title = "色彩风格",
                  leading = { SettingsRowIcon(MiuixIcons.Glass.GridView) },
                  value = colorStyleLabel(prefs.colorStyle),
                  options = ColorStyleOptions,
                  selected = prefs.colorStyle,
                  onSelect = onColorStyle,
              )
            }
            item {
              SettingMenuRow(
                  title = "色彩标准",
                  leading = { SettingsRowIcon(MiuixIcons.Glass.ListView) },
                  value = colorSpecLabel(prefs.colorSpec),
                  options = ColorSpecOptions,
                  selected = prefs.colorSpec,
                  onSelect = onColorSpec,
              )
            }
          }
        }
      }
    }
    item {
      SectionTitle("显示")
      SettingGroup() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
          item {
            CompactSwitchRow(
                title = "预测性返回手势",
                icon = MiuixIcons.Glass.Back,
                checked = prefs.predictiveBack,
                onCheckedChange = onPredictiveBack,
            )
          }
        }
        item {
          PageScaleControl(
              scale = prefs.pageScale,
              onOpenEditor = { showPageScaleEditor = true },
              onScaleChange = onPageScale,
          )
        }
      }
    }
    item {
      SectionTitle("手势")
      SettingGroup() {
        item {
          CompactSwitchRow(
              title = "横移返回手势",
              icon = MiuixIcons.Glass.ChevronForward,
              checked = prefs.swipeBackGesture,
              onCheckedChange = onSwipeBack,
          )
        }
        item {
          CompactSwitchRow(
              title = "页面切换手势",
              icon = MiuixIcons.Glass.AddWindow,
              checked = prefs.pageSwipeGesture,
              onCheckedChange = onPageSwipe,
              showDivider = false,
          )
        }
      }
    }
    item {
      SectionTitle("视觉效果")
      SettingGroup() {
        item {
          CompactSwitchRow(
              title = "悬浮底栏",
              icon = MiuixIcons.Glass.Import,
              checked = prefs.floatingBottomBar,
              onCheckedChange = onFloating,
          )
        }
        // COUI 主题的底栏与卡片不使用液态玻璃，这个开关在该主题下没有意义，直接隐藏。
        if (!isSrxThemeCoui()) {
          item {
            CompactSwitchRow(
                title = "液态玻璃",
                icon = MiuixIcons.Glass.Info,
                checked = prefs.liquidGlass,
                onCheckedChange = onLiquid,
            )
          }
        }
        item {
          CompactSwitchRow(
              title = "材质模糊",
              icon = MiuixIcons.Glass.File,
              checked = prefs.blurEffect,
              onCheckedChange = onBlurEffect,
              showDivider = false,
          )
        }
      }
    }
  }
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

@SuppressLint("ConfigurationScreenWidthHeight")
@Composable
internal fun ThemePreview(prefs: UiPreferences, modifier: Modifier = Modifier) {
  // 预览卡在两个主题下共用，必须读统一色板：COUI 分支里没有 MiuixTheme，
  // 读 MiuixTheme 会回落到库默认蓝，预览色就和实际主题不一致（用户看到的"预览不对"）。
  val colors = LocalSrxColorScheme.current
  val configuration = LocalConfiguration.current
  val screenRatio =
      configuration.screenWidthDp.toFloat() / configuration.screenHeightDp.coerceAtLeast(1)
  val frameShape = RoundedCornerShape(20.dp)
  val rowShape = RoundedCornerShape(12.dp)
  val glassSurface = colors.surface.copy(alpha = if (prefs.blurEffect) 0.58f else 0.92f)
  val hairline = colors.onSurface.copy(alpha = 0.08f)
  Box(modifier = modifier, contentAlignment = Alignment.Center) {
    Box(
        modifier =
            Modifier.fillMaxWidth(0.4f)
                .widthIn(max = 190.dp)
                .aspectRatio(screenRatio)
                .clip(frameShape)
                .appMeshBackground()
                .border(1.dp, colors.onSurface.copy(alpha = 0.12f), frameShape),
    ) {
      Column(modifier = Modifier.fillMaxSize()) {
        Column(
            modifier =
                Modifier.fillMaxWidth().weight(1f).padding(horizontal = 12.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
          Box(modifier = Modifier.fillMaxWidth().height(24.dp)) {
            Box(
                Modifier.align(Alignment.BottomStart)
                    .width(48.dp)
                    .height(10.dp)
                    .clip(RoundedCornerShape(5.dp))
                    .background(colors.onSurface),
            )
            Box(
                Modifier.align(Alignment.TopEnd)
                    .width(20.dp)
                    .height(5.dp)
                    .clip(RoundedCornerShape(2.dp))
                    .background(colors.onSurface.copy(alpha = 0.34f)),
            )
          }
          Box(
              Modifier.fillMaxWidth()
                  .weight(1.2f)
                  .clip(RoundedCornerShape(16.dp))
                  .background(colors.primary.copy(alpha = 0.88f)),
          )
          Column(
              modifier = Modifier.fillMaxWidth().weight(1f),
              verticalArrangement = Arrangement.spacedBy(7.dp),
          ) {
            repeat(3) {
              Box(
                  Modifier.fillMaxWidth()
                      .weight(1f)
                      .clip(rowShape)
                      .background(glassSurface)
                      .border(1.dp, hairline, rowShape),
              )
            }
          }
        }
        BottomBarMock(prefs = prefs, glassSurface = glassSurface, hairline = hairline)
      }
    }
  }
}

/** 底栏预览：悬浮态是留边距的胶囊，贴底态是全宽导航栏，与 [BottomNavigation] 的两条分支对应。 */
@Composable
private fun BottomBarMock(prefs: UiPreferences, glassSurface: Color, hairline: Color) {
  val colors = MiuixTheme.colorScheme
  val floating = prefs.floatingBottomBar
  val shape =
      if (floating) {
        RoundedCornerShape(16.dp)
      } else {
        RoundedCornerShape(0.dp)
      }
  Box(
      modifier = Modifier.fillMaxWidth().padding(bottom = if (floating) 8.dp else 0.dp),
      contentAlignment = Alignment.Center,
  ) {
    Row(
        modifier =
            (if (floating) Modifier.fillMaxWidth(0.82f) else Modifier.fillMaxWidth())
                .height(if (floating) 34.dp else 32.dp)
                .clip(shape)
                .background(glassSurface)
                .border(1.dp, hairline, shape)
                .padding(horizontal = if (floating) 12.dp else 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement =
            if (floating) Arrangement.spacedBy(14.dp, Alignment.CenterHorizontally)
            else Arrangement.SpaceEvenly,
    ) {
      repeat(4) { index ->
        Box(
            Modifier.size(if (floating) 8.dp else 9.dp)
                .clip(CircleShape)
                .background(
                    if (index == 0) {
                      colors.primary
                    } else {
                      colors.onSurfaceVariantSummary.copy(alpha = 0.42f)
                    }
                ),
        )
      }
    }
  }
}

@Composable
private fun PageScaleControl(
    scale: Float,
    onOpenEditor: () -> Unit,
    onScaleChange: (Float) -> Unit,
) {
  var sliderValue by remember(scale) { mutableFloatStateOf(scale.coerceIn(0.8f, 1.1f)) }
  Column(modifier = Modifier.fillMaxWidth()) {
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
        showKeyPoints = true,
        keyPoints = listOf(0.8f, 0.9f, 1f, 1.1f),
        magnetThreshold = 0.01f,
        hapticEffect = SliderDefaults.SliderHapticEffect.Step,
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

private val ThemeStyleSelectOptions =
    listOf(
        // 与 Eta 一致：只显示风格名，不带括号里的系统名。
        UiThemeStyle.Miuix to "Miuix",
        UiThemeStyle.Coui to "COUI",
    )

private fun themeStyleLabel(style: UiThemeStyle): String =
    ThemeStyleSelectOptions.firstOrNull { it.first == style }?.second ?: "Miuix"

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
  // 统一走 CenteredDialog：底部对齐、离开底边，且带 imePadding —— 输入框聚焦时
  // 整块被输入法顶起（OverlayDialog 是居中弹窗，做不到这个观感）。
  CenteredDialog(
      show = show,
      title = "界面缩放",
      summary = "降低比例可以缓解 DPI 或字体放大后的文本截断；范围 80% - 110%。",
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
