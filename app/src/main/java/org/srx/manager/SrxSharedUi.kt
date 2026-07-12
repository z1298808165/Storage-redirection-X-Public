package org.srx.manager

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlin.math.roundToInt
import org.srx.manager.ui.liquid.lens
import org.srx.manager.ui.liquid.vibrancy
import org.srx.manager.ui.theme.isSrxBlurEffectEnabled
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.CardDefaults
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.InfiniteProgressIndicator
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.blur.Backdrop
import top.yukonga.miuix.kmp.blur.blur
import top.yukonga.miuix.kmp.blur.drawBackdrop
import top.yukonga.miuix.kmp.blur.highlight.BloomStroke
import top.yukonga.miuix.kmp.blur.highlight.Highlight
import top.yukonga.miuix.kmp.blur.highlight.LightPosition
import top.yukonga.miuix.kmp.blur.highlight.LightSource
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Back
import top.yukonga.miuix.kmp.theme.MiuixTheme

internal val LocalSrxBackdrop = staticCompositionLocalOf<Backdrop?> { null }

@Composable
internal fun srxSuccessColor(): Color =
    if (isSrxDarkTheme()) Color(0xFF50E0B1) else Color(0xFF14B37D)

@Composable
internal fun srxWarningColor(): Color =
    if (isSrxDarkTheme()) Color(0xFFF0C45C) else Color(0xFFD48B16)

@Composable
internal fun srxDangerColor(): Color =
    if (isSrxDarkTheme()) Color(0xFFFF7A7A) else Color(0xFFD94D4D)

@Composable internal fun srxPrimaryColor(): Color = MiuixTheme.colorScheme.primary

private val RoundActionSize = 38.dp
private val RoundActionIconSize = 19.dp

private val FloatingGlassHighlight: Highlight =
    Highlight(
        width = 1.dp,
        alpha = 1f,
        style =
            BloomStroke(
                color = Color.White.copy(alpha = 0.12f),
                innerBlurRadius = 2.dp,
                primaryLight =
                    LightSource(
                        position = LightPosition(0.5f, -0.3f, -0.05f),
                        color = Color.White,
                        intensity = 1f,
                    ),
                secondaryLight =
                    LightSource(
                        position = LightPosition(0.5f, 0.8f, -0.5f),
                        color = Color.White,
                        intensity = 0.4f,
                    ),
                dualPeak = true,
            ),
    )

@Composable
internal fun isSrxGlassBackdropEnabled(): Boolean =
    isSrxLiquidGlassEnabled() && isSrxBlurEffectEnabled() && LocalSrxBackdrop.current != null

@Composable
internal fun glassSurfaceColor(alpha: Float = 0.7f): Color =
    if (!isSrxLiquidGlassEnabled()) {
      MiuixTheme.colorScheme.surfaceContainerHigh
    } else if (isSrxDarkTheme()) {
      MiuixTheme.colorScheme.surfaceContainerHigh.copy(
          alpha = (alpha * 0.38f).coerceIn(0.22f, 0.6f)
      )
    } else {
      MiuixTheme.colorScheme.surface.copy(alpha = (alpha * 0.42f).coerceIn(0.24f, 0.64f))
    }

@Composable
internal fun subtleFieldLabelColor(): Color =
    MiuixTheme.colorScheme.onSurfaceVariantSummary.copy(
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
    surfaceAlpha: Float = 0.68f,
): Modifier {
  val dark = isSrxDarkTheme()
  val glass = isSrxGlassBackdropEnabled()
  val backdrop = LocalSrxBackdrop.current
  val surface = glassSurfaceColor(surfaceAlpha)
  val accentWash = MiuixTheme.colorScheme.primary.copy(alpha = if (dark) 0.036f else 0.028f)
  val sheen = Color.White.copy(alpha = if (dark) 0.052f else 0.18f)
  val rim = Color.White.copy(alpha = if (dark) 0.08f else 0.22f)
  return this.dropShadow(
          shape = shape,
          shadow =
              Shadow(
                  radius = if (glass) if (dark) 34.dp else 48.dp else 10.dp,
                  color = if (dark) Color.Black else Color(0xFF667895),
                  alpha =
                      if (glass) if (dark) shadowAlpha * 2.15f else shadowAlpha * 1.38f
                      else shadowAlpha * 0.8f,
              ),
      )
      .then(
          if (glass && backdrop != null) {
            Modifier.clip(shape)
                .drawBackdrop(
                    backdrop = backdrop,
                    shape = { shape },
                    effects = {
                      vibrancy()
                      blur(32.dp.toPx(), 32.dp.toPx())
                      lens(
                          refractionHeight = 15.dp.toPx(),
                          refractionAmount = 16.dp.toPx(),
                          depthEffect = true,
                          chromaticAberration = 0.36f,
                      )
                    },
                    highlight = { FloatingGlassHighlight.copy(alpha = if (dark) 0.52f else 0.44f) },
                    onDrawSurface = {
                      drawRect(surface)
                      drawRect(accentWash)
                      drawRect(sheen)
                      drawRect(rim, alpha = 0.35f)
                    },
                )
          } else {
            Modifier.background(surface, shape)
          },
      )
}

@Composable
internal fun Modifier.floatingGlassPanel(shape: Shape): Modifier {
  val dark = isSrxDarkTheme()
  val glass = isSrxGlassBackdropEnabled()
  val backdrop = LocalSrxBackdrop.current
  val surface = MiuixTheme.colorScheme.surfaceContainer
  val container =
      if (glass && backdrop != null) {
        surface.copy(alpha = 0.4f)
      } else {
        MiuixTheme.colorScheme.surfaceContainerHigh
      }
  return this.dropShadow(
          shape = shape,
          shadow = Shadow(radius = 10.dp, color = Color.Black, alpha = if (dark) 0.2f else 0.1f),
      )
      .then(
          if (glass && backdrop != null) {
            Modifier.drawBackdrop(
                backdrop = backdrop,
                shape = { shape },
                effects = {
                  vibrancy()
                  blur(4.dp.toPx(), 4.dp.toPx())
                  lens(refractionHeight = 24.dp.toPx(), refractionAmount = 24.dp.toPx())
                },
                highlight = { FloatingGlassHighlight.copy(alpha = 0.75f) },
                onDrawSurface = { drawRect(container) },
            )
          } else {
            Modifier.background(container, shape)
          },
      )
}

@Composable
internal fun GlassCard(
    modifier: Modifier = Modifier,
    cornerRadius: Dp = 26.dp,
    insideMargin: PaddingValues = PaddingValues(0.dp),
    alpha: Float = 0.62f,
    shadowAlpha: Float = 0.1f,
    content: @Composable () -> Unit,
) {
  val shape = RoundedCornerShape(cornerRadius)
  Card(
      modifier = modifier.glassPanel(shape, shadowAlpha, alpha),
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
    denseSurface: Boolean = false,
    onDismiss: () -> Unit,
    content: @Composable () -> Unit,
) {
  if (!show) return
  val dark = isSrxDarkTheme()
  val liquid = isSrxGlassBackdropEnabled()
  val outsideClick = remember { MutableInteractionSource() }
  val insideClick = remember { MutableInteractionSource() }
  Dialog(
      onDismissRequest = onDismiss,
      properties = DialogProperties(usePlatformDefaultWidth = false),
  ) {
    Box(
        modifier =
            Modifier.fillMaxSize()
                .background(
                    if (dark) Color.Black.copy(alpha = 0.24f) else Color.White.copy(alpha = 0.08f)
                )
                .clickable(
                    interactionSource = outsideClick,
                    indication = null,
                    onClick = onDismiss,
                ),
        contentAlignment = Alignment.Center,
    ) {
      GlassCard(
          modifier =
              Modifier.fillMaxWidth()
                  .padding(horizontal = 28.dp)
                  .widthIn(max = 360.dp)
                  .then(
                      if (denseSurface) {
                        Modifier.background(
                            MiuixTheme.colorScheme.surfaceContainerHigh.copy(
                                alpha = if (dark) 0.82f else 0.76f
                            ),
                            RoundedCornerShape(30.dp),
                        )
                      } else {
                        Modifier
                      }
                  )
                  .clickable(
                      interactionSource = insideClick,
                      indication = null,
                  ) {},
          cornerRadius = 30.dp,
          insideMargin = PaddingValues(24.dp),
          alpha = if (liquid) if (denseSurface) 0.94f else 0.78f else 1f,
          shadowAlpha = 0.2f,
      ) {
        Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
          if (title != null) {
            Text(title, fontWeight = FontWeight.Black, fontSize = 17.sp, lineHeight = 21.sp)
          }
          if (summary != null) {
            Text(
                summary,
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
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
  val color = if (danger) MiuixTheme.colorScheme.error else MiuixTheme.colorScheme.primary
  val liquid = isSrxLiquidGlassEnabled()
  Box(
      modifier =
          modifier
              .size(size)
              .dropShadow(
                  CircleShape,
                  Shadow(
                      radius = if (liquid) 14.dp else 8.dp,
                      color = if (isSrxDarkTheme()) Color.Black else MiuixTheme.colorScheme.primary,
                      alpha = if (liquid) 0.08f else 0.05f,
                  ),
              )
              .clip(CircleShape)
              .background(
                  if (liquid) glassSurfaceColor(0.76f)
                  else color.copy(alpha = if (danger) 0.1f else 0.08f),
                  CircleShape,
              )
              .alpha(if (enabled) 1f else 0.45f)
              .clickable(
                  enabled = enabled,
                  interactionSource = null,
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

@Composable
internal fun GlassTextButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    primary: Boolean = false,
    danger: Boolean = false,
) {
  val tint =
      when {
        danger -> MiuixTheme.colorScheme.error
        primary -> srxPrimaryColor()
        else -> MiuixTheme.colorScheme.onSurface
      }
  TextButton(
      text = text,
      onClick = onClick,
      modifier =
          modifier
              .height(44.dp)
              .clip(CircleShape)
              .background(
                  when {
                    danger ->
                        MiuixTheme.colorScheme.error.copy(
                            alpha = if (isSrxLiquidGlassEnabled()) 0.14f else 0.08f
                        )
                    primary -> tint.copy(alpha = if (isSrxLiquidGlassEnabled()) 0.18f else 0.12f)
                    else -> glassSurfaceColor(if (isSrxLiquidGlassEnabled()) 0.82f else 1f)
                  },
                  CircleShape,
              ),
      cornerRadius = 22.dp,
      minWidth = 0.dp,
      minHeight = 44.dp,
      insideMargin = PaddingValues(0.dp),
      colors =
          if (primary) ButtonDefaults.textButtonColorsPrimary()
          else ButtonDefaults.textButtonColors(),
  )
}

@Composable
internal fun PageHeader(
    title: String,
    modifier: Modifier = Modifier,
    trailing: String? = null,
    actions: @Composable (() -> Unit)? = null,
) {
  Row(
      modifier = modifier.fillMaxWidth(),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(12.dp),
  ) {
    Text(
        text = title,
        fontSize = 24.sp,
        lineHeight = 29.sp,
        fontWeight = FontWeight.Black,
        modifier = Modifier.weight(1f),
    )
    if (trailing != null) {
      Text(
          text = trailing,
          color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
          fontSize = 12.sp,
          fontWeight = FontWeight.SemiBold,
          maxLines = 1,
          overflow = TextOverflow.Ellipsis,
      )
    }
    actions?.invoke()
  }
}

@Composable
internal fun BackPageHeader(
    title: String,
    onBack: () -> Unit,
    modifier: Modifier = Modifier,
    actions: @Composable (() -> Unit)? = null,
) {
  Row(
      modifier = modifier.fillMaxWidth(),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(12.dp),
  ) {
    RoundIconAction(MiuixIcons.Back, "返回", onBack)
    Text(
        text = title,
        fontSize = 24.sp,
        lineHeight = 29.sp,
        fontWeight = FontWeight.Black,
        modifier = Modifier.weight(1f),
    )
    actions?.invoke()
  }
}

@Composable
internal fun SectionTitle(text: String) {
  Text(
      text = text,
      modifier = Modifier.padding(start = 6.dp, bottom = 8.dp),
      color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
      fontSize = 12.sp,
      fontWeight = FontWeight.Bold,
  )
}

@Composable
internal fun EmptyText(text: String) {
  Box(
      modifier = Modifier.fillMaxWidth().padding(28.dp),
      contentAlignment = Alignment.Center,
  ) {
    Text(text, color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
  }
}

@Composable
internal fun busyScrimColor(): Color =
    if (isSrxDarkTheme()) {
      Color.Black.copy(alpha = 0.62f)
    } else {
      Color(0xFFEAF0FA).copy(alpha = 0.66f)
    }

@Composable
internal fun BusyOverlay(message: String, progress: Float? = null) {
  val shape = RoundedCornerShape(24.dp)
  val safeProgress = progress?.coerceIn(0f, 1f)
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
              .background(glassSurfaceColor(0.9f), shape)
              .padding(horizontal = 24.dp, vertical = 22.dp),
      horizontalAlignment = Alignment.CenterHorizontally,
      verticalArrangement = Arrangement.spacedBy(14.dp),
  ) {
    if (safeProgress == null) {
      InfiniteProgressIndicator(modifier = Modifier.size(34.dp))
    } else {
      DeterminateProgressRing(safeProgress)
    }
    Text(
        text = message,
        color = MiuixTheme.colorScheme.onSurface,
        fontSize = 15.sp,
        fontWeight = FontWeight.Bold,
        textAlign = TextAlign.Center,
    )
    if (safeProgress != null) {
      Box(
          modifier =
              Modifier.fillMaxWidth()
                  .height(5.dp)
                  .clip(CircleShape)
                  .background(capsuleContainerColor()),
      ) {
        Box(
            modifier =
                Modifier.fillMaxWidth(safeProgress.coerceAtLeast(0.02f))
                    .height(5.dp)
                    .clip(CircleShape)
                    .background(srxPrimaryColor(), CircleShape),
        )
      }
    }
  }
}

@Composable
private fun DeterminateProgressRing(progress: Float) {
  val percentText = "${(progress * 100f).roundToInt().coerceIn(0, 100)}%"
  val primary = srxPrimaryColor()
  val track = capsuleContainerColor()
  Box(
      modifier = Modifier.size(44.dp),
      contentAlignment = Alignment.Center,
  ) {
    Canvas(modifier = Modifier.size(44.dp)) {
      val strokeWidth = 4.dp.toPx()
      val inset = strokeWidth / 2f
      val arcSize = size.copy(width = size.width - strokeWidth, height = size.height - strokeWidth)
      drawArc(
          color = track,
          startAngle = -90f,
          sweepAngle = 360f,
          useCenter = false,
          topLeft = Offset(inset, inset),
          size = arcSize,
          style = Stroke(width = strokeWidth, cap = StrokeCap.Round),
      )
      drawArc(
          color = primary,
          startAngle = -90f,
          sweepAngle = 360f * progress.coerceIn(0f, 1f),
          useCenter = false,
          topLeft = Offset(inset, inset),
          size = arcSize,
          style = Stroke(width = strokeWidth, cap = StrokeCap.Round),
      )
    }
    Text(
        text = percentText,
        color = MiuixTheme.colorScheme.onSurface,
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
              .glassPanel(shape, shadowAlpha = 0.12f, surfaceAlpha = 0.84f)
              .clip(shape)
              .padding(horizontal = 18.dp, vertical = 11.dp),
      color = MiuixTheme.colorScheme.onSurface,
      fontWeight = FontWeight.SemiBold,
      fontSize = 13.sp,
  )
}

@Composable
internal fun Modifier.appMeshBackground(): Modifier {
  val dark = isSrxDarkTheme()
  val colors = MiuixTheme.colorScheme
  val primary = colors.primary
  val success = srxSuccessColor()
  val base = colors.surface
  val elevated = colors.surfaceContainerHigh
  return drawBehind {
    val w = size.width.coerceAtLeast(1f)
    val h = size.height.coerceAtLeast(1f)
    drawRect(base)
    drawRect(
        Brush.linearGradient(
            listOf(
                elevated.copy(alpha = if (dark) 0.32f else 0.72f),
                base.copy(alpha = 0.18f),
                elevated.copy(alpha = if (dark) 0.22f else 0.58f),
            ),
            start = Offset.Zero,
            end = Offset(w, h),
        ),
    )
    drawRect(
        Brush.linearGradient(
            listOf(
                primary.copy(alpha = if (dark) 0.22f else 0.16f),
                Color.Transparent,
                success.copy(alpha = if (dark) 0.12f else 0.10f),
            ),
            start = Offset(0f, 0f),
            end = Offset(w, h),
        ),
    )
    drawRect(
        Brush.linearGradient(
            listOf(
                Color.Transparent,
                primary.copy(alpha = if (dark) 0.035f else 0.024f),
                Color.Transparent,
            ),
            start = Offset(w * 0.92f, 0f),
            end = Offset(w * 0.08f, h * 0.82f),
        ),
    )
  }
}

@Composable
internal fun AppBackground(): Brush {
  val dark = isSrxDarkTheme()
  return if (dark) {
    Brush.linearGradient(listOf(Color(0xFF05070B), Color(0xFF101827), Color(0xFF071B1A)))
  } else {
    Brush.linearGradient(listOf(Color(0xFFEDF2F7), Color(0xFFEAF4FF), Color(0xFFEAFBF7)))
  }
}
