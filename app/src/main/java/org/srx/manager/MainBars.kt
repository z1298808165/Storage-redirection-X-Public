package org.srx.manager

import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
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
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.data.UiPreferences
import org.srx.manager.ui.component.FloatingBottomBar
import org.srx.manager.ui.component.FloatingBottomBarItem
import org.srx.manager.ui.liquid.BlurredBar
import org.srx.manager.ui.screen.TemplatePickerDialog
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.NavigationBar
import top.yukonga.miuix.kmp.basic.NavigationBarItem
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.blur.Backdrop
import top.yukonga.miuix.kmp.blur.LayerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.Close
import top.yukonga.miuix.kmp.icon.glass.Ok
import top.yukonga.miuix.kmp.icon.glass.SelectAll
import top.yukonga.miuix.kmp.icon.glass.Tune
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
internal fun BottomNavigation(
    page: Page,
    // 手势滑动过程中底栏高亮要立刻跟随，不能等 pager 停稳（settledPage 太晚）。
    barSelectedIndex: Int = page.ordinal,
    onPageChange: (Page) -> Unit,
    enabled: Boolean,
    prefs: UiPreferences,
    blurBackdrop: LayerBackdrop?,
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
          FloatingBottomBarItem(
              onClick = { if (enabled) onPageChange(item) },
              modifier =
                  Modifier.defaultMinSize(minWidth = 76.dp).alpha(if (enabled) 1f else 0.52f),
          ) {
            Icon(
                imageVector = item.icon,
                contentDescription = item.label,
                tint = MiuixTheme.colorScheme.onSurface,
            )
            Text(
                text = item.label,
                fontSize = 11.sp,
                lineHeight = 14.sp,
                color = MiuixTheme.colorScheme.onSurface,
                fontWeight = if (selected) FontWeight.Bold else FontWeight.SemiBold,
                maxLines = 1,
                overflow = TextOverflow.Visible,
            )
          }
        }
      }
    }
  } else {
    BlurredBar(backdrop = blurBackdrop, enabled = prefs.blurEffect) {
      NavigationBar(
          modifier = Modifier.fillMaxWidth(),
          color =
              if (blurBackdrop != null && prefs.blurEffect) {
                androidx.compose.ui.graphics.Color.Transparent
              } else {
                MiuixTheme.colorScheme.surface
              },
      ) {
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
}

@Composable
internal fun AppBatchActionBar(
    selectedCount: Int,
    enabled: Boolean,
    templates: List<ConfigTemplate>,
    onApplyTemplate: (ConfigTemplate) -> Unit,
    onSelectAll: () -> Unit,
    onCancel: () -> Unit,
    prefs: UiPreferences,
    blurBackdrop: LayerBackdrop?,
    backdrop: Backdrop,
    dialogBackdrop: Backdrop?,
    kyantBackdrop: KyantBackdrop?,
) {
  var showTemplates by remember { mutableStateOf(false) }
  val bottomInset = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding()
  val itemAlpha = if (enabled) 1f else 0.52f

  fun runAction(action: () -> Unit) {
    if (!enabled) return
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
          selectedIndex = 0,
          onSelected = {},
          showSelectionIndicator = false,
          backdrop = backdrop,
          kyantBackdrop = kyantBackdrop,
          tabsCount = 4,
          isBlurEnabled = prefs.blurEffect,
          isLiquidGlassEnabled = prefs.liquidGlass,
          enableDrag = false,
      ) {
        FloatingBottomBarItem(
            onClick = {},
            modifier = Modifier.defaultMinSize(minWidth = 76.dp).alpha(itemAlpha),
        ) {
          Icon(
              MiuixIcons.Glass.Ok,
              contentDescription = null,
              tint = MiuixTheme.colorScheme.onSurface,
          )
          Text(
              text = "\u5df2\u9009$selectedCount",
              fontSize = 11.sp,
              lineHeight = 14.sp,
              color = MiuixTheme.colorScheme.onSurface,
              fontWeight = FontWeight.Bold,
              maxLines = 1,
              overflow = TextOverflow.Visible,
          )
        }
        FloatingBottomBarItem(
            onClick = { runAction { showTemplates = true } },
            modifier = Modifier.defaultMinSize(minWidth = 76.dp).alpha(itemAlpha),
        ) {
          Icon(
              MiuixIcons.Glass.Tune,
              contentDescription = null,
              tint = MiuixTheme.colorScheme.onSurface,
          )
          Text(
              "\u6a21\u677f",
              fontSize = 11.sp,
              lineHeight = 14.sp,
              color = MiuixTheme.colorScheme.onSurface,
              fontWeight = FontWeight.SemiBold,
              maxLines = 1,
          )
        }
        FloatingBottomBarItem(
            onClick = { runAction(onSelectAll) },
            modifier = Modifier.defaultMinSize(minWidth = 76.dp).alpha(itemAlpha),
        ) {
          Icon(
              MiuixIcons.Glass.SelectAll,
              contentDescription = null,
              tint = MiuixTheme.colorScheme.onSurface,
          )
          Text(
              "\u5168\u9009",
              fontSize = 11.sp,
              lineHeight = 14.sp,
              color = MiuixTheme.colorScheme.onSurface,
              fontWeight = FontWeight.SemiBold,
              maxLines = 1,
          )
        }
        FloatingBottomBarItem(
            onClick = { runAction(onCancel) },
            modifier = Modifier.defaultMinSize(minWidth = 76.dp).alpha(itemAlpha),
        ) {
          Icon(
              MiuixIcons.Glass.Close,
              contentDescription = null,
              tint = MiuixTheme.colorScheme.error,
          )
          Text(
              "\u53d6\u6d88",
              fontSize = 11.sp,
              lineHeight = 14.sp,
              color = MiuixTheme.colorScheme.error,
              fontWeight = FontWeight.SemiBold,
              maxLines = 1,
          )
        }
      }
    }
  } else {
    BlurredBar(backdrop = blurBackdrop, enabled = prefs.blurEffect) {
      NavigationBar(
          modifier = Modifier.fillMaxWidth().alpha(itemAlpha),
          color =
              if (blurBackdrop != null && prefs.blurEffect) {
                androidx.compose.ui.graphics.Color.Transparent
              } else {
                MiuixTheme.colorScheme.surface
              },
      ) {
        NavigationBarItem(
            selected = false,
            onClick = {},
            icon = MiuixIcons.Glass.Ok,
            label = "已选$selectedCount",
            modifier = Modifier.weight(1f),
        )
        NavigationBarItem(
            selected = false,
            onClick = { runAction { showTemplates = true } },
            icon = MiuixIcons.Glass.Tune,
            label = "模板",
            modifier = Modifier.weight(1f),
        )
        NavigationBarItem(
            selected = false,
            onClick = { runAction(onSelectAll) },
            icon = MiuixIcons.Glass.SelectAll,
            label = "全选",
            modifier = Modifier.weight(1f),
        )
        NavigationBarItem(
            selected = false,
            onClick = { runAction(onCancel) },
            icon = MiuixIcons.Glass.Close,
            label = "取消",
            modifier = Modifier.weight(1f),
        )
      }
    }
  }
  CompositionLocalProvider(
      LocalSrxBackdrop provides dialogBackdrop,
      LocalSrxKyantBackdrop provides kyantBackdrop,
  ) {
    TemplatePickerDialog(
        show = showTemplates,
        templates = templates,
        title = "选择配置模板",
        emptyText = "还没有配置模板",
        onDismiss = { showTemplates = false },
        onPick = {
          showTemplates = false
          onApplyTemplate(it)
        },
    )
  }
}
