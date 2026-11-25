package org.srx.manager.ui.coui

import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.suqi8.coui.kmp.basic.BasicComponent
import io.github.suqi8.coui.kmp.basic.BasicComponentColors
import io.github.suqi8.coui.kmp.basic.BasicComponentDefaults
import io.github.suqi8.coui.kmp.basic.CardListPosition
import io.github.suqi8.coui.kmp.basic.DropdownArrowEndAction
import io.github.suqi8.coui.kmp.basic.DropdownDefaults
import io.github.suqi8.coui.kmp.basic.DropdownEntry
import io.github.suqi8.coui.kmp.basic.DropdownItem
import io.github.suqi8.coui.kmp.basic.LocalCardListPosition
import io.github.suqi8.coui.kmp.basic.Text
import io.github.suqi8.coui.kmp.basic.preciseClickAnchor
import io.github.suqi8.coui.kmp.basic.rememberPreciseClickState
import io.github.suqi8.coui.kmp.blur.MaterialBlurDefaults
import io.github.suqi8.coui.kmp.popup.OverlayDropdownPopup
import io.github.suqi8.coui.kmp.theme.COUITheme
import org.srx.manager.ui.theme.LocalSrxColorScheme
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.srxSecondaryTextColor
import org.srx.manager.ui.theme.srxTertiaryTextColor

/**
 * 与 COUI 库 `OverlayDropdownPreference` 同构的下拉行，但**去掉了它的 holdDown 锁灰**： 库实现在展开时把 `holdDownState` 置
 * true，用 `HoldDownObserver` 一直往 interactionSource 注入 HoldDown，灰色 state layer 会一直亮到弹窗 dismiss
 * 动画结束才释放；我们的内缘高光 又画在 state layer 之上，于是出现「选中行发灰、灰色还被高光圈住」的怪异效果。
 *
 * Miuix 的下拉只在按下瞬间有 indication、抬起即消失。这里保留库的全部布局 / 材质 / 弹窗 行为，仅把 `holdDownState` 恒为
 * false，让按压反馈回到系统的瞬时 indication。
 */
@Composable
fun CouiDropdownRow(
    items: List<String>,
    selectedIndex: Int,
    title: String,
    modifier: Modifier = Modifier,
    titleColor: BasicComponentColors = BasicComponentDefaults.titleColor(),
    summary: String? = null,
    summaryColor: BasicComponentColors = BasicComponentDefaults.summaryColor(),
    startAction: @Composable (() -> Unit)? = null,
    bottomAction: (@Composable () -> Unit)? = null,
    insideMargin: PaddingValues = BasicComponentDefaults.InsideMargin,
    cardListPosition: CardListPosition = LocalCardListPosition.current,
    maxHeight: Dp? = null,
    enabled: Boolean = true,
    showValue: Boolean = true,
    renderInRootScaffold: Boolean = true,
    onSelectedIndexChange: ((Int) -> Unit)? = null,
) {
  val interactionSource = remember { MutableInteractionSource() }
  val expanded = remember { mutableStateOf(false) }
  val preciseClickState = rememberPreciseClickState()
  val currentOnSelected = rememberUpdatedState(onSelectedIndexChange)

  val entry =
      remember(items, selectedIndex) {
        DropdownEntry(
            items.mapIndexed { index, item ->
              DropdownItem(
                  text = item,
                  selected = index == selectedIndex,
                  onClick = { currentOnSelected.value?.invoke(index) },
              )
            },
        )
      }
  val itemsNotEmpty = entry.items.isNotEmpty()
  val actualEnabled = enabled && itemsNotEmpty
  val actionColor =
      if (actualEnabled) srxTertiaryTextColor() else COUITheme.colorScheme.disabledNeutral
  val valueColor =
      if (actualEnabled) srxSecondaryTextColor() else COUITheme.colorScheme.disabledNeutral
  // 弹窗面板与卡片/对话框共用同一真值源：背景色取 couiPanelSurface，模糊层
  // 走 panelBlur（其 mix/blend 已由主题色板派生），选中项用主题色而非默认中性色。
  val seed = LocalSrxColorScheme.current
  val dark = isSrxDarkTheme()
  val popupBlur = panelBlur(MaterialBlurDefaults::popup)
  val popupSurface = couiPanelSurface()
  val popupColors =
      DropdownDefaults.dropdownColors(
          selectedIndicatorColor = seed.primary,
          selectedContentColor = seed.primary,
      )

  BasicComponent(
      modifier = modifier.preciseClickAnchor(preciseClickState),
      interactionSource = interactionSource,
      insideMargin = insideMargin,
      cardListPosition = cardListPosition,
      title = title,
      titleColor = titleColor,
      summary = summary,
      summaryColor = summaryColor,
      startAction = startAction,
      endActions = {
        if (showValue && itemsNotEmpty) {
          val text = entry.items.firstOrNull { it.selected }?.text
          if (!text.isNullOrEmpty()) {
            Text(
                text = text,
                modifier =
                    Modifier.padding(end = 4.dp)
                        .align(Alignment.CenterVertically)
                        .weight(1f, fill = false),
                fontSize = COUITheme.textStyles.bodyS.fontSize,
                color = valueColor,
                textAlign = TextAlign.End,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
          }
        }
        DropdownArrowEndAction(actionColor = actionColor)
        if (itemsNotEmpty) {
          OverlayDropdownPopup(
              entry = entry,
              show = expanded.value,
              onDismiss = { expanded.value = false },
              onDismissFinished = {},
              maxHeight = maxHeight,
              dropdownColors = popupColors,
              backgroundColor = popupSurface,
              backgroundModifier = popupBlur?.backgroundModifier ?: androidx.compose.ui.Modifier,
              renderInRootScaffold = renderInRootScaffold,
              collapseOnSelection = true,
              preciseClickState = preciseClickState,
          )
        }
      },
      bottomAction = bottomAction,
      onClick = { if (actualEnabled) expanded.value = !expanded.value },
      role = Role.DropdownList,
      // 关键差异：不锁 holdDown，按压灰只在系统 indication 的按下瞬间出现、抬起即消失。
      holdDownState = false,
      enabled = actualEnabled,
  )
}
