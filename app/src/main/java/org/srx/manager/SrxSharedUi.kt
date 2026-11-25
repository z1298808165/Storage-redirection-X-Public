package org.srx.manager

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.add
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import com.kyant.backdrop.Backdrop as KyantBackdrop
import kotlin.math.PI
import kotlin.math.roundToInt
import kotlin.math.sin
import org.srx.manager.ui.component.liquidPressScale
import org.srx.manager.ui.coui.couiCardFill
import org.srx.manager.ui.coui.couiPanelEdge
import org.srx.manager.ui.coui.couiPanelSurface
import org.srx.manager.ui.liquid.lens
import org.srx.manager.ui.liquid.vibrancy
import org.srx.manager.ui.theme.LocalSrxColorScheme
import org.srx.manager.ui.theme.SrxRowMetrics
import org.srx.manager.ui.theme.SrxShapes
import org.srx.manager.ui.theme.isSrxBlurEffectEnabled
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import org.srx.manager.ui.theme.isSrxThemeCoui
import org.srx.manager.ui.theme.srxAccentColor
import org.srx.manager.ui.theme.srxPageBackgroundColor
import org.srx.manager.ui.theme.srxSecondaryTextColor
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.CardDefaults
import top.yukonga.miuix.kmp.basic.CircularProgressIndicator
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.InfiniteProgressIndicator
import top.yukonga.miuix.kmp.basic.MiuixScrollBehavior
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.ScrollBehavior
import top.yukonga.miuix.kmp.basic.SmallTitle
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TopAppBar
import top.yukonga.miuix.kmp.blur.Backdrop
import top.yukonga.miuix.kmp.blur.LayerBackdrop
import top.yukonga.miuix.kmp.blur.ProgressiveBlur
import top.yukonga.miuix.kmp.blur.blur
import top.yukonga.miuix.kmp.blur.drawBackdrop
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.blur.progressiveBlur
import top.yukonga.miuix.kmp.blur.rememberLayerBackdrop
import top.yukonga.miuix.kmp.glass.GlassTopAppBar
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.Back
import top.yukonga.miuix.kmp.material.EdgeLightSource
import top.yukonga.miuix.kmp.material.EdgePeak
import top.yukonga.miuix.kmp.material.EdgeStroke
import top.yukonga.miuix.kmp.material.drawEdgeStroke
import top.yukonga.miuix.kmp.material.isRuntimeShaderSupported
import top.yukonga.miuix.kmp.utils.overScrollVertical

@Composable
internal fun srxSuccessColor(): Color =
    if (isSrxDarkTheme()) Color(0xFF50E0B1) else Color(0xFF14B37D)

@Composable
internal fun srxWarningColor(): Color =
    if (isSrxDarkTheme()) Color(0xFFF0C45C) else Color(0xFFD48B16)

@Composable
internal fun srxDangerColor(): Color =
    if (isSrxDarkTheme()) Color(0xFFFF7A7A) else Color(0xFFD94D4D)

@Composable internal fun srxPrimaryColor(): Color = LocalSrxColorScheme.current.primary

private val RoundActionSize = 38.dp
private val RoundActionIconSize = 19.dp

/** 无障碍要求的最小触控目标尺寸。 */
internal val MinTouchTargetSize = 48.dp

private val FloatingGlassStroke: EdgeStroke =
    EdgeStroke(
        width = 1.dp,
        bevel = 2.dp,
        color = Color.White.copy(alpha = 0.12f),
        primary = EdgeLightSource(0.5f, 0.5f, -0.5f, intensity = 0.4f),
        secondary = EdgeLightSource(0.5f, 0.6f, -0.5f, intensity = 0.25f),
        peak = EdgePeak.Axial,
    )

internal val LocalSrxBackdrop = staticCompositionLocalOf<Backdrop?> { null }
internal val LocalSrxBlurBackdrop = staticCompositionLocalOf<LayerBackdrop?> { null }

internal val LocalSrxOverlayBackdrop = staticCompositionLocalOf<Backdrop?> { null }
internal val LocalSrxKyantBackdrop = staticCompositionLocalOf<KyantBackdrop?> { null }

@Composable
internal fun isSrxBackdropEffectEnabled(): Boolean =
    (isSrxLiquidGlassEnabled() || isSrxBlurEffectEnabled()) && LocalSrxBackdrop.current != null

@Composable
internal fun glassSurfaceColor(alpha: Float = 0.7f): Color =
    // COUI 的内部面板（搜索框、胶囊、内层卡片）应取 COUI 卡片色：
    // surfaceContainerHigh 在 Monet 下带壁纸色相，会把面板染上一层取色。
    if (isSrxThemeCoui()) {
      // COUI 的面板一律是**不透明**的卡片色：带 alpha 的白叠在页面灰底上会偏灰，
      // 和旁边 GlassCard（纯白）并排就出现"同一页两种面板色"。
      couiPanelSurface()
    } else if (isSrxLiquidGlassEnabled() && isSrxBlurEffectEnabled()) {
      LocalSrxColorScheme.current.surfaceContainerHigh.copy(alpha = alpha.coerceIn(0.3f, 0.9f))
    } else {
      themedSurfaceColor()
    }

@Composable
internal fun themedSurfaceColor(): Color =
    // 用 Miuix 官方卡片底色，不再把强调色按比例叠进表面色。参考 KernelSU 管理器的
    // 做法：卡片/弹窗就吃主题的 surface，强调色只出现在文字、图标与选中态上。
    // 之前这里 primary@12% 叠 surface，等于给每张卡片都染了一层主题色。
    LocalSrxColorScheme.current.surface

@Composable
internal fun subtleFieldLabelColor(): Color =
    LocalSrxColorScheme.current.onSurfaceVariantSummary.copy(
        alpha = if (isSrxDarkTheme()) 0.58f else 0.48f
    )

@Composable
internal fun capsuleContainerColor(): Color =
    if (isSrxDarkTheme()) Color.White.copy(alpha = 0.12f) else Color.Black.copy(alpha = 0.07f)

@Composable
internal fun capsuleSelectedColor(): Color =
    if (isSrxDarkTheme()) Color.White.copy(alpha = 0.2f) else Color.White.copy(alpha = 0.78f)

@Composable
internal fun Modifier.glassPanel(
    shape: Shape,
    shadowAlpha: Float = 0.08f,
): Modifier {
  val dark = isSrxDarkTheme()
  val liquid = isSrxLiquidGlassEnabled()
  val blurEnabled = isSrxBlurEffectEnabled()
  // COUI 有自己的材质语言（实色 + contour），叠一层 Miuix 液态玻璃会让同一个页面里
  // 「走 COUI Card 的卡片」和「走 glassPanel 的容器（应用列表、概览大卡）」出现两种底色：
  // 一个平涂 cardBackground，一个被模糊/折射染成渐变色，深色模式下对比尤其明显。
  // 所以 COUI 下这里直接平涂 COUI 卡片色，玻璃只留给 Miuix 主题。
  if (isSrxThemeCoui()) {
    return this.background(couiCardFill(), shape)
  }
  val backdrop = LocalSrxBackdrop.current
  val hasBackdrop = backdrop != null && (liquid || blurEnabled)
  // 卡片/弹窗表面统一用官方 surface：不再按材质开关在 surfaceContainer 上叠 alpha。
  // 半透明 + 背后渐变会让浅色卡片看起来"被染色"，与 KernelSU 管理器的原生观感不同。
  // COUI 下必须取 COUI 自己的卡片色：LocalSrxColorScheme.surface 是「页面底」那一层，
  // 用它铺列表/大卡就等于把页面灰又涂了一遍，和白色卡片不是同一个色。
  val surface = if (isSrxThemeCoui()) couiPanelSurface() else LocalSrxColorScheme.current.surface
  // 顶部柔光收一档：原值在大面积卡片上会把底色"提亮成奶白"，
  // 与下方列表卡片并排时能看出色差（概览页那两张卡最明显）。
  val sheen = Color.White.copy(alpha = if (dark) 0.02f else 0.04f)
  val rim = Color.White.copy(alpha = if (dark) 0.08f else 0.16f)
  return this.dropShadow(
          shape = shape,
          shadow =
              Shadow(
                  radius = if (liquid) 18.dp else 10.dp,
                  color = if (dark) Color.Black else Color(0xFF667895),
                  alpha = shadowAlpha * if (liquid) 1.1f else 0.8f,
              ),
      )
      .then(
          if (hasBackdrop) {
            Modifier.clip(shape)
                .drawBackdrop(
                    backdrop = requireNotNull(backdrop),
                    shape = { shape },
                    effects = {
                      if (liquid) vibrancy()
                      if (blurEnabled) blur(32.dp.toPx(), 32.dp.toPx())
                      if (liquid) {
                        lens(
                            refractionHeight = 15.dp.toPx(),
                            refractionAmount = 16.dp.toPx(),
                            depthEffect = true,
                            chromaticAberration = 0.24f,
                        )
                      }
                    },
                    onDrawSurface = {
                      drawRect(surface)
                      if (liquid) {
                        drawRect(sheen)
                        drawRect(rim, alpha = 0.35f)
                      }
                    },
                    onDrawFront = {
                      if (liquid) {
                        drawEdgeStroke(
                            shape = shape,
                            stroke = FloatingGlassStroke,
                            alpha = if (dark) 0.72f else 0.68f,
                        )
                      }
                    },
                )
          } else {
            Modifier.background(surface, shape)
          },
      )
}

/**
 * 设置分组：COUI 下用官方 Card + CardListGroup（分隔线、首尾圆角、按压高光都由分组负责）， Miuix 下仍是 GlassCard + 顺序排列。调用方用 `item {
 * }` 描述每一行。
 *
 * COUI 官方指南规则 3 明确要求走这套分组结构；直接往 Card 里塞 Column 会让分隔线与 圆角位置失效。
 */
internal interface SrxGroupScope {
  fun item(key: Any? = null, showDivider: Boolean = true, content: @Composable () -> Unit)
}

private class SrxGroupCollector : SrxGroupScope {
  // apply 在组合期同步执行，仅收集 lambda，不渲染。

  val rows = mutableListOf<Triple<Any?, Boolean, @Composable () -> Unit>>()

  override fun item(key: Any?, showDivider: Boolean, content: @Composable () -> Unit) {
    rows += Triple(key, showDivider, content)
  }
}

@Composable
internal fun SettingGroup(
    modifier: Modifier = Modifier,
    insideMargin: PaddingValues = PaddingValues(0.dp),
    content: @Composable SrxGroupScope.() -> Unit,
) {
  val collector = SrxGroupCollector().apply { content() }
  if (isSrxThemeCoui()) {
    io.github.suqi8.coui.kmp.basic.Card(
        modifier = modifier.fillMaxWidth(),
        insideMargin = insideMargin,
        // 显式给卡片色：默认色会拿 COUI 的 cardBackground（半透明叠色），
        // 铺在带色相的页面底上会被染成偏蓝/偏灰，和 GlassCard 用的人造合成色不一致。
        colors = io.github.suqi8.coui.kmp.basic.CardDefaults.defaultColors(color = couiCardFill()),
    ) {
      // 行数变化（例如开关带出的二级行）时换 key，避免分组沿用旧槽位导致行不消失。
      androidx.compose.runtime.key(collector.rows.size) {
        io.github.suqi8.coui.kmp.basic.CardListGroup {
          collector.rows.forEach { (key, showDivider, row) ->
            item(key = key, showDivider = showDivider) { row() }
          }
        }
      }
    }
  } else {
    GlassCard(modifier = modifier, insideMargin = insideMargin) {
      Column(Modifier.fillMaxWidth()) { collector.rows.forEach { row -> row.third() } }
    }
  }
}

@Composable
internal fun GlassCard(
    modifier: Modifier = Modifier,
    cornerRadius: Dp = SrxShapes.card,
    insideMargin: PaddingValues = PaddingValues(0.dp),
    shadowAlpha: Float = 0.1f,
    content: @Composable () -> Unit,
) {
  if (isSrxThemeCoui()) {
    // Eta 的 COUI 卡片直接使用 COUI 官方 Card（CardDefaults.defaultColors(color = card)），
    // 材质（色调、描边、模糊）由 COUI 库自己绘制；手绘实心底会丢掉这层质感。
    // 自绘实色 + contour，而不是直接用 COUI 的 Card：Card 自身会按 COUI 材质开关
    // 叠一层半透明玻璃，在深色模式下和其它页面的容器（glassPanel 平涂）颜色不一致。
    // 层次靠 contour 表达（官方 CardShadow 在隐藏单边时会切出灰黑直角边，故不用）。
    // 弹窗面板本身就是卡片色，里面的分组卡片要再分一档才有层次：
    // 浅色压暗 2%、深色提亮 7%（深色下"更亮"才像浮起来）。
    val insideDialog = LocalSrxInsideDialog.current
    val couiFill =
        if (!insideDialog) couiCardFill()
        else if (isSrxDarkTheme()) lerp(couiCardFill(), Color.White, 0.07f)
        // 浅色下弹窗面板是纯白，卡片只压 2% 几乎看不见（用户反馈"卡片颜色不对"），
        // 5% 才和弹窗里其它灰卡（允许路径/路径映射）同一档。
        else lerp(couiCardFill(), Color.Black, 0.05f)
    Column(
        modifier =
            modifier
                .fillMaxWidth()
                .background(couiFill, RoundedCornerShape(cornerRadius))
                .then(
                    if (shadowAlpha > 0f) Modifier.couiPanelEdge(cornerRadius = cornerRadius)
                    else Modifier
                )
                .padding(insideMargin)
    ) {
      content()
    }
    return
  }
  val shape = RoundedCornerShape(cornerRadius)
  Card(
      modifier = modifier.glassPanel(shape, shadowAlpha),
      cornerRadius = cornerRadius,
      insideMargin = insideMargin,
      colors = CardDefaults.defaultColors(color = Color.Transparent),
  ) {
    content()
  }
}

/** 标记当前内容位于弹窗面板内：面板与卡片的层次要按这个标记区分。 */
internal val LocalSrxInsideDialog = androidx.compose.runtime.compositionLocalOf { false }

/** 弹窗/菜单统一的高一档表面：深色取 surfaceContainerHigh，浅色保持 surface。 */
@Composable
internal fun srxElevatedSurfaceColor(): Color {
  val colors = LocalSrxColorScheme.current
  return if (isSrxDarkTheme()) colors.surfaceContainerHigh else colors.surface
}

@Composable
internal fun CenteredDialog(
    show: Boolean,
    title: String? = null,
    summary: String? = null,
    modifier: Modifier = Modifier,
    maxWidth: Dp = 640.dp,
    outsideMargin: DpSize = DpSize(16.dp, 16.dp),
    insideMargin: DpSize = DpSize(24.dp, 20.dp),
    onDismiss: () -> Unit,
    content: @Composable () -> Unit,
) {
  val backdrop = LocalSrxOverlayBackdrop.current ?: LocalSrxBackdrop.current
  val couiStyle = isSrxThemeCoui()
  val shape = RoundedCornerShape(SrxShapes.card)
  val dark = isSrxDarkTheme()
  val outsideClick = remember { MutableInteractionSource() }
  val insideClick = remember { MutableInteractionSource() }
  if (!show) return
  // 用标准 Compose Dialog 自绘：MiuixPopupUtils.DialogLayout 依赖 Miuix 根 Scaffold，
  // COUI 主题下没有该宿主会让弹窗完全不渲染（两主题弹窗全部点不开的根因）。
  Dialog(
      onDismissRequest = onDismiss,
      properties =
          DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false),
  ) {
    BackHandler(enabled = show, onBack = onDismiss)
    CompositionLocalProvider(
        LocalSrxBackdrop provides backdrop,
        LocalSrxInsideDialog provides true,
    ) {
      Box(
          modifier =
              Modifier.fillMaxSize()
                  .background(Color.Black.copy(alpha = if (dark) 0.5f else 0.3f))
                  .clickable(
                      interactionSource = outsideClick,
                      indication = null,
                      onClick = onDismiss,
                  ),
      ) {
        // 与 COUI 弹窗一致：手机上贴底对齐但**离开底边**（Alignment.BottomCenter +
        // outsideMargin），而不是 Miuix OverlayBottomSheet 那种 edge-to-edge 贴死。
        // imePadding 放在内容层，输入法弹出时整块上移。
        Box(
            modifier = Modifier.fillMaxSize().imePadding(),
            contentAlignment = Alignment.BottomCenter,
        ) {
          Card(
              modifier =
                  modifier
                      .padding(
                          start = outsideMargin.width,
                          end = outsideMargin.width,
                          bottom = outsideMargin.height,
                      )
                      .widthIn(max = maxWidth)
                      .fillMaxWidth()
                      .dropShadow(
                          shape = shape,
                          shadow =
                              Shadow(
                                  radius = 18.dp,
                                  color = Color.Black,
                                  alpha = if (dark) 0.28f else 0.12f,
                              ),
                      )
                      .clickable(interactionSource = insideClick, indication = null) {},
              cornerRadius = SrxShapes.card,
              insideMargin =
                  PaddingValues(
                      horizontal = insideMargin.width,
                      vertical = insideMargin.height,
                  ),
              // Miuix Card 默认色是 surfaceContainer（动态色板下偏暖），与卡片用的
              // surface 不一致会显粉，统一取 surface。
              colors =
                  CardDefaults.defaultColors(
                      color =
                          // COUI 的 cardBackground 是「白色 10%」这类叠色，
                          // 先在页面底上合成成实色再用，否则面板半透明会透出页面。
                          if (couiStyle) couiCardFill()
                          // Miuix 侧同样要分层次：面板取 surface 往黑压一档，
                          // 里面的玻璃卡片（固定用 surface + 投影）才会浮起来。
                          // 不分档时两者同色，深色下完全看不出弹窗边界。
                          else srxElevatedSurfaceColor()
                  ),
          ) {
            Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
              if (title != null) {
                Text(title, fontWeight = FontWeight.Black, fontSize = 17.sp, lineHeight = 21.sp)
              }
              if (summary != null) {
                Text(
                    summary,
                    color = srxSecondaryTextColor(),
                    fontSize = 13.sp,
                    lineHeight = 19.sp,
                )
              }
              content()
            }
          }
        }
      }
    }
  }
}

/**
 * 按钮/图标底座的统一材质：实心面板 + 投影 + 顶部高光描边。
 *
 * 半透明色块（12%~14%）在浅色页面上与背景几乎同色，按用户的话就是「一块平的圆」。 这里给实心面板 + 5dp 投影 + 上亮下暗的背景渐变 + 一圈高光边，才有按压前的厚度感。
 *
 * @param tint 传 null 用中性面板（白/深灰），传颜色则用该色淡染（用于主按钮、危险按钮）。
 */
@Composable
internal fun Modifier.srxPanelSurface(
    shape: Shape,
    tint: Color? = null,
    elevation: Dp = 5.dp,
): Modifier {
  val dark = isSrxDarkTheme()
  val seed = LocalSrxColorScheme.current
  // COUI：Eta 的按钮就是「卡片色实底 + 一圈极淡描边」，没有渐变也没有投影。
  // 之前沿用的「上亮下暗 + 双层投影」是 Miuix 玻璃材质的画法，在 COUI 里偏"厚"。
  if (isSrxThemeCoui()) {
    // 中性按钮 = 卡片色实底；着色按钮（确定/删除）按 tint 混色，
    // 否则「取消」和「保存」在 COUI 下长得一模一样。
    val fill =
        if (tint == null) couiCardFill() else lerp(couiCardFill(), tint, if (dark) 0.30f else 0.16f)
    return this.background(fill, shape)
        .border(
            width = 1.dp,
            brush =
                Brush.verticalGradient(
                    listOf(
                        Color.White.copy(alpha = if (dark) 0.07f else 0.55f),
                        Color.Black.copy(alpha = if (dark) 0.16f else 0.04f),
                    )
                ),
            shape = shape,
        )
  }
  // 按钮底色以「卡片面板色」为基准，而不是 surface：COUI 深色下 surface 就是页面底
  // （近黑），拿它做渐变会让按钮上端跟页面一样黑，正是"按钮顶部偏黑"的成因。
  val base = if (isSrxThemeCoui()) couiCardFill() else seed.surface
  val top: Color
  val bottom: Color
  if (tint == null) {
    // 中性面板必须**不透明**，和着色分支同一个道理：半透明白色叠在深色页面上等于
    // 露出底色 + 底层投影，深色模式下就成了"上黑下灰"的一根灰棒，按钮顶部看着发黑。
    // 这里改成和 surface 混色的实色，深色比 surface 略亮、浅色接近纯白。
    // 深色下按钮要比卡片明显亮一档才看得出厚度：12% 在 COUI（面板本身带 10% 白）
    // 里刚好，但在 Miuix 的实色 surface 上仍然偏暗，所以统一提到 16%。
    top = if (dark) lerp(base, Color.White, 0.16f) else lerp(base, Color.White, 0.85f)
    bottom = if (dark) lerp(base, Color.Black, 0.08f) else lerp(base, Color.Black, 0.02f)
  } else {
    // 关键：着色必须**不透明**。之前用半透明 tint 叠渐变，下面的投影会从
    // 整块胶囊里透上来，把确认按钮染成一块深灰褐——看着像禁用态。
    // 这里直接和 surface 做混色，得到实色但仍保留强调色相。
    top = lerp(base, tint, if (dark) 0.34f else 0.15f)
    bottom = lerp(base, tint, if (dark) 0.22f else 0.09f)
  }
  // 阴影照 Miuix 玻璃材质的配方（GlassShadows.Regular）：纯黑 5% 透明度、
  // y 偏移 2dp、扩散半径很大。Compose 的 elevation shadow 会画出一圈偏硬的暗边，
  // 在浅色页面上就是"脏边"；这里改成两层柔和的 dropShadow：
  // 一层贴身的接触阴影，一层大范围的漫射，边缘完全渐隐。
  val shadowAlpha = if (dark) 0.24f else 0.06f
  return this.dropShadow(
          shape = shape,
          shadow =
              Shadow(
                  radius = 6.dp * (elevation.value / 5f).coerceIn(0.6f, 1.6f),
                  color = Color.Black,
                  alpha = shadowAlpha,
              ),
      )
      .dropShadow(
          shape = shape,
          shadow =
              Shadow(
                  radius = 18.dp * (elevation.value / 5f).coerceIn(0.6f, 1.6f),
                  color = Color.Black,
                  alpha = shadowAlpha * 0.75f,
              ),
      )
      .background(Brush.verticalGradient(listOf(top, bottom)), shape)
      .border(
          width = 0.9.dp,
          brush =
              Brush.verticalGradient(
                  listOf(
                      Color.White.copy(alpha = if (dark) 0.18f else 0.75f),
                      if (dark) Color.Black.copy(alpha = 0.12f)
                      else seed.primary.copy(alpha = 0.10f),
                  )
              ),
          shape = shape,
      )
}

/** 标记当前位于 GlassTopAppBar 内：顶栏按钮要继承玻璃材质（库里的 context 是 internal）。 */
internal val LocalSrxGlassTopBar = androidx.compose.runtime.compositionLocalOf { false }

/** Eta 的有向边缘光：只有上下两个峰值，向左右连续衰减到透明。 抽成共享函数，玻璃按钮和"无采样玻璃"（关于页）用同一套，避免一边是圆环一边是弧光。 */
private fun androidx.compose.ui.graphics.drawscope.DrawScope.drawSrxEdgeLight(
    dark: Boolean,
    shape: Shape? = null,
    inset: Float = 1.dp.toPx(),
    // 上下峰值不要拉太开：Eta 的 lit band 上下是对称的，
    // 之前 top 0.78 / bottom 0.38 会让顶部显得过亮、底部像没光。
    topPeak: Float = if (dark) 0.30f else 0.62f,
    bottomPeak: Float = if (dark) 0.24f else 0.44f,
) {
  // 非圆形（行内 +/X 是 10dp 圆角矩形）：用轮廓描边 + 竖向渐变，
  // 顶边白、中间透明、底边白，左右竖边自然是透明的。
  if (shape != null && shape != CircleShape) {
    val outline = shape.createOutline(size, layoutDirection, this)
    val path =
        when (outline) {
          is androidx.compose.ui.graphics.Outline.Generic -> outline.path
          is androidx.compose.ui.graphics.Outline.Rounded ->
              Path().apply { addRoundRect(outline.roundRect) }
          is androidx.compose.ui.graphics.Outline.Rectangle ->
              Path().apply { addRect(outline.rect) }
        }
    drawPath(
        path = path,
        brush =
            Brush.verticalGradient(
                listOf(
                    Color.White.copy(alpha = topPeak),
                    Color.Transparent,
                    Color.White.copy(alpha = bottomPeak),
                )
            ),
        style = Stroke(width = 1.1.dp.toPx()),
    )
    return
  }
  val center = Offset(size.width / 2f, size.height / 2f)
  // 之前用 5 个等距色标，alpha 要在 90° 里从 0 爬到峰值，绝大多数角度都只是半亮，
  // 所以怎么调峰值都不显眼。这里按角度逐点采样（每 5° 一个色标），
  // 峰值附近形成一小段"平台"，再向左右余弦衰减——这才是 Eta 的 dot(n,L)^2 布光观感。
  fun intensity(angle: Float, peak: Float): Float {
    val d = ((angle - 270f + 540f) % 360f) - 180f // 到顶部的角距（-180..180）
    val t = (kotlin.math.abs(d) / 90f).coerceIn(0f, 1f)
    val falloff = (0.5f * (1f + kotlin.math.cos(Math.PI * t))).toFloat()
    return peak * falloff * falloff
  }

  fun bottomIntensity(angle: Float, peak: Float): Float {
    val d = ((angle - 90f + 540f) % 360f) - 180f
    val t = (kotlin.math.abs(d) / 90f).coerceIn(0f, 1f)
    val falloff = (0.5f * (1f + kotlin.math.cos(Math.PI * t))).toFloat()
    return peak * falloff * falloff
  }

  val stops =
      (0..72).map { i ->
        val fraction = i / 72f
        val angle = fraction * 360f
        val alpha =
            maxOf(intensity(angle, topPeak), bottomIntensity(angle, bottomPeak)).coerceIn(0f, 1f)
        fraction to Color.White.copy(alpha = alpha)
      }
  drawCircle(
      brush = Brush.sweepGradient(*stops.toTypedArray(), center = center),
      radius = (size.minDimension / 2f - inset).coerceAtLeast(1f),
      style = Stroke(width = 1.3.dp.toPx()),
  )
}

/**
 * 全项目统一的「玻璃圆钮」材质：Eta 的 puredThinGlass 同款 —— backdrop 采样 + 模糊 + 折射/色散 + 低透明填充 + 细高光描边。顶栏按钮、行内
 * +/X、关于页返回都用它， 只通过 [fillAlpha] 调透明度；[progressive] 用于顶栏那种上糊下清的渐进模糊。 没有 backdrop
 * 的场合（例如某些弹窗）由调用方回落到原来的实心面板。
 */
@Composable
internal fun Modifier.srxGlassButtonSurface(
    backdrop: top.yukonga.miuix.kmp.blur.Backdrop,
    shape: Shape = CircleShape,
    fillAlpha: Float,
    blurRadius: Dp = 24.dp,
    progressive: Boolean = false,
    edgeLight: Boolean = true,
): Modifier {
  val dark = isSrxDarkTheme()
  val layer =
      backdrop as? LayerBackdrop
          ?: return this.background(
              Color.White.copy(alpha = fillAlpha * 0.6f),
              shape,
          )
  return this.clip(shape)
      .drawBackdrop(
          backdrop = layer,
          shape = { shape },
          effects = {
            vibrancy()
            // Eta 的顶栏按钮用的是渐进模糊：上边最糊、往下逐渐变清晰，
            // 比均匀 blur 更有玻璃层次/质感。
            if (progressive)
                progressiveBlur(
                    blurRadius.toPx(),
                    blurRadius.toPx(),
                    gradient = ProgressiveBlur.Top.copy(curve = 0.25f),
                )
            else blur(blurRadius.toPx(), blurRadius.toPx())
            lens(
                refractionHeight = 10.dp.toPx(),
                refractionAmount = 10.dp.toPx(),
                depthEffect = true,
                chromaticAberration = 0.18f,
            )
          },
          onDrawSurface = {
            // 玻璃体做上亮下暗的极淡竖向渐变：浅色页面本身接近纯白，
            // 若整体同色，白色边缘光就没有对比、看着"不亮"；压一点底部才有凸起感。
            // 深色下白色填充要按比例收：同样的 0.14+0.05 在暗底上会糊成一块白光。
            val whiteTone = if (dark) 0.35f else 1f
            drawRect(
                Brush.verticalGradient(
                    listOf(
                        Color.White.copy(alpha = (fillAlpha + 0.05f) * whiteTone),
                        Color.Black.copy(alpha = if (dark) 0.10f else 0.045f),
                    )
                )
            )
            // 顶部偏上的径向高光：模拟玻璃受光面，让圆钮看起来是凸的而不是平片。
            drawCircle(
                brush =
                    Brush.radialGradient(
                        listOf(
                            Color.White.copy(alpha = if (dark) 0.05f else 0.12f),
                            Color.Transparent,
                        ),
                        center = Offset(this.size.width / 2f, this.size.height * 0.40f),
                        radius = this.size.minDimension * 0.62f,
                    ),
                radius = this.size.minDimension / 2f,
            )
          },
          onDrawFront = { if (edgeLight) drawSrxEdgeLight(dark, shape = shape) },
      )
}

/** 没有 backdrop 但有渐变背景的页面（例如关于页）：用无采样版玻璃按钮。 */
internal val LocalSrxGlassFallbackButton = androidx.compose.runtime.compositionLocalOf { false }

@Composable
internal fun RoundIconAction(
    icon: ImageVector,
    contentDescription: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    danger: Boolean = false,
    enabled: Boolean = true,
    size: Dp = RoundActionSize,
    iconSize: Dp = RoundActionIconSize,
) {
  val color = if (danger) LocalSrxColorScheme.current.error else LocalSrxColorScheme.current.primary
  val glassBackdrop = LocalSrxBackdrop.current
  if (LocalSrxGlassTopBar.current && glassBackdrop != null) {
    // Eta 的顶栏按钮是「有轮廓的模糊玻璃圆」：这里保留我们自己的圆钮材质，
    // 靠低填充 + 模糊 + 一圈淡描边形成透明圆；顶层 GlassTopAppBar 不再叠第二层胶囊。
    val interaction = remember { MutableInteractionSource() }
    Box(
        modifier = modifier.sizeIn(minWidth = MinTouchTargetSize, minHeight = MinTouchTargetSize),
        contentAlignment = Alignment.Center,
    ) {
      // 玻璃材质本身 inner.alpha = 1（库把采样到的 backdrop 整块画成不透明），
      // 所以只在 fill 上给透明度没用——必须把整层玻璃的图层 alpha 降下来，
      // 页面底才会真正透出来。图标单独放在上层，保持全不透明。
      val darkTop = isSrxDarkTheme()
      val glassLayerAlpha = if (darkTop) 0.62f else 0.50f
      // 玻璃圆必须和图标一样是 size(38dp)；之前 matchParentSize 撑到 48dp 命中区，
      // 所以看起来"变大了"。48dp 只用于点击区域，视觉尺寸不跟着变。
      Box(
          modifier =
              Modifier.size(size)
                  .graphicsLayer { alpha = glassLayerAlpha * if (enabled) 1f else 0.45f }
                  // 用完整材质（vibrancy + lens + sheen/rim）而不是 light 模式：
                  // 整体 alpha 已经负责通透，材质负责 Eta 那种玻璃质感。
                  .srxGlassButtonSurface(
                      backdrop = glassBackdrop,
                      fillAlpha = 0.05f,
                      blurRadius = 24.dp,
                      progressive = true,
                      edgeLight = false,
                  )
      )
      // 边缘光必须画在降 alpha 的玻璃层**外面**：Eta 的 lit band 是满强度叠加，
      // 之前放在 alpha 图层里会被 0.5 乘掉一半，所以怎么调都"不亮"。
      Box(
          modifier =
              Modifier.size(size).drawWithContent {
                drawContent()
                // 顶栏按钮背后是顶栏自己的模糊带，玻璃本体又降过 alpha，
                // 同一组峰值在设置/主题页会明显比关于页暗——这里按页面底独立提亮。
                drawSrxEdgeLight(
                    darkTop,
                    topPeak = if (darkTop) 0.52f else 0.80f,
                    bottomPeak = if (darkTop) 0.40f else 0.62f,
                )
              }
      )
      Box(
          modifier =
              Modifier.size(size)
                  .liquidPressScale(interaction)
                  .alpha(if (enabled) 1f else 0.45f)
                  .clickable(
                      enabled = enabled,
                      interactionSource = interaction,
                      indication = null,
                      onClick = onClick,
                  ),
          contentAlignment = Alignment.Center,
      ) {
        Icon(
            icon,
            contentDescription = contentDescription,
            tint = color,
            modifier = Modifier.size(iconSize),
        )
      }
    }
    return
  }
  // 关于页优先：即使 BgEffectBackground 提供了 backdrop 也不能用它采样
  // （之前叠层触发过 libhwui 崩溃），所以无条件走无采样玻璃。
  if (LocalSrxGlassFallbackButton.current) {
    val fallbackDark = isSrxDarkTheme()
    // 关于页：BgEffectBackground 自带渐变背景，不能再叠一层 layerBackdrop（会触发
    // libhwui pinImages native 崩溃）。这里用无采样版：低透明填充 + 亮边 + 内阴影，
    // 叠在渐变背景上观感和玻璃一致。
    val interaction = remember { MutableInteractionSource() }
    Box(
        modifier = modifier.sizeIn(minWidth = MinTouchTargetSize, minHeight = MinTouchTargetSize),
        contentAlignment = Alignment.Center,
    ) {
      Box(
          modifier =
              Modifier.size(size)
                  .clip(CircleShape)
                  .graphicsLayer { alpha = if (enabled) 0.50f else 0.24f }
                  .background(Color.White.copy(alpha = 0.16f), CircleShape)
      )
      Box(
          modifier =
              Modifier.size(size).drawWithContent {
                drawContent()
                drawSrxEdgeLight(fallbackDark)
              }
      )
      // 图标不能跟着玻璃层一起降 alpha，否则箭头会淡到看不见。
      Box(
          modifier =
              Modifier.size(size)
                  .liquidPressScale(interaction)
                  .alpha(if (enabled) 1f else 0.45f)
                  .clickable(
                      enabled = enabled,
                      interactionSource = interaction,
                      indication = null,
                      onClick = onClick,
                  ),
          contentAlignment = Alignment.Center,
      ) {
        Icon(
            icon,
            contentDescription = contentDescription,
            tint = color,
            modifier = Modifier.size(iconSize),
        )
      }
    }
    return
  }
  Box(
      modifier =
          modifier
              // 圆形按钮视觉尺寸小于无障碍基线的 48dp，这里把可点击区域撑到基线大小，
              // 内层保持原有视觉尺寸，外观不变。
              .sizeIn(minWidth = MinTouchTargetSize, minHeight = MinTouchTargetSize),
      contentAlignment = Alignment.Center,
  ) {
    RoundIconActionSurface(
        icon = icon,
        contentDescription = contentDescription,
        onClick = onClick,
        color = color,
        danger = danger,
        enabled = enabled,
        size = size,
        iconSize = iconSize,
    )
  }
}

@Composable
private fun RoundIconActionSurface(
    icon: ImageVector,
    contentDescription: String,
    onClick: () -> Unit,
    color: Color,
    danger: Boolean,
    enabled: Boolean,
    size: Dp,
    iconSize: Dp,
) {
  // 用「实心面板 + 投影 + 顶高光」而不是半透明填充：14% 的色块在浅色页面上
  // 和背景几乎同色，看起来就是一块平的圆（用户反馈"没有质感和立体感"）。
  // 面板取 surface 系颜色保持与卡片同族，靠投影和高光给出厚度。
  val interactionSource = remember { MutableInteractionSource() }
  val dark = isSrxDarkTheme()
  val seed = LocalSrxColorScheme.current
  val tint = if (danger) seed.error else color
  Box(
      modifier =
          Modifier.size(size)
              .srxPanelSurface(shape = CircleShape)
              .liquidPressScale(interactionSource)
              .alpha(if (enabled) 1f else 0.45f)
              .clickable(
                  enabled = enabled,
                  interactionSource = interactionSource,
                  indication = null,
                  onClick = onClick,
              ),
      contentAlignment = Alignment.Center,
  ) {
    Icon(
        icon,
        contentDescription = contentDescription,
        tint = tint,
        modifier = Modifier.size(iconSize),
    )
  }
}

/** 便捷取当前色板，避免每处都写 LocalSrxColorScheme.current。 */
private val srxColorScheme: top.yukonga.miuix.kmp.theme.Colors
  @Composable get() = LocalSrxColorScheme.current

@Composable
internal fun GlassTextButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    primary: Boolean = false,
    danger: Boolean = false,
    enabled: Boolean = true,
) {
  val tint =
      when {
        danger -> srxColorScheme.error
        primary -> srxPrimaryColor()
        else -> srxColorScheme.onSurface
      }
  // 自绘容器：Miuix TextButton 会按自己的 size 画内层底色，导致"外层 48dp、内层更矮"，
  // 不同弹窗里看着高矮不一。这里整块就是按钮，尺寸完全由我们决定。
  Box(
      modifier =
          modifier
              .height(48.dp)
              .clip(CircleShape)
              .srxPanelSurface(
                  shape = CircleShape,
                  tint = if (danger || primary) tint else null,
                  elevation = if (enabled) (if (danger || primary) 2.dp else 4.dp) else 0.dp,
              )
              .clickable(enabled = enabled, onClick = onClick),
      contentAlignment = Alignment.Center,
  ) {
    Text(
        text = text,
        color = if (enabled) tint else tint.copy(alpha = 0.4f),
        fontSize = 15.sp,
        lineHeight = 20.sp,
        fontWeight = FontWeight.Bold,
        maxLines = 1,
    )
  }
}

private data class SrxMeshPalette(
    val dark: Boolean,
    val primary: Color,
    val success: Color,
    val base: Color,
    val elevated: Color,
)

@Composable
private fun rememberSrxMeshPalette(): SrxMeshPalette {
  val dark = isSrxDarkTheme()
  val success = srxSuccessColor()
  // 两个主题共用同一份按用户偏好算出的色板：切换主题只换组件，不改变页面配色。
  // 若各读各库的默认色板，COUI 会拿到写死的灰底，与 Miuix 的取色结果不一致。
  val colors = LocalSrxColorScheme.current
  val primary = colors.primary
  val base = colors.surface
  val elevated = colors.surfaceContainerHigh
  return remember(dark, primary, success, base, elevated) {
    SrxMeshPalette(dark, primary, success, base, elevated)
  }
}

private fun DrawScope.drawSrxMeshBackground(palette: SrxMeshPalette, w: Float, h: Float) {
  drawRect(palette.base)
  drawRect(
      Brush.linearGradient(
          listOf(
              palette.elevated.copy(alpha = if (palette.dark) 0.32f else 0.72f),
              palette.base.copy(alpha = 0.18f),
              palette.elevated.copy(alpha = if (palette.dark) 0.22f else 0.58f),
          ),
          start = Offset.Zero,
          end = Offset(w, h),
      ),
  )
  drawRect(
      Brush.linearGradient(
          listOf(
              palette.primary.copy(alpha = if (palette.dark) 0.22f else 0.16f),
              Color.Transparent,
              palette.success.copy(alpha = if (palette.dark) 0.12f else 0.10f),
          ),
          start = Offset(0f, 0f),
          end = Offset(w, h),
      ),
  )
  drawRect(
      Brush.linearGradient(
          listOf(
              Color.Transparent,
              palette.primary.copy(alpha = if (palette.dark) 0.035f else 0.024f),
              Color.Transparent,
          ),
          start = Offset(w * 0.92f, 0f),
          end = Offset(w * 0.08f, h * 0.82f),
      ),
  )
}

@Composable
internal fun rememberBlurBackdrop(enableBlur: Boolean): LayerBackdrop? {
  if (!enableBlur || !isRuntimeShaderSupported()) return null
  // 背景已改为纯色，backdrop 录制同样只铺纯色：否则模糊层会采到渐变，出现色带。
  val background = srxPageBackgroundColor()
  return rememberLayerBackdrop {
    drawRect(background)
    drawContent()
  }
}

/**
 * OS4 顶栏：直接复用 Miuix GlassTopAppBar 的 progressiveBlur。
 *
 * 0.9.5-beta04 起组件内部按 scrollBehavior 自己算模糊带的折叠进度（移除了 visualContentOffset 参数）， 这里只需透传 backdrop
 * 与滚动状态；bandBrush 保持透明，避免出现独立 surface 色块。
 */
@Composable
private fun SrxProgressiveBlurTopBar(
    title: String,
    subtitle: String,
    scrollBehavior: ScrollBehavior,
    backdrop: LayerBackdrop?,
    onBack: (() -> Unit)?,
    actions: @Composable RowScope.() -> Unit,
    bottomContent: @Composable () -> Unit,
) {
  val navigationIcon: @Composable () -> Unit = {
    if (onBack != null) {
      RoundIconAction(MiuixIcons.Glass.Back, "返回", onBack)
    }
  }
  if (backdrop != null) {
    androidx.compose.runtime.CompositionLocalProvider(LocalSrxGlassTopBar provides true) {
      GlassTopAppBar(
          title = title,
          subtitle = subtitle,
          backdrop = backdrop,
          progressiveBlur = true,
          // 未收缩的顶栏必须和页面同色，否则大标题会和页面出现一条色带。
          // 玻璃质感全部放在按钮上（按钮自己采样 backdrop），不依赖顶栏 band。
          bandBrush = Brush.verticalGradient(listOf(Color.Transparent, Color.Transparent)),
          scrollBehavior = scrollBehavior,
          navigationIcon = navigationIcon,
          actions = actions,
          bottomContent = bottomContent,
      )
    }
  } else {
    TopAppBar(
        title = title,
        subtitle = subtitle,
        modifier = Modifier.srxTopBarMeshBackground(),
        // 关闭材质模糊时不能回落成不透明单色：内容层是网格渐变，顶栏会变成一条
        // 与页面割裂的色带，深色模式下就是用户看到的「顶栏变黑」。这里用与页面
        // 同一套全屏网格渐变绘制顶栏背景，颜色连续且能遮住滚动内容。
        color = Color.Transparent,
        scrollBehavior = scrollBehavior,
        navigationIcon = navigationIcon,
        actions = actions,
        bottomContent = bottomContent,
    )
  }
}

/**
 * 每页单独创建 backdrop，Scaffold 顶栏采样全屏内容层；progressiveTextureBlur 随滚动进度渐入， TopAppBar 通过 scrollBehavior
 * 折叠标题。内容层与顶栏处于同一坐标系，不会把顶栏自身纳入采样。
 */
@Composable
internal fun SrxProgressivePage(
    title: String,
    subtitle: String = "",
    onBack: (() -> Unit)? = null,
    actions: @Composable RowScope.() -> Unit = {},
    bottomContent: @Composable () -> Unit = {},
    content: @Composable (Modifier, PaddingValues, Modifier) -> Unit,
) {
  val scrollBehavior = MiuixScrollBehavior()
  val backdrop = rememberBlurBackdrop(isSrxBlurEffectEnabled())
  val state = scrollBehavior.state
  val onOffsetChanged: (Float) -> Unit = remember(state) { { state.overscrollOffset = it } }

  Scaffold(
      modifier = Modifier.fillMaxSize(),
      containerColor = Color.Transparent,
      // 顶部 inset 由 TopAppBar 处理；底部由外层底栏或调用方传入的 bottomPadding 处理，
      // 这里只保留水平 inset，避免嵌套 Scaffold 重复加顶部/底部留白。
      contentWindowInsets =
          WindowInsets.systemBars
              .add(WindowInsets.displayCutout)
              .only(WindowInsetsSides.Horizontal),
      topBar = {
        SrxProgressiveBlurTopBar(
            title = title,
            subtitle = subtitle,
            scrollBehavior = scrollBehavior,
            backdrop = backdrop,
            onBack = onBack,
            actions = actions,
            bottomContent = bottomContent,
        )
      },
  ) { innerPadding ->
    val pageModifier =
        Modifier.fillMaxSize()
            .then(if (backdrop != null) Modifier.layerBackdrop(backdrop) else Modifier)
            .appMeshBackground()
    // 这两个 modifier 必须由调用方挂在「真正产生滚动」的节点上，且顺序不能颠倒：
    // overScrollVertical 的 onPostScroll 会吞掉全部剩余下拉，而顶栏连接要靠这份
    // available.y 把 heightOffset 加回去。若挂到外层容器，中间的 PullToRefresh
    // 会先截断 delta，标题折叠后就再也展开不了（KSU / Miuix 官方示例同款写法）。
    val scrollModifiers =
        Modifier.overScrollVertical(onOffsetChanged = onOffsetChanged)
            .nestedScroll(scrollBehavior.nestedScrollConnection)
    content(pageModifier, innerPadding, scrollModifiers)
  }
}

@Composable
internal fun SrxProgressiveListPage(
    title: String,
    subtitle: String = "",
    onBack: (() -> Unit)? = null,
    actions: @Composable RowScope.() -> Unit = {},
    bottomContent: @Composable () -> Unit = {},
    listState: LazyListState = rememberLazyListState(),
    bottomPadding: Dp = 0.dp,
    // Eta：Miuix 页面左右留白 12dp、COUI 16dp，这也是两套风格一眼可辨的地方。
    horizontalPadding: Dp = if (isSrxThemeCoui()) 16.dp else 12.dp,
    topSpacing: Dp = 12.dp,
    bottomSpacing: Dp = 28.dp,
    verticalArrangement: Arrangement.Vertical = Arrangement.spacedBy(SrxRowMetrics.groupSpacing()),
    content: LazyListScope.() -> Unit,
) {
  SrxProgressivePage(
      title = title,
      subtitle = subtitle,
      onBack = onBack,
      actions = actions,
      bottomContent = bottomContent,
  ) { pageModifier, innerPadding, scrollModifiers ->
    val layoutDirection = LocalLayoutDirection.current
    LazyColumn(
        state = listState,
        modifier = pageModifier.then(scrollModifiers),
        contentPadding =
            PaddingValues(
                top = innerPadding.calculateTopPadding() + topSpacing,
                bottom = bottomPadding + bottomSpacing,
                start = horizontalPadding + innerPadding.calculateLeftPadding(layoutDirection),
                end = horizontalPadding + innerPadding.calculateRightPadding(layoutDirection),
            ),
        verticalArrangement = verticalArrangement,
        overscrollEffect = null,
        content = content,
    )
  }
}

@Composable
internal fun SectionTitle(text: String) {
  if (isSrxThemeCoui()) {
    // COUI：强调色小标题（Eta 的 section header 风格），Miuix 保持灰字 SmallTitle。
    Text(
        text = text,
        modifier = Modifier.padding(start = 6.dp, bottom = 8.dp),
        color = srxAccentColor().copy(alpha = 0.9f),
        fontSize = 14.sp,
        lineHeight = 18.sp,
        fontWeight = FontWeight.Black,
    )
    return
  }
  SmallTitle(
      text = text,
      modifier = Modifier.padding(start = 6.dp, bottom = 8.dp),
      insideMargin = PaddingValues(0.dp),
  )
}

@Composable
internal fun EmptyText(text: String) {
  Box(
      modifier = Modifier.fillMaxWidth().padding(28.dp),
      contentAlignment = Alignment.Center,
  ) {
    Text(text, color = LocalSrxColorScheme.current.onSurfaceVariantSummary)
  }
}

@Composable
internal fun busyScrimColor(): Color =
    // 与 CenteredDialog 的遮罩保持一致：浅色 0.3、深色 0.5 的黑色。
    // 之前浅色用 0xFFEAF0FA 的 66%，盖在页面上是一层"朦胧白"，与其它弹窗完全不同。
    Color.Black.copy(alpha = if (isSrxDarkTheme()) 0.5f else 0.3f)

/**
 * 进度条上的"波浪带"：与进度值无关地持续横向流动，因此即使进度停住也能看出"还在跑"。 单独叠在 LinearProgressIndicator 之上，不改动它本身的 determinate
 * 表现。
 */
@Composable
internal fun WaveSheen(modifier: Modifier, color: Color) {
  val transition = rememberInfiniteTransition(label = "waveSheen")
  val phase by
      transition.animateFloat(
          initialValue = 0f,
          targetValue = 1f,
          animationSpec = infiniteRepeatable(tween(1300, easing = LinearEasing)),
          label = "wavePhase",
      )
  Canvas(modifier) {
    // 正弦波：一条沿 x 轴前进的波峰波谷曲线，覆盖在进度条上（并非竖直渐变条）。
    val waveLength = size.width / 2.2f
    val amplitude = size.height * 0.42f
    val centerY = size.height / 2f
    val steps = 72
    val path = Path()
    for (i in 0..steps) {
      val x = size.width * i / steps
      val y = centerY + amplitude * sin((x / waveLength + phase) * 2f * PI.toFloat())
      if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
    }
    drawPath(
        path = path,
        color = color.copy(alpha = 0.85f),
        style = Stroke(width = size.height * 0.9f, cap = StrokeCap.Round),
    )
  }
}

/** 环上叠一段持续旋转的弧，用于表达"仍在进行"，与 determinate 圆环共存。 */
@Composable
internal fun RingSweep(modifier: Modifier, color: Color, strokeWidth: Dp = 4.dp) {
  val transition = rememberInfiniteTransition(label = "ringSweep")
  val angle by
      transition.animateFloat(
          initialValue = 0f,
          targetValue = 360f,
          animationSpec = infiniteRepeatable(tween(1100, easing = LinearEasing)),
          label = "ringAngle",
      )
  Canvas(modifier) {
    // 弧要完全落在圆环上：按 1/2 线宽内缩，避免出现"另一圈大小不一致的弧贴在旁边"。
    val inset = strokeWidth.toPx() / 2f
    rotate(angle) {
      drawArc(
          color = color.copy(alpha = 0.9f),
          startAngle = 0f,
          sweepAngle = 70f,
          useCenter = false,
          topLeft = Offset(inset, inset),
          size = Size(size.width - inset * 2f, size.height - inset * 2f),
          style = Stroke(width = strokeWidth.toPx(), cap = StrokeCap.Round),
      )
    }
  }
}

@Composable
internal fun BusyOverlay(message: String, progress: Float? = null) {
  val shape = RoundedCornerShape(SrxShapes.card)
  val rawProgress = progress?.coerceIn(0f, 1f)
  // 进度值做补间：底层上报是跳跃的，直接画会"一格一格"跳；这里平滑过渡，环与条一起动。
  val animatedProgress by
      animateFloatAsState(
          targetValue = rawProgress ?: 0f,
          animationSpec = tween(durationMillis = 420, easing = FastOutSlowInEasing),
          label = "busyProgress",
      )
  val safeProgress = rawProgress?.let { animatedProgress }
  Column(
      modifier =
          Modifier.widthIn(min = 220.dp, max = 320.dp)
              .dropShadow(
                  shape,
                  Shadow(
                      radius = 24.dp,
                      color = if (isSrxDarkTheme()) Color.Black else Color(0xFF8EA4C8),
                      alpha = 0.18f,
                  ),
              )
              .clip(shape)
              .background(LocalSrxColorScheme.current.surface, shape)
              .padding(horizontal = 24.dp, vertical = 22.dp),
      horizontalAlignment = Alignment.CenterHorizontally,
      verticalArrangement = Arrangement.spacedBy(SrxRowMetrics.groupSpacing()),
  ) {
    if (safeProgress == null) {
      InfiniteProgressIndicator(modifier = Modifier.size(34.dp), size = 34.dp)
    } else {
      // 自绘：轨道、进度弧、旋转高光片段画在同一个 Canvas、同一半径与线宽上，
      // 不会出现"另一圈大小不一致的弧贴着转"。
      SrxProgressRing(progress = safeProgress, size = 44.dp, strokeWidth = 4.dp)
    }
    Text(
        text = message,
        color = LocalSrxColorScheme.current.onSurface,
        fontSize = 15.sp,
        fontWeight = FontWeight.Bold,
        textAlign = TextAlign.Center,
    )
    if (safeProgress != null) {
      // 水波：填充区内部是一条持续流动的正弦水面，进度停住时波浪仍在走。
      // 高度给到 20dp：振幅 = (高 - 线宽)/2，8dp 时只有 2.5dp，波峰几乎看不见。
      SrxWaveProgressBar(
          progress = safeProgress,
          modifier = Modifier.fillMaxWidth().height(20.dp),
      )
    }
  }
}

/** 自绘圆形进度：轨道、进度弧与旋转高光片在**同一个 Canvas、同一半径、同一线宽**上绘制， 因此不会出现旧实现里"另一圈大小不一致的弧贴着圆环转"的错位。 */
@Composable
internal fun SrxProgressRing(progress: Float, size: Dp, strokeWidth: Dp) {
  val accent = srxAccentColor()
  val track = LocalSrxColorScheme.current.onSurface.copy(alpha = 0.12f)
  val shown by
      animateFloatAsState(
          targetValue = progress.coerceIn(0f, 1f),
          animationSpec = tween(durationMillis = 420, easing = FastOutSlowInEasing),
          label = "ringProgress",
      )
  Box(Modifier.size(size), contentAlignment = Alignment.Center) {
    Canvas(Modifier.fillMaxSize()) {
      val stroke = strokeWidth.toPx()
      val inset = stroke / 2f
      val ringSize = Size(this.size.width - stroke, this.size.height - stroke)
      drawArc(
          color = track,
          startAngle = 0f,
          sweepAngle = 360f,
          useCenter = false,
          topLeft = Offset(inset, inset),
          size = ringSize,
          style = Stroke(width = stroke),
      )
      // 只有进度弧，不再叠旋转高光。
      drawArc(
          color = accent,
          startAngle = -90f,
          sweepAngle = 360f * shown,
          useCenter = false,
          topLeft = Offset(inset, inset),
          size = ringSize,
          style = Stroke(width = stroke, cap = StrokeCap.Round),
      )
    }
    // 中心百分比：数字变化时纵向滚动切换（里程表效果）。
    val percent = (shown * 100f).roundToInt().coerceIn(0, 100)
    AnimatedContent(
        targetState = percent,
        transitionSpec = {
          (slideInVertically { it } + fadeIn()) togetherWith
              (slideOutVertically { -it } + fadeOut())
        },
        label = "ringPercent",
    ) { value ->
      Text(
          text = "$value%",
          color = LocalSrxColorScheme.current.onSurface,
          fontSize = 11.sp,
          fontWeight = FontWeight.Black,
      )
    }
  }
}

/** 水波进度条：填充区域内部是一条持续右移的正弦水面（波峰波谷）， 即使进度值不变，波浪也一直在流——这就是"进度停住时仍能看出在跑"的效果。 */
@Composable
internal fun SrxWaveProgressBar(progress: Float, modifier: Modifier = Modifier) {
  val accent = srxAccentColor()
  val trackColor = LocalSrxColorScheme.current.onSurface.copy(alpha = 0.10f)
  val phase by
      rememberInfiniteTransition(label = "waveFlow")
          .animateFloat(
              initialValue = 0f,
              targetValue = 1f,
              animationSpec = infiniteRepeatable(tween(900, easing = LinearEasing)),
              label = "wavePhase",
          )
  Canvas(modifier) {
    val strokeWidth = 3.dp.toPx()
    // 振幅在地板高度的一半基础上再收三分之一：20dp 高的条约 5.7dp 起伏，看着不夸张。
    val amplitude = (size.height - strokeWidth) / 2f * 0.45f
    val centerY = size.height / 2f
    val waveLength = size.width / 7f
    val steps = 96

    fun wavePath(width: Float): Path {
      val path = Path()
      for (i in 0..steps) {
        val x = width * i / steps
        // 单一正弦：叠加次波会让波峰互相干扰、看起来发乱，这里只用一组波。
        val y = centerY + amplitude * sin((x / waveLength - phase) * 2f * PI.toFloat())
        if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
      }
      return path
    }

    // 进度：圆角正弦线，波峰波谷上下起伏并整体右移；按进度裁剪长度。
    val filled = size.width * progress.coerceIn(0f, 1f)
    if (filled > 0f) {
      clipPath(
          Path().apply { addRect(androidx.compose.ui.geometry.Rect(0f, 0f, filled, size.height)) }
      ) {
        drawPath(
            path = wavePath(size.width),
            color = accent,
            style = Stroke(width = strokeWidth, cap = StrokeCap.Round, join = StrokeJoin.Round),
        )
      }
    }
  }
}

@Composable
private fun DeterminateProgressRing(progress: Float) {
  val percentText = "${(progress * 100f).roundToInt().coerceIn(0, 100)}%"
  Box(
      modifier = Modifier.size(44.dp),
      contentAlignment = Alignment.Center,
  ) {
    CircularProgressIndicator(
        progress = progress.coerceIn(0f, 1f),
        modifier = Modifier.size(44.dp),
        strokeWidth = 4.dp,
        size = 44.dp,
    )
    RingSweep(Modifier.size(44.dp), srxAccentColor(), strokeWidth = 4.dp)
    Text(
        text = percentText,
        color = LocalSrxColorScheme.current.onSurface,
        fontSize = 10.sp,
        fontWeight = FontWeight.Black,
        textAlign = TextAlign.Center,
    )
  }
}

@Composable
internal fun ToastPill(text: String, modifier: Modifier = Modifier) {
  val shape = CircleShape
  Text(
      text = text,
      modifier =
          modifier
              .glassPanel(shape, shadowAlpha = 0.12f)
              .clip(shape)
              .padding(horizontal = 18.dp, vertical = 11.dp),
      color = LocalSrxColorScheme.current.onSurface,
      fontWeight = FontWeight.SemiBold,
      fontSize = 13.sp,
  )
}

@Composable
internal fun Modifier.appMeshBackground(): Modifier {
  // 改为纯色背景：网格渐变会让浅色卡片看起来像被染色，也让两个主题的观感互相比出冷暖差。
  // 名字保留 appMeshBackground，避免改动 4 处调用点与既有测试。
  // 页面底色比卡片暗一档，卡片（surface）才能"浮"起来。
  val background = srxPageBackgroundColor()
  return background(background)
}

/**
 * 顶栏专用的不透明网格背景：用整屏渐变参数绘制，只裁到顶栏高度，因此顶栏显示的 恰好是页面渐变的顶部一段，颜色严格连续；同时能遮住滚动到顶栏下方的内容。 关闭材质模糊时没有 backdrop
 * 可做玻璃遮罩，纯透明顶栏会让列表文字穿透重叠。
 */
@Composable
internal fun Modifier.srxTopBarMeshBackground(): Modifier {
  // 顶栏同样使用页面底色，保证与页面严格一致（原先靠"整屏渐变裁到顶栏高度"对齐）。
  return background(srxPageBackgroundColor())
}

/** 两主题共用的确认弹窗：与页面里手写的 CenteredDialog + 两个 GlassTextButton 完全同构。 */
@Composable
internal fun SrxConfirmDialog(
    show: Boolean,
    summary: String,
    onDismiss: () -> Unit,
    onConfirm: () -> Unit,
    title: String? = null,
    cancelText: String = "取消",
    confirmText: String = "确认",
    danger: Boolean = false,
) {
  CenteredDialog(title = title, summary = summary, show = show, onDismiss = onDismiss) {
    // 两个主题统一用同一套 44dp 按钮：此前 COUI 走 DialogButtonBar，其行高与
    // 模板弹窗的取消/保存不一致，肉眼就是"这个弹窗的按钮偏高/偏矮"。
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
      GlassTextButton(cancelText, onDismiss, modifier = Modifier.weight(1f))
      GlassTextButton(
          confirmText,
          onConfirm,
          modifier = Modifier.weight(1f),
          primary = !danger,
          danger = danger,
      )
    }
  }
}

/** 两主题共用的信息弹窗（单按钮）。 */
@Composable
internal fun SrxInfoDialog(
    show: Boolean,
    summary: String,
    onDismiss: () -> Unit,
    title: String? = null,
    confirmText: String = "知道了",
) {
  CenteredDialog(title = title, summary = summary, show = show, onDismiss = onDismiss) {
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
      GlassTextButton(confirmText, onDismiss, modifier = Modifier.weight(1f), primary = true)
    }
  }
}

/**
 * 跨主题切换保留滚动位置：切换 COUI/Miuix 会 dispose 整个内容子树， `rememberLazyListState()` 随之归零、页面回到顶部。把状态存在主题之外的
 * 缓存里，重建后按 key 取回同一个实例即可原位恢复。
 */
internal class SrxScrollCache {
  private val states = mutableMapOf<String, LazyListState>()

  /**
   * 取缓存实例；没有时用 [fallback] 播种。 [fallback] 来自 rememberSaveable，因此 Activity 重建后能带回上次位置，
   * 而主题切换时缓存里已有同一个实例，位置天然保留。
   */
  fun get(key: String, fallback: () -> LazyListState): LazyListState =
      states.getOrPut(key, fallback)
}

internal val LocalSrxScrollCache = staticCompositionLocalOf<SrxScrollCache?> { null }

/**
 * 页面级列表状态：用 rememberSaveable 保存首项与偏移。
 *
 * 之前用"主题之外的缓存对象"只能挺过组合销毁，挺不过 Activity 重建 （例如切换预测性返回开关会重建 Activity），所以仍会回到顶部。 rememberSaveable 会在
 * Activity 重建后按 key 恢复；配合外层的 SaveableStateHolder，主题切换导致的子树销毁也能还原。
 */
@Composable
internal fun srxRememberListState(key: String): LazyListState {
  // 先用 rememberSaveable 建一份（Activity 重建可恢复），
  // 再让主题之外的缓存持有同一个实例（主题切换不会丢位置）。
  val saveable = rememberSaveable(key, saver = LazyListState.Saver) { LazyListState() }
  val cache = LocalSrxScrollCache.current ?: return saveable
  return cache.get(key) { saveable }
}
