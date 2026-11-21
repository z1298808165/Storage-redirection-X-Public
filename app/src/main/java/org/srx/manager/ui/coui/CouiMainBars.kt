package org.srx.manager.ui.coui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.kyant.backdrop.Backdrop as KyantBackdrop
import io.github.suqi8.coui.kmp.basic.BasicComponent
import io.github.suqi8.coui.kmp.basic.Icon
import io.github.suqi8.coui.kmp.basic.NavigationBar
import io.github.suqi8.coui.kmp.basic.NavigationBarItem
import io.github.suqi8.coui.kmp.basic.Text
import io.github.suqi8.coui.kmp.icon.COUIIcons
import io.github.suqi8.coui.kmp.icon.extended.Close
import io.github.suqi8.coui.kmp.icon.extended.Ok
import io.github.suqi8.coui.kmp.icon.extended.SelectAll
import io.github.suqi8.coui.kmp.icon.extended.Settings
import org.srx.manager.Page
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.data.UiPreferences
import org.srx.manager.ui.component.FloatingBottomBar
import org.srx.manager.ui.component.FloatingBottomBarItem
import org.srx.manager.ui.theme.srxErrorColor
import org.srx.manager.ui.theme.srxPrimaryTextColor
import org.srx.manager.ui.theme.srxSecondaryTextColor
import top.yukonga.miuix.kmp.blur.Backdrop

/** COUI 主题下的底部导航：普通态用 COUI NavigationBar；悬浮态复用悬浮容器但图标与文字改用 COUI 配色。 */
@Composable
internal fun CouiBottomNavigation(
    page: Page,
    // 手势滑动过程中底栏高亮要立刻跟随，不能等 pager 停稳（settledPage 太晚）。
    barSelectedIndex: Int = page.ordinal,
    onPageChange: (Page) -> Unit,
    enabled: Boolean,
    prefs: UiPreferences,
    backdrop: Backdrop,
    kyantBackdrop: KyantBackdrop?,
) {
  val bottomInset = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
  if (prefs.floatingBottomBar) {
    Box(Modifier.fillMaxWidth()) {
      FloatingBottomBar(
          modifier =
              Modifier.align(Alignment.BottomCenter)
                  .clickable(
                      interactionSource = remember { MutableInteractionSource() },
                      indication = null,
                      onClick = {},
                  )
                  .padding(bottom = 12.dp + bottomInset),
          selectedIndex = barSelectedIndex,
          onSelected = { if (enabled) onPageChange(Page.entries[it]) },
          backdrop = backdrop,
          kyantBackdrop = kyantBackdrop,
          tabsCount = Page.entries.size,
          isBlurEnabled = prefs.blurEffect,
          isLiquidGlassEnabled = prefs.liquidGlass,
      ) {
        Page.entries.forEach { item ->
          val selected = item == page
          val tint = if (selected) srxPrimaryTextColor() else srxSecondaryTextColor()
          FloatingBottomBarItem(
              onClick = { if (enabled) onPageChange(item) },
              modifier =
                  Modifier.defaultMinSize(minWidth = 76.dp).alpha(if (enabled) 1f else 0.52f),
          ) {
            Icon(imageVector = item.icon, contentDescription = item.label, tint = tint)
            Text(
                text = item.label,
                fontSize = 11.sp,
                lineHeight = 14.sp,
                color = tint,
                fontWeight = if (selected) FontWeight.Bold else FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Visible,
            )
          }
        }
      }
    }
  } else {
    NavigationBar(modifier = Modifier.fillMaxWidth(), showDivider = false) {
      Page.entries.forEach { item ->
        NavigationBarItem(
            selected = page == item,
            onClick = { if (enabled) onPageChange(item) },
            icon = item.icon,
            label = item.label,
            modifier = Modifier.weight(1f).alpha(if (enabled) 1f else 0.52f),
        )
      }
    }
  }
}

/** COUI 主题下的批量应用模板操作栏。模板选择走 COUI 对话框。 */
@Composable
internal fun CouiAppBatchActionBar(
    selectedCount: Int,
    enabled: Boolean,
    templates: List<ConfigTemplate>,
    onApplyTemplate: (ConfigTemplate) -> Unit,
    onSelectAll: () -> Unit,
    onCancel: () -> Unit,
    prefs: UiPreferences,
    backdrop: Backdrop,
    kyantBackdrop: KyantBackdrop?,
) {
  var showTemplates by remember { mutableStateOf(false) }
  var selectedIndex by remember { mutableIntStateOf(0) }
  val bottomInset = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
  val itemAlpha = if (enabled) 1f else 0.52f

  fun runAction(index: Int, action: () -> Unit) {
    if (!enabled) return
    selectedIndex = index
    action()
  }

  if (prefs.floatingBottomBar) {
    Box(Modifier.fillMaxWidth()) {
      FloatingBottomBar(
          modifier =
              Modifier.align(Alignment.BottomCenter)
                  .clickable(
                      interactionSource = remember { MutableInteractionSource() },
                      indication = null,
                      onClick = {},
                  )
                  .padding(bottom = 12.dp + bottomInset),
          selectedIndex = selectedIndex,
          onSelected = { selectedIndex = it },
          backdrop = backdrop,
          kyantBackdrop = kyantBackdrop,
          tabsCount = 4,
          isBlurEnabled = prefs.blurEffect,
          isLiquidGlassEnabled = prefs.liquidGlass,
          enableDrag = false,
      ) {
        val primary = srxPrimaryTextColor()
        FloatingBottomBarItem(
            onClick = { selectedIndex = 0 },
            modifier = Modifier.defaultMinSize(minWidth = 76.dp).alpha(itemAlpha),
        ) {
          Icon(COUIIcons.Ok, null, tint = primary)
          Text(
              "已选$selectedCount",
              fontSize = 11.sp,
              lineHeight = 14.sp,
              color = primary,
              fontWeight = FontWeight.Bold,
              maxLines = 1,
          )
        }
        FloatingBottomBarItem(
            onClick = { runAction(1) { showTemplates = true } },
            modifier = Modifier.defaultMinSize(minWidth = 76.dp).alpha(itemAlpha),
        ) {
          Icon(COUIIcons.Settings, null, tint = primary)
          Text(
              "模板",
              fontSize = 11.sp,
              lineHeight = 14.sp,
              color = primary,
              fontWeight = FontWeight.SemiBold,
              maxLines = 1,
          )
        }
        FloatingBottomBarItem(
            onClick = { runAction(2, onSelectAll) },
            modifier = Modifier.defaultMinSize(minWidth = 76.dp).alpha(itemAlpha),
        ) {
          Icon(COUIIcons.SelectAll, null, tint = primary)
          Text(
              "全选",
              fontSize = 11.sp,
              lineHeight = 14.sp,
              color = primary,
              fontWeight = FontWeight.SemiBold,
              maxLines = 1,
          )
        }
        FloatingBottomBarItem(
            onClick = { runAction(3, onCancel) },
            modifier = Modifier.defaultMinSize(minWidth = 76.dp).alpha(itemAlpha),
        ) {
          Icon(COUIIcons.Close, null, tint = srxErrorColor())
          Text(
              "取消",
              fontSize = 11.sp,
              lineHeight = 14.sp,
              color = srxErrorColor(),
              fontWeight = FontWeight.SemiBold,
              maxLines = 1,
          )
        }
      }
    }
  } else {
    NavigationBar(modifier = Modifier.fillMaxWidth().alpha(itemAlpha), showDivider = false) {
      NavigationBarItem(
          selected = selectedIndex == 0,
          onClick = { selectedIndex = 0 },
          icon = COUIIcons.Ok,
          label = "已选$selectedCount",
          modifier = Modifier.weight(1f),
      )
      NavigationBarItem(
          selected = selectedIndex == 1,
          onClick = { runAction(1) { showTemplates = true } },
          icon = COUIIcons.Settings,
          label = "模板",
          modifier = Modifier.weight(1f),
      )
      NavigationBarItem(
          selected = selectedIndex == 2,
          onClick = { runAction(2, onSelectAll) },
          icon = COUIIcons.SelectAll,
          label = "全选",
          modifier = Modifier.weight(1f),
      )
      NavigationBarItem(
          selected = selectedIndex == 3,
          onClick = { runAction(3, onCancel) },
          icon = COUIIcons.Close,
          label = "取消",
          modifier = Modifier.weight(1f),
      )
    }
  }
  if (showTemplates) {
    SrxCouiDialog(
        show = true,
        title = "选择配置模板",
        summary = if (templates.isEmpty()) "还没有配置模板" else null,
        onDismissRequest = { showTemplates = false },
    ) {
      androidx.compose.foundation.lazy.LazyColumn(modifier = Modifier.heightIn(max = 380.dp)) {
        templates.forEach { template ->
          item {
            BasicComponent(
                title = template.name,
                summary = "点击应用",
                onClick = {
                  showTemplates = false
                  onApplyTemplate(template)
                },
            )
          }
        }
      }
    }
  }
}
