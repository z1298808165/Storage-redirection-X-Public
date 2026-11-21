package org.srx.manager.ui.coui

import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Outline
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import io.github.suqi8.coui.kmp.effect.LocalCOUIMaterialEffectEnabled
import io.github.suqi8.coui.kmp.squircle.SquircleShape
import io.github.suqi8.coui.kmp.squircle.isSquircleEnabled
import io.github.suqi8.coui.kmp.theme.COUIRoundCorner
import org.srx.manager.ui.theme.isSrxDarkTheme

/**
 * 卡片 / 弹窗 / 顶栏圆钮共用的面板表面色，两个主题同一真值源。
 *
 * 不能直接用 `seed.surface`：Miuix 的 `darkColorScheme().surface` 是 `Color.Black`， 深色模式直接注入会让 COUI
 * 卡片变成纯黑（实测 (18,14,13)），而 Miuix 卡片走 `glassPanel` 的 `surfaceContainer` 半透明叠层（实测 (51,44,42)）。 这里改用与
 * Miuix glassPanel 相同的 `surfaceContainer` 槽位：
 * - 材质开启：半透明玻璃色叠在页面网格背景上，深浅色都不会纯黑；
 * - 材质关闭：不透明实色，仍取自 surfaceContainer 系，避免回落到纯黑 surface。
 */
/**
 * 卡片 / 弹窗 / 圆钮共用的面板色：直接用 COUI 官方色板的 `backgroundWithCard`。
 *
 * 参考 KernelSU / stelliberty 的做法，面板颜色由主题自己决定，项目侧只提供强调色； 之前这里按 Miuix 的 surfaceContainer
 * 手工推导，还要区分材质开关，是面板"越调越怪"的根源。
 */
@Composable
fun couiPanelSurface(): Color =
    // 面板一律取 COUI 自己的卡片色（浅色纯白 / 深色纯深灰）。
    // 之前跟动态取色走 surfaceContainer，而 Monet 的容器色带主色相，
    // 结果 COUI 卡片被壁纸色染上一层，Miuix 侧走玻璃反而没这问题。
    io.github.suqi8.coui.kmp.theme.COUITheme.colorScheme.cardBackground

/** Eta 的 COUI 卡片用 SquircleShape(24dp)（CouiPageDefaults.shape），这里对齐到 24dp。 */
// 与 Miuix GlassCard 统一为 16dp：单行卡片才不会看起来像胶囊。
val CouiPanelCornerRadius: Dp = 24.dp

/**
 * COUI 面板的「内部边缘高光 + 内发光」，对齐 Eta `CouiContour` 的观感与几何。
 *
 * Eta 不用 `Modifier.border`：它先 `clip(CouiCapsuleShape)`，再用 `drawWithContent` 把 contour shader
 * 铺满整个面板，线宽 2.5–3dp，`lineAlpha` 浅色 0.8f / 深色 0.1f、 `innerGlowAlpha` 浅色 0.05f / 深色
 * 0.01f。边框完全落在形状**内部**，不向外溢出。
 *
 * 本实现用与 COUI Card 完全相同的 [SquircleShape]（半径 + Normal.Weight），把 outline 内缩半个线宽后再描边，因此高光与卡片 squircle
 * 边缘严格贴合，不会出现 RoundedCornerShape 那种「高光和卡片边缘大小不一致」。
 *
 * 材质模糊关闭时**仍绘制**（强度降到 0.55f），只是从玻璃质感回落为较淡的实色 描边；开关改变的是强度与卡片底色，不是「有/无」。
 */
@Composable
fun Modifier.couiPanelEdge(cornerRadius: Dp = CouiPanelCornerRadius): Modifier {
  val materialOn = LocalCOUIMaterialEffectEnabled.current
  val dark = isSrxDarkTheme()
  // 与 BasicCard.squircleSurface 相同的形状分支，保证几何一致
  val shape: Shape =
      if (isSquircleEnabled()) SquircleShape(cornerRadius, COUIRoundCorner.Normal.Weight)
      else RoundedCornerShape(cornerRadius)
  val strength = if (materialOn) 1f else 0.55f
  // 卡片高光对齐 Miuix glassPanel 的 rim/sheen（不是 Eta 的「胶囊按钮」CouiContour）：
  // Miuix 卡片 rim = White@0.16 再乘 0.35 ≈ 0.056，sheen = White@0.12，边缘描边 alpha≈0.44。
  // 之前误用了 Eta 按钮的 lineAlpha=0.8 + 3dp 纯白描边，套在卡片上就成了刺眼白边。
  val lineAlpha = (if (dark) 0.10f else 0.16f) * 0.35f * strength
  val innerGlowAlpha = (if (dark) 0.02f else 0.05f) * strength
  val innerShadowAlpha = (if (dark) 0.08f else 0.05f) * strength
  val sheenAlpha = (if (dark) 0.052f else 0.12f) * strength
  val rimBrush =
      Brush.verticalGradient(
          colors =
              listOf(
                  Color.White.copy(alpha = lineAlpha),
                  Color.White.copy(alpha = innerGlowAlpha),
                  Color.Black.copy(alpha = innerShadowAlpha),
              ),
      )
  return this.drawWithContent {
    drawContent()
    drawPanelContour(
        shape = shape,
        rimBrush = rimBrush,
        sheenAlpha = sheenAlpha,
        lineWidthPx = 1.5.dp.toPx(),
    )
  }
}

private fun DrawScope.drawPanelContour(
    shape: Shape,
    rimBrush: Brush,
    sheenAlpha: Float,
    lineWidthPx: Float,
) {
  val w = size.width.coerceAtLeast(1f)
  val h = size.height.coerceAtLeast(1f)
  // 顶部柔光：Miuix glassPanel 的 onDrawSurface(sheen)，让面板有纵向层次而非纯色片
  if (sheenAlpha > 0f) {
    val sheenHeight = (120.dp.toPx()).coerceAtMost(h)
    drawRect(
        brush =
            Brush.verticalGradient(
                colors =
                    listOf(
                        Color.White.copy(alpha = sheenAlpha),
                        Color.White.copy(alpha = sheenAlpha * 0.35f),
                        Color.Transparent,
                    ),
                // 必须把 endY 收到 sheenHeight，否则渐变会在矩形边界被裁断，
                // 在卡片中间留下一条明显的水平硬边。
                endY = sheenHeight,
            ),
        size = Size(w, sheenHeight),
    )
  }
  // outline 内缩半个线宽 + 以线宽描边 → 整圈高光完整落在卡片内部
  val inset = lineWidthPx / 2f
  val innerSize = Size((w - lineWidthPx).coerceAtLeast(1f), (h - lineWidthPx).coerceAtLeast(1f))
  val outline = shape.createOutline(innerSize, layoutDirection, this)
  val rimPath = Path()
  when (outline) {
    is Outline.Rectangle -> rimPath.addRect(outline.rect)
    is Outline.Rounded -> rimPath.addRoundRect(outline.roundRect)
    is Outline.Generic -> rimPath.addPath(outline.path)
  }
  translate(left = inset, top = inset) {
    drawPath(path = rimPath, brush = rimBrush, style = Stroke(width = lineWidthPx))
  }
}

/**
 * COUI 弹窗/浮层面板的底色：取 `backgroundWithCard`（页面底那一层）， 让内部
 * [couiPanelSurface]（`cardBackground`，纯白）的卡片能浮起来。
 */
@Composable
fun couiDialogSurface(): Color =
    io.github.suqi8.coui.kmp.theme.COUITheme.colorScheme.backgroundWithCard
