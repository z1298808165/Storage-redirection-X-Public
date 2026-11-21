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
import androidx.compose.ui.draw.dropShadow
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
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalDensity
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
import org.srx.manager.ui.coui.couiDialogSurface
import org.srx.manager.ui.coui.couiPanelEdge
import org.srx.manager.ui.coui.couiPanelSurface
import org.srx.manager.ui.liquid.lens
import org.srx.manager.ui.liquid.vibrancy
import org.srx.manager.ui.theme.LocalSrxColorScheme
import org.srx.manager.ui.theme.SrxRowMetrics
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
import top.yukonga.miuix.kmp.blur.blur
import top.yukonga.miuix.kmp.blur.drawBackdrop
import top.yukonga.miuix.kmp.blur.layerBackdrop
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
  val backdrop = LocalSrxBackdrop.current
  val hasBackdrop = backdrop != null && (liquid || blurEnabled)
  // 卡片/弹窗表面统一用官方 surface：不再按材质开关在 surfaceContainer 上叠 alpha。
  // 半透明 + 背后渐变会让浅色卡片看起来"被染色"，与 KernelSU 管理器的原生观感不同。
  // COUI 下必须取 COUI 自己的卡片色：LocalSrxColorScheme.surface 是「页面底」那一层，
  // 用它铺列表/大卡就等于把页面灰又涂了一遍，和白色卡片不是同一个色。
  val surface = if (isSrxThemeCoui()) couiPanelSurface() else LocalSrxColorScheme.current.surface
  val sheen = Color.White.copy(alpha = if (dark) 0.052f else 0.12f)
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
                            alpha = if (dark) 0.52f else 0.44f,
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
    cornerRadius: Dp = 16.dp,
    insideMargin: PaddingValues = PaddingValues(0.dp),
    shadowAlpha: Float = 0.1f,
    content: @Composable () -> Unit,
) {
  if (isSrxThemeCoui()) {
    // Eta 的 COUI 卡片直接使用 COUI 官方 Card（CardDefaults.defaultColors(color = card)），
    // 材质（色调、描边、模糊）由 COUI 库自己绘制；手绘实心底会丢掉这层质感。
    io.github.suqi8.coui.kmp.basic.Card(
        modifier =
            modifier
                .fillMaxWidth()
                .then(
                    // shadowAlpha 在 COUI 下映射为「边缘光强度」：卡片是扁平的，
                    // 用 contour 表达层次，不画投影（见下方 shadow = null 的说明）。
                    if (shadowAlpha > 0f) Modifier.couiPanelEdge(cornerRadius = cornerRadius)
                    else Modifier
                ),
        cornerRadius = cornerRadius,
        insideMargin = insideMargin,
        colors =
            io.github.suqi8.coui.kmp.basic.CardDefaults.defaultColors(color = couiPanelSurface()),
        // COUI 的卡片是扁平材质，层次靠「边缘光」而不是投影：官方 CardShadow 在
        // 隐藏单边时用矩形 clipRect 裁剪，会在圆角处切出灰黑直角边；四边都画又会
        // 变成一圈硬边灰框。所以这里不传 shadow，改为 shadowAlpha > 0 时叠加 contour。
        shadow = null,
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
  val shape = RoundedCornerShape(if (couiStyle) 24.dp else 30.dp)
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
    CompositionLocalProvider(LocalSrxBackdrop provides backdrop) {
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
              cornerRadius = if (couiStyle) 24.dp else 30.dp,
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
                          // COUI 的弹窗面板要取「页面底」那一层（backgroundWithCard），
                          // 内部分组卡片才是 cardBackground（纯白）。两者都用 cardBackground
                          // 时卡片在白底上等于隐形，观感就变成"弹窗没有分区、没有卡片"。
                          if (couiStyle) couiDialogSurface()
                          else LocalSrxColorScheme.current.surface
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
  val interactionSource = remember { MutableInteractionSource() }
  Box(
      modifier =
          Modifier.size(size)
              .dropShadow(
                  CircleShape,
                  Shadow(
                      radius = 8.dp,
                      color =
                          if (isSrxDarkTheme()) Color.Black
                          else LocalSrxColorScheme.current.primary,
                      alpha = 0.05f,
                  ),
              )
              .liquidPressScale(interactionSource)
              // 纯实心底：圆钮不再叠玻璃层。玻璃层无论怎么压 alpha 都带半透，
              // 在弹窗里会透出下面的内容（用户反馈"按钮像透明的"）。
              .background(
                  if (danger) LocalSrxColorScheme.current.error.copy(alpha = 0.14f)
                  else color.copy(alpha = 0.14f),
                  CircleShape,
              )
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
        tint = color,
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
              .background(
                  when {
                    danger -> tint.copy(alpha = if (enabled) 0.12f else 0.06f)
                    primary -> tint.copy(alpha = if (enabled) 0.14f else 0.06f)
                    // 次要按钮不描边：COUI 的这套按钮是纯填充（container12），
                    // 之前那圈 1dp 描边在两个主题下都多出一条边。
                    else ->
                        if (isSrxThemeCoui())
                            io.github.suqi8.coui.kmp.theme.COUITheme.colorScheme.container12
                        else srxColorScheme.surfaceContainerHigh
                  },
                  CircleShape,
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
 * OS4 顶栏：直接复用 Miuix GlassTopAppBar 的 progressiveBlur。 显式把 collapsedFraction 映射为
 * visualContentOffset，确保滚动折叠时模糊带按同一进度渐入； 透传每页内容层 backdrop，并用透明 bandBrush 避免独立 surface 色块。
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
  val rampDistancePx = with(LocalDensity.current) { 48.dp.toPx() }
  if (backdrop != null) {
    GlassTopAppBar(
        title = title,
        subtitle = subtitle,
        backdrop = backdrop,
        progressiveBlur = true,
        fill = Color.Transparent,
        bandBrush = Brush.verticalGradient(listOf(Color.Transparent, Color.Transparent)),
        backgroundAlpha = { 1f },
        alpha = 0f,
        stroke = null,
        buttonShadow = null,
        scrollBehavior = scrollBehavior,
        visualContentOffset = { -scrollBehavior.state.collapsedFraction * rampDistancePx },
        navigationIcon = navigationIcon,
        actions = actions,
        bottomContent = bottomContent,
    )
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
  val shape = RoundedCornerShape(24.dp)
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
