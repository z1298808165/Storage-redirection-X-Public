package org.srx.manager.ui.coui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import io.github.suqi8.coui.kmp.blur.MaterialBlurDefaults
import io.github.suqi8.coui.kmp.effect.LocalCOUIMaterialEffectEnabled
import io.github.suqi8.coui.kmp.overlay.OverlayDialog
import org.srx.manager.ui.theme.LocalSrxColorScheme
import org.srx.manager.ui.theme.isSrxDarkTheme

/**
 * COUI 弹窗统一封装，对齐 Eta 的 COUI 17 对话框观感：
 * - 边缘高光 + 内阴影（`materialStroke`），与页面材质开关联动；
 * - 面板背景模糊（`panelBlur(MaterialBlurDefaults::dialog)`），不可用时回落不透明表面；
 * - 内容区统一水平内边距（默认 24dp）。COUI 的 `OverlayDialog` content slot 本身不带内边距， 直接在调用点写内容会贴边；Eta 用
 *   `CouiDialogDefaults.contentPadding` 统一收口。
 *
 * @param contentHorizontalPadding 内容区水平内边距；若调用方的 content 自带水平 padding，传 0.dp 避免叠加。
 */
@Composable
fun SrxCouiDialog(
    show: Boolean,
    title: String? = null,
    summary: String? = null,
    onDismissRequest: () -> Unit,
    renderInRootScaffold: Boolean = true,
    contentHorizontalPadding: Dp = 24.dp,
    content: @Composable () -> Unit,
) {
  val blur = panelBlur(MaterialBlurDefaults::dialog)
  val dark = isSrxDarkTheme()
  val seed = LocalSrxColorScheme.current
  // 弹窗与卡片共用 couiPanelSurface 这一份真值源。这里**恒按材质开启取色**：
  // 弹窗渲染在 root scaffold 上，`LocalPageBackdrop` 为空，panelBlur 必然返回 null，
  // 若按 `blur != null` 取色就会落到"材质关闭"分支（primary@4.5% over surface），
  // 面板变成奶油白实色，和主题页里"主题风格"选择器弹窗的玻璃面板色对不上。
  // 取不到 backdrop 只是没有模糊层，颜色仍应与选择器弹窗一致。
  // 面板色与卡片一致：直接用 COUI 官方 backgroundWithCard，不在这里做合成/染色。
  // （COUI 库禁止渲染在 backdrop 内的 popup 模糊页面，这是库的设计，不是配置问题。）
  val panelSurface = couiPanelSurface()
  OverlayDialog(
      show = show,
      title = title,
      summary = summary,
      onDismissRequest = onDismissRequest,
      renderInRootScaffold = renderInRootScaffold,
      backgroundColor = panelSurface,
      // 不再用模糊背景层：它是半透明的，会透出弹窗下面的页面（按钮看着像''透明''）。
      backgroundModifier = Modifier,
      materialStroke = LocalCOUIMaterialEffectEnabled.current,
      // COUI 默认 outsideMargin 更大（弹窗更窄），显式对齐 Miuix CenteredDialog 的 16dp。
      outsideMargin = DpSize(8.dp, 12.dp),
  ) {
    Box(
        modifier =
            Modifier.fillMaxWidth()
                .padding(horizontal = contentHorizontalPadding)
                .padding(bottom = 12.dp),
    ) {
      content()
    }
  }
}
