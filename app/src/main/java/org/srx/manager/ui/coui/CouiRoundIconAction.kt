package org.srx.manager.ui.coui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp
import io.github.suqi8.coui.kmp.basic.Icon
import io.github.suqi8.coui.kmp.basic.IconButton
import io.github.suqi8.coui.kmp.effect.LocalCOUIMaterialEffectEnabled
import org.srx.manager.ui.theme.LocalSrxColorScheme
import org.srx.manager.ui.theme.isSrxDarkTheme

/**
 * COUI 顶栏圆形按钮，与 Miuix 的 `RoundIconAction` 完全同规格：视觉 38dp 圆 + 19dp 图标， 外层补到 48dp 无障碍触摸区。此前误用了 COUI
 * `IconButton` 默认的 40dp（CornerRadius 20dp）， 比 Miuix 大一圈。
 *
 * 圆形底仍由 COUI 官方 `IconButton` 的 `materialPanel` 提供（边缘光 / 聚光 / 按压）， 颜色复用 [couiPanelSurface]
 * 与卡片同源，因此材质、取色、开关行为全部与 Miuix 对齐。
 */
@Composable
fun CouiRoundIconAction(
    icon: ImageVector,
    contentDescription: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    danger: Boolean = false,
    enabled: Boolean = true,
) {
  val seed = LocalSrxColorScheme.current
  val dark = isSrxDarkTheme()
  val materialOn = LocalCOUIMaterialEffectEnabled.current
  val tint = if (danger) seed.error else seed.primary
  // 与 Miuix RoundIconAction 一致：内层视觉尺寸 38dp，外层触摸区撑到 48dp。
  Box(
      modifier = modifier.sizeIn(minWidth = 48.dp, minHeight = 48.dp),
      contentAlignment = Alignment.Center,
  ) {
    IconButton(
        onClick = onClick,
        enabled = enabled,
        material = materialOn,
        backgroundColor = couiPanelSurface(),
        cornerRadius = 19.dp,
        minWidth = 38.dp,
        minHeight = 38.dp,
    ) {
      Icon(
          imageVector = icon,
          contentDescription = contentDescription,
          tint = tint,
          modifier = Modifier.size(19.dp),
      )
    }
  }
}
