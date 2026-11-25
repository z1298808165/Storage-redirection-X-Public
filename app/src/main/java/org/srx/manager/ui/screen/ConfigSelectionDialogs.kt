package org.srx.manager.ui.screen

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupProperties
import io.github.suqi8.coui.kmp.basic.DropdownImpl
import io.github.suqi8.coui.kmp.basic.DropdownItem
import io.github.suqi8.coui.kmp.basic.ListPopupColumn
import io.github.suqi8.coui.kmp.basic.PopupPositionProvider
import io.github.suqi8.coui.kmp.overlay.OverlayListPopup
import kotlin.math.roundToInt
import org.srx.manager.CenteredDialog
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.glassSurfaceColor
import org.srx.manager.srxPrimaryColor
import org.srx.manager.ui.theme.SrxShapes
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import org.srx.manager.ui.theme.isSrxThemeCoui
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.ChevronForward
import top.yukonga.miuix.kmp.icon.glass.File
import top.yukonga.miuix.kmp.icon.glass.Ok
import top.yukonga.miuix.kmp.icon.glass.Tune
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
internal fun TemplatePickerDialog(
    show: Boolean,
    templates: List<ConfigTemplate>,
    title: String,
    emptyText: String,
    onDismiss: () -> Unit,
    onPick: (ConfigTemplate) -> Unit,
) {
  // 统一接入 CenteredDialog：与 COUI 一致地''贴底对齐但离开底边''。
  // OverlayBottomSheet 是 edge-to-edge 贴死屏底，官方参数无法留白。
  CenteredDialog(show = show, title = title, onDismiss = onDismiss) {
    if (templates.isEmpty()) {
      Text(
          emptyText,
          modifier = Modifier.fillMaxWidth(),
          color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
          fontSize = 13.sp,
          lineHeight = 18.sp,
          textAlign = TextAlign.Center,
      )
    } else {
      LazyColumn(
          modifier = Modifier.fillMaxWidth().heightIn(max = 420.dp),
          verticalArrangement = Arrangement.spacedBy(8.dp),
      ) {
        items(templates, key = { it.id }) { template ->
          TemplatePickRow(template, onClick = { onPick(template) })
        }
      }
    }
  }
}

@Composable
internal fun AutoTemplatePickerDialog(
    show: Boolean,
    templates: List<ConfigTemplate>,
    currentTemplateId: String,
    onDismiss: () -> Unit,
    onPick: (String) -> Unit,
) {
  val currentTemplateExists =
      remember(templates, currentTemplateId) {
        currentTemplateId.isNotBlank() && templates.any { it.id == currentTemplateId }
      }
  val listMaxHeight =
      (LocalConfiguration.current.screenHeightDp.dp * 0.48f).coerceIn(220.dp, 340.dp)
  CenteredDialog(show = show, title = "新应用默认配置", onDismiss = onDismiss) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
      Text(
          text = "选择一个模板后，新安装应用会自动使用该模板生成配置。",
          color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
          fontSize = 13.sp,
          lineHeight = 19.sp,
      )
      LazyColumn(
          modifier = Modifier.fillMaxWidth().heightIn(max = listMaxHeight),
          verticalArrangement = Arrangement.spacedBy(8.dp),
      ) {
        item {
          AutoTemplateDefaultRow(
              selected = currentTemplateId.isBlank() || !currentTemplateExists,
              onClick = { onPick("") },
          )
        }
        items(templates, key = { it.id }) { template ->
          TemplatePickRow(
              template = template,
              selected = currentTemplateExists && template.id == currentTemplateId,
              onClick = { onPick(template.id) },
          )
        }
      }
    }
  }
}

@Composable
internal fun AutoRedirectTemplateStatusRow(
    template: ConfigTemplate?,
    templateId: String,
    fallbackNoticeId: String,
    onClick: () -> Unit,
) {
  val missingTemplate =
      (templateId.isNotBlank() && template == null) || fallbackNoticeId.isNotBlank()
  val accentColor =
      if (missingTemplate) MiuixTheme.colorScheme.error else MiuixTheme.colorScheme.primary
  val summary =
      when {
        template != null -> "使用模板：${template.name}"
        missingTemplate -> "模板已失效，已回退为仅开启重定向"
        else -> "仅开启重定向，无其它规则配置"
      }
  Column(Modifier.fillMaxWidth()) {
    Row(
        modifier =
            Modifier.fillMaxWidth()
                .clickable(onClick = onClick)
                .padding(start = 24.dp, end = 16.dp)
                .padding(vertical = 9.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
      // 二级行用同一套圆角底座，但小一号（22dp/7dp）并整体右缩进，
      // 既与一级行同一语言，又能一眼看出层级；之前的 34dp 圆形既不一致又偏大。
      SettingsRowIcon(MiuixIcons.Glass.Tune, size = 22.dp, radius = 7.dp)
      Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
        Text(
            "自动配置模板",
            fontSize = 14.sp,
            lineHeight = 18.sp,
            fontWeight = FontWeight.Bold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            summary,
            color =
                if (missingTemplate) accentColor
                else MiuixTheme.colorScheme.onSurfaceVariantSummary,
            fontSize = 12.sp,
            lineHeight = 16.sp,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
      }
      AutoTemplateChevron(
          tint = if (missingTemplate) accentColor else MiuixTheme.colorScheme.onSurface,
      )
    }
    ConfigDivider()
  }
}

@Composable
internal fun SettingSelectRow(
    title: String,
    // 与 CompactSwitchRow 一致：副标题可选，Eta 风格只留标题。
    summary: String? = null,
    value: String,
    onClick: () -> Unit,
    showDivider: Boolean = true,
    leading: @Composable (() -> Unit)? = null,
    // 需要"点哪弹哪"时，用落点回调替代普通点击：子组件的 clickable 会吃掉点击，
    // 所以落点必须在行内部采集。
    onClickPosition: ((androidx.compose.ui.geometry.Offset) -> Unit)? = null,
) {
  if (isSrxThemeCoui()) {
    // 文档规则 4：行本身交给官方 ArrowPreference；值文本走 endActions。
    io.github.suqi8.coui.kmp.preference.ArrowPreference(
        title = title,
        summary = summary,
        startAction = leading,
        endActions = {
          Text(
              text = value,
              color = org.srx.manager.ui.theme.LocalSrxColorScheme.current.onSurfaceVariantSummary,
              fontSize = 13.sp,
              lineHeight = 17.sp,
              fontWeight = FontWeight.Bold,
              maxLines = 1,
              overflow = TextOverflow.Ellipsis,
          )
        },
        onClick = onClick,
    )
    return
  }
  Column(Modifier.fillMaxWidth()) {
    Row(
        modifier =
            Modifier.fillMaxWidth()
                .heightIn(min = 56.dp)
                .then(
                    if (onClickPosition != null) {
                      Modifier.pointerInput(Unit) {
                        detectTapGestures { offset -> onClickPosition(offset) }
                      }
                    } else {
                      Modifier.clickable(onClick = onClick)
                    }
                )
                .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
      leading?.invoke()
      Column(
          modifier = Modifier.weight(1f),
          verticalArrangement = Arrangement.spacedBy(4.dp),
      ) {
        Text(
            text = title,
            fontSize = 16.sp,
            lineHeight = 20.sp,
            fontWeight = FontWeight.Bold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (summary != null) {
          Text(
              text = summary,
              color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
              fontSize = 13.sp,
              lineHeight = 18.sp,
              maxLines = 2,
              overflow = TextOverflow.Ellipsis,
          )
        }
      }
      Text(
          text = value,
          // 值文本用次级文字色：之前取 MiuixTheme.colorScheme.primary，而 COUI 分支
          // 没有 MiuixTheme 外壳，拿到的是库默认蓝色 —— 这就是"系统取色/100% 还是蓝的"。
          color = org.srx.manager.ui.theme.LocalSrxColorScheme.current.onSurfaceVariantSummary,
          fontSize = 13.sp,
          lineHeight = 17.sp,
          fontWeight = FontWeight.Bold,
          maxLines = 1,
          overflow = TextOverflow.Ellipsis,
          textAlign = TextAlign.End,
          modifier = Modifier.widthIn(max = 108.dp),
      )
      AutoTemplateChevron(tint = MiuixTheme.colorScheme.onSurface)
    }
    if (showDivider) {
      ConfigDivider(startInset = if (leading != null) RowTextInset else 16.dp, endInset = 16.dp)
    }
  }
}

/**
 * 与 [SettingSelectRow] 同外观，点击后在**点击位置**下方弹出二级菜单。
 *
 * 材质与动画都交给各自主题自己的实现：
 * - COUI：走 COUI 原生 OverlayListPopup + ListPopupColumn + DropdownImpl， 与 Eta 的 CouiMenu
 *   一致，带库自带的弹出动画与材质；
 * - Miuix：用 Popup + scale/alpha 入场动画 + 中性表面 + 描边。
 */
@Composable
internal fun <T> SettingMenuRow(
    title: String,
    value: String,
    options: List<Pair<T, String>>,
    selected: T,
    onSelect: (T) -> Unit,
    showDivider: Boolean = true,
    leading: @Composable (() -> Unit)? = null,
) {
  var expanded by remember { mutableStateOf(false) }
  var tapX by remember { mutableFloatStateOf(0f) }
  var rowHeightPx by remember { mutableIntStateOf(0) }
  val bounds = remember { mutableStateOf<androidx.compose.ui.unit.IntRect?>(null) }
  val coui = isSrxThemeCoui()
  val anchorPos = remember(bounds) { CouiMenuAnchor(bounds) }

  Box(
      Modifier.onGloballyPositioned {
        rowHeightPx = it.size.height
        val b = it.boundsInWindow()
        bounds.value =
            androidx.compose.ui.unit.IntRect(
                b.left.roundToInt(),
                b.top.roundToInt(),
                b.right.roundToInt(),
                b.bottom.roundToInt(),
            )
      }
  ) {
    SettingSelectRow(
        title = title,
        value = value,
        // 兜底：COUI 的官方行拿不到落点，靠这个回调打开菜单。
        onClick = { expanded = true },
        showDivider = showDivider,
        leading = leading,
        onClickPosition = { offset ->
          tapX = offset.x
          expanded = true
        },
    )

    SrxAnchoredOptionMenu(
        expanded = expanded,
        onDismiss = { expanded = false },
        options = options,
        selected = selected,
        onSelect = onSelect,
        bounds = bounds.value,
        anchorHeightPx = rowHeightPx,
        popupOffsetX = tapX.roundToInt().coerceAtLeast(0),
    )
  }
}

/** COUI 下拉位置：以行的窗口坐标为锚，交给库的 DropdownPositionProvider。 */
private class CouiMenuAnchor(
    private val boundsState:
        androidx.compose.runtime.MutableState<androidx.compose.ui.unit.IntRect?>,
) : PopupPositionProvider {
  override fun calculatePosition(
      anchorBounds: androidx.compose.ui.unit.IntRect,
      windowBounds: androidx.compose.ui.unit.IntRect,
      layoutDirection: androidx.compose.ui.unit.LayoutDirection,
      popupContentSize: androidx.compose.ui.unit.IntSize,
      popupMargin: androidx.compose.ui.unit.IntRect,
      alignment: PopupPositionProvider.Align,
  ): IntOffset =
      io.github.suqi8.coui.kmp.basic.ListPopupDefaults.DropdownPositionProvider.calculatePosition(
          boundsState.value ?: anchorBounds,
          windowBounds,
          layoutDirection,
          popupContentSize,
          popupMargin,
          alignment,
      )

  override fun getMargins(): androidx.compose.foundation.layout.PaddingValues =
      io.github.suqi8.coui.kmp.basic.ListPopupDefaults.DropdownPositionProvider.getMargins()
}

@Composable
private fun TemplatePickRow(
    template: ConfigTemplate,
    onClick: () -> Unit,
    selected: Boolean = false,
) {
  SelectableDialogRow(
      selected = selected,
      title = template.name,
      subtitle = "${template.config.users.size} 个用户配置",
      onClick = onClick,
      leading = {
        SelectableDialogIcon(
            icon = MiuixIcons.Glass.File,
            selected = selected,
            shape = RoundedCornerShape(SrxShapes.inner),
        )
      },
  )
}

@Composable
private fun AutoTemplateDefaultRow(selected: Boolean, onClick: () -> Unit) {
  SelectableDialogRow(
      selected = selected,
      title = "仅开启重定向",
      subtitle = "不附加允许路径、沙盒路径或映射规则",
      onClick = onClick,
      leading = {
        SelectableDialogIcon(
            icon = MiuixIcons.Glass.Ok,
            selected = selected,
            shape = CircleShape,
        )
      },
  )
}

@Composable
private fun SelectableDialogRow(
    selected: Boolean,
    title: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    leading: @Composable (() -> Unit)? = null,
) {
  val primary = MiuixTheme.colorScheme.primary
  val dark = isSrxDarkTheme()
  val liquid = isSrxLiquidGlassEnabled()
  val shape = RoundedCornerShape(SrxShapes.inner)
  val interactionSource = remember { MutableInteractionSource() }
  val pressed by interactionSource.collectIsPressedAsState()
  val scale by
      animateFloatAsState(
          targetValue = if (pressed) 0.972f else 1f,
          animationSpec = spring(dampingRatio = 0.72f, stiffness = 620f),
          label = "dialog-option-press",
      )
  val container =
      if (selected) {
        primary.copy(alpha = if (dark) 0.16f else 0.09f)
      } else {
        if (liquid) {
          MiuixTheme.colorScheme.surfaceContainerHigh.copy(alpha = if (dark) 0.78f else 0.72f)
        } else {
          glassSurfaceColor(0.9f)
        }
      }
  val borderColor =
      if (selected) {
        primary.copy(alpha = if (dark) 0.42f else 0.3f)
      } else {
        MiuixTheme.colorScheme.onSurface.copy(alpha = if (dark) 0.085f else 0.06f)
      }
  Row(
      modifier =
          modifier
              .fillMaxWidth()
              .graphicsLayer {
                scaleX = scale
                scaleY = scale
              }
              .then(
                  if (selected) {
                    Modifier.dropShadow(
                        shape,
                        Shadow(
                            radius = 14.dp,
                            color = primary,
                            alpha = if (dark) 0.12f else 0.08f,
                        ),
                    )
                  } else {
                    Modifier
                  },
              )
              .clip(shape)
              .background(container, shape)
              .border(1.dp, borderColor, shape)
              .clickable(
                  interactionSource = interactionSource,
                  indication = null,
                  onClick = onClick,
              )
              .padding(horizontal = 12.dp, vertical = if (subtitle == null) 11.dp else 12.dp),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(11.dp),
  ) {
    leading?.invoke()
    Column(
        modifier = Modifier.weight(1f),
        verticalArrangement = Arrangement.spacedBy(if (subtitle == null) 0.dp else 2.dp),
    ) {
      Text(
          title,
          color = if (selected) primary else MiuixTheme.colorScheme.onSurface,
          fontWeight = if (selected) FontWeight.Black else FontWeight.Bold,
          fontSize = 14.sp,
          lineHeight = 18.sp,
          maxLines = 1,
          overflow = TextOverflow.Ellipsis,
      )
      if (subtitle != null) {
        Text(
            subtitle,
            color =
                if (selected) {
                  primary.copy(alpha = if (dark) 0.82f else 0.78f)
                } else {
                  MiuixTheme.colorScheme.onSurfaceVariantSummary
                },
            fontSize = 11.sp,
            lineHeight = 15.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
      }
    }
    SelectionCheckMark(selected)
  }
}

@Composable
private fun SelectableDialogIcon(
    icon: ImageVector,
    selected: Boolean,
    shape: Shape,
    tint: Color = MiuixTheme.colorScheme.primary,
) {
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
                            alpha = if (isSrxDarkTheme()) 0.14f else 0.1f,
                        ),
                    )
                  } else {
                    Modifier
                  },
              )
              .clip(shape)
              .background(
                  tint.copy(
                      alpha = if (selected) 0.2f else if (isSrxLiquidGlassEnabled()) 0.14f else 0.1f
                  ),
                  shape,
              ),
      contentAlignment = Alignment.Center,
  ) {
    Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(18.dp))
  }
}

@Composable
private fun SelectionCheckMark(selected: Boolean) {
  Box(
      modifier = Modifier.size(24.dp),
      contentAlignment = Alignment.Center,
  ) {
    if (selected) {
      Box(
          modifier =
              Modifier.size(23.dp)
                  .dropShadow(
                      CircleShape,
                      Shadow(
                          radius = 9.dp,
                          color = MiuixTheme.colorScheme.primary,
                          alpha = if (isSrxDarkTheme()) 0.2f else 0.16f,
                      ),
                  )
                  .clip(CircleShape)
                  .background(MiuixTheme.colorScheme.primary, CircleShape),
          contentAlignment = Alignment.Center,
      ) {
        Icon(
            MiuixIcons.Glass.Ok,
            contentDescription = null,
            tint = Color.White,
            modifier = Modifier.size(14.dp),
        )
      }
    }
  }
}

@Composable
private fun AutoTemplateChevron(tint: Color) {
  Box(
      modifier =
          Modifier.size(24.dp)
              .clip(CircleShape)
              .background(
                  if (isSrxLiquidGlassEnabled()) glassSurfaceColor(0.5f) else Color.Transparent,
                  CircleShape,
              ),
      contentAlignment = Alignment.Center,
  ) {
    Icon(
        imageVector = MiuixIcons.Glass.ChevronForward,
        contentDescription = null,
        tint = tint,
        modifier = Modifier.size(18.dp),
    )
  }
}

/** 通用「锚定选项菜单」：点触发器后在它下方弹出，COUI 用官方 OverlayListPopup， Miuix 用自绘 Popup。用户切换和主题风格共用，避免两套定位与动效实现。 */
@Composable
internal fun <T> SrxAnchoredOptionMenu(
    expanded: Boolean,
    onDismiss: () -> Unit,
    options: List<Pair<T, String>>,
    selected: T,
    onSelect: (T) -> Unit,
    bounds: androidx.compose.ui.unit.IntRect?,
    anchorHeightPx: Int,
    popupOffsetX: Int = 0,
) {
  val coui = isSrxThemeCoui()
  // COUI 的 OverlayListPopup 自带与触发点之间的间距，Miuix 自绘没有，补 6dp。
  val gapPx = with(androidx.compose.ui.platform.LocalDensity.current) { 6.dp.roundToPx() }
  val boundsState = remember(bounds) { mutableStateOf(bounds) }
  val anchorPos = remember(bounds) { CouiMenuAnchor(boundsState) }
  if (coui) {
    // COUI：交还给它自己的 Popup，动画与材质由库保证。
    OverlayListPopup(
        show = expanded,
        onDismissRequest = onDismiss,
        popupPositionProvider = anchorPos,
    ) {
      ListPopupColumn {
        options.forEach { (option, label) ->
          DropdownImpl(
              item =
                  DropdownItem(
                      text = label,
                      selected = option == selected,
                      enabled = true,
                  ),
              optionSize = options.size,
              index = options.indexOfFirst { it.first == option },
              isSelected = option == selected,
              reserveIconSlot = false,
              onSelectedIndexChange = {
                onDismiss()
                if (option != selected) onSelect(option)
              },
          )
        }
      }
    }
  } else if (expanded) {
    // Miuix 侧自绘菜单：当前依赖版本（0.9.5-beta02）还没有官方的
    // WindowDropdownPopup，故按 Miuix 的视觉与动效自行实现——
    // 中性 surface + 1dp 描边 + 阴影，入场 缩放(0.92→1)+淡入，
    // 曲线用 spring(0.82, 620) 接近 Miuix 的弹出节奏。
    Popup(
        alignment = Alignment.TopStart,
        offset = IntOffset(popupOffsetX, anchorHeightPx + gapPx),
        onDismissRequest = onDismiss,
        properties = PopupProperties(focusable = true),
    ) {
      var shown by remember { mutableStateOf(false) }
      val progress by
          animateFloatAsState(
              targetValue = if (shown) 1f else 0f,
              animationSpec = spring(dampingRatio = 0.82f, stiffness = 620f),
              label = "srxMenuAppear",
          )
      LaunchedEffect(Unit) { shown = true }
      val shape = RoundedCornerShape(SrxShapes.inner)
      Column(
          modifier =
              Modifier.width(180.dp)
                  .graphicsLayer {
                    val p = progress.coerceIn(0f, 1f)
                    scaleX = 0.92f + 0.08f * p
                    scaleY = 0.92f + 0.08f * p
                    alpha = p
                    transformOrigin = androidx.compose.ui.graphics.TransformOrigin(0f, 0f)
                  }
                  .dropShadow(shape, Shadow(radius = 12.dp, color = Color.Black, alpha = 0.16f))
                  .clip(shape)
                  // 深色下 surface 与页面底几乎同色，菜单会糊在一起；
                  // 提到 surfaceContainerHigh 这一档，再加重描边和投影，层次才出来。
                  .background(org.srx.manager.srxElevatedSurfaceColor())
                  .border(
                      1.dp,
                      MiuixTheme.colorScheme.outline.copy(
                          alpha = if (org.srx.manager.ui.theme.isSrxDarkTheme()) 0.30f else 0.18f
                      ),
                      shape,
                  )
                  .padding(vertical = 6.dp),
      ) {
        options.forEach { (option, label) ->
          val isSelected = option == selected
          Row(
              modifier =
                  Modifier.fillMaxWidth()
                      .clickable {
                        onDismiss()
                        if (!isSelected) onSelect(option)
                      }
                      .padding(horizontal = 14.dp, vertical = 10.dp),
              verticalAlignment = Alignment.CenterVertically,
              horizontalArrangement = Arrangement.spacedBy(8.dp),
          ) {
            Text(
                text = label,
                modifier = Modifier.weight(1f),
                color =
                    if (isSelected) srxPrimaryColor()
                    else org.srx.manager.ui.theme.LocalSrxColorScheme.current.onSurface,
                fontSize = 14.sp,
                lineHeight = 18.sp,
                fontWeight = if (isSelected) FontWeight.Bold else FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (isSelected) {
              Text(
                  text = "\u2713",
                  color = srxPrimaryColor(),
                  fontSize = 14.sp,
                  fontWeight = FontWeight.Bold,
              )
            }
          }
        }
      }
    }
  }
}
