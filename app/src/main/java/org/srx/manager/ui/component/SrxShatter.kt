package org.srx.manager.ui.component

import android.view.HapticFeedbackConstants
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import org.srx.manager.CenteredDialog
import org.srx.manager.GlassTextButton
import org.srx.manager.srxSuccessColor
import org.srx.manager.srxWarningColor
import org.srx.manager.ui.theme.LocalSrxColorScheme

/**
 * 删除时的「破碎消失」动效。
 *
 * 只做一次单向动画：行先轻微收缩，再炸成碎片并淡出，动画结束回调 [onFinished] 才真正把数据删掉。 碎片的位置和飞散方向全部由下标决定（不用随机数），因此同一次动画每帧都是确定性的，
 * 不会因为重组抖动；也不需要任何外部状态机。
 *
 * [shattering] 由调用方按「要删的那一条」置位，调用方在 [onFinished] 里清除它。
 */
@Composable
internal fun ShatterRow(
    shattering: Boolean,
    onFinished: () -> Unit,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
  val progress = remember { Animatable(1f) }
  // 动画跑完到数据真正从列表里消失之间会隔几帧（删除走的是 ViewModel/StateFlow）。
  // 这几帧里 shattering 已经被置回 false，如果把进度 snap 回 1，内容就会"重新出现再消失"。
  // 所以完成过一次之后保持 0，直到这一行被移出组合（换了 key）才重置。
  var finished by remember { mutableStateOf(false) }
  val view = LocalView.current
  LaunchedEffect(shattering) {
    if (!shattering) {
      if (!finished) progress.snapTo(1f)
      return@LaunchedEffect
    }
    finished = false
    // 起爆立刻给一记"确认"触感，碎片落地时补一记轻的，和动画节奏对齐。
    // 走 View.performHapticFeedback 而不是自建 Vibrator：不需要 VIBRATE 权限，
    // 也自动遵守系统「触感反馈」开关。
    view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
    val settle = launch {
      delay(HapticSettleDelayMs)
      view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
    }
    try {
      progress.animateTo(
          targetValue = 0f,
          animationSpec = tween(durationMillis = ShatterDurationMs, easing = LinearEasing),
      )
    } finally {
      // 正常跑完和被打断都要保持隐藏；否则 onFinished 置回 shattering=false 后，
      // 这几帧会把进度 snap 回 1，删掉的行又闪回来。
      finished = true
      settle.cancel()
      // 动画被取消（用户在这段时间里返回或切页）时也要把删除落下去：
      // 否则界面回来看起来像"点了删除但没删"，比动画不完整严重得多。
      onFinished()
    }
  }
  val p = progress.value
  Box(modifier = modifier.fillMaxWidth()) {
    // 内容层单独做透明度/缩放：碎片必须画在它的外面，否则会跟着一起被淡掉。
    Box(
        modifier =
            Modifier.fillMaxWidth().graphicsLayer {
              alpha = if (p > 0.6f) ((p - 0.6f) / 0.4f).coerceIn(0f, 1f) else 0f
              val pop = 1f + 0.03f * (1f - p)
              scaleX = pop
              scaleY = pop
            },
    ) {
      content()
    }
    if (p < 0.999f) {
      // 碎片层不参与上面的 alpha，且放在最上层，保证"炸开"能看见。
      ShatterShards(progress = p, modifier = Modifier.matchParentSize().zIndex(2f))
    }
  }
}

private const val ShatterDurationMs = 820

/** 碎片落地的第二次触感延时，取动画的约 45%。 */
private const val HapticSettleDelayMs = 320L

/**
 * 爆炸粒子层。
 *
 * 圆形颗粒 + 拖尾 + 柔光 + 减速曲线：距离用 easeOut（先快后慢）而不是线性位移， 更接近真实爆散；每颗粒子后面跟一小段沿速度方向的圆头条带，模拟运动模糊，
 * 再叠一层更大更淡的圆做光晕。参数全部由下标经线性同余推导，不用 `Random()`， 每帧结果一致、重组不会跳。
 */
@Composable
private fun ShatterShards(progress: Float, modifier: Modifier = Modifier) {
  // 粒子要「五颜六色」但不能脏：只用主题里的强调色系（强调/成功/警告/错误），
  // 每个再派生一档浅色，避免出现深灰或黑色颗粒——之前 75% 用的是 onSurface，
  // 整体看起来就是一堆黑点。
  val accents =
      listOf(
          LocalSrxColorScheme.current.primary,
          srxSuccessColor(),
          srxWarningColor(),
          LocalSrxColorScheme.current.error,
      )
  val palette = buildList {
    accents.forEach { color ->
      add(color)
      add(lerp(color, Color.White, 0.45f))
    }
  }
  val accent = LocalSrxColorScheme.current.primary
  Canvas(modifier) {
    val t = (1f - progress).coerceIn(0f, 1f)
    if (t <= 0f) return@Canvas
    val w = size.width
    val h = size.height
    val cx = w / 2f
    val cy = h / 2f
    val spreadX = w * 0.95f
    val spreadY = h * 1.10f
    val gravity = h * 2.35f

    // 起爆双环：内环实一点、外环更淡更大，比单环更像冲击波。
    if (t < 0.32f) {
      val ringT = t / 0.32f
      val fade = (1f - ringT) * (1f - ringT)
      drawCircle(
          color = accent.copy(alpha = fade * 0.18f),
          radius = h * 0.30f + w * 0.55f * ringT,
          center = Offset(cx, cy),
          style = androidx.compose.ui.graphics.drawscope.Stroke(width = 1.6.dp.toPx()),
      )
      drawCircle(
          color = accent.copy(alpha = fade * 0.08f),
          radius = h * 0.44f + w * 0.78f * ringT,
          center = Offset(cx, cy),
          style = androidx.compose.ui.graphics.drawscope.Stroke(width = 1.dp.toPx()),
      )
    }

    repeat(ParticleCount) { index ->
      val r1 = pseudoRandom(index * 5 + 1)
      val r2 = pseudoRandom(index * 5 + 2)
      val r3 = pseudoRandom(index * 5 + 3)
      val r4 = pseudoRandom(index * 5 + 4)
      val r5 = pseudoRandom(index * 5 + 5)

      val startX = w * (0.04f + 0.92f * r1)
      val startY = h * (0.20f + 0.60f * r2)
      val dirX = (startX - cx) / maxOf(cx, 1f)
      val dirY = (startY - cy) / maxOf(cy, 1f)

      val speedX = (dirX * (0.50f + 0.70f * r3) + (r4 - 0.5f) * 0.45f) * spreadX
      // 竖直初速统一向上（负值），叠上一点方向差，才能出弧线。
      val speedY = (dirY * 0.25f - (0.55f + 0.85f * r3)) * spreadY

      // 抛物线：水平匀速、竖直匀加速（先向上抛、再被重力拉下来），
      // 不再用同一个 ease 因子走两轴——那样看起来是直线飘散而不是烟花。
      // 逐个错峰 0~8% 起爆：同一时刻只有一部分粒子在飞，像烟花从中心依次绽开，
      // 而不是所有点同步平移（同步平移看着像整块图层在飘）。
      val localT = ((t - 0.08f * r1) / (1f - 0.08f * r1)).coerceIn(0f, 1f)
      val x = startX + speedX * localT
      val y = startY + speedY * localT + gravity * localT * localT

      // 大小分层：少量大颗粒带出碎屑感，大量小颗粒提供密度。
      val big = r5 > 0.72f
      val radius =
          if (big) (3.2.dp.toPx() + 3.4.dp.toPx() * r4) else (1.1.dp.toPx() + 1.7.dp.toPx() * r4)
      val shrink = 1f - 0.22f * localT
      // 前 60% 保持不透明，之后才收尾。
      val alpha = if (localT < 0.60f) 1f else ((1f - localT) / 0.40f).coerceIn(0f, 1f)

      val base = palette[index % palette.size]
      // 同色系内再抖一点明度，避免同色颗粒看起来是复制粘贴。
      val color = lerp(base, if (r5 > 0.5f) Color.White else Color.Black, 0.10f * r4)

      drawCircle(
          color = color.copy(alpha = alpha * 0.12f),
          radius = radius * 2.4f * shrink,
          center = Offset(x, y),
      )
      drawCircle(
          color = color.copy(alpha = alpha * 0.95f),
          radius = radius * shrink,
          center = Offset(x, y),
      )
      // 高光只用白色：浅色主题下用黑色会在颗粒上留一个脏点，看起来像阴影。
      if (big) {
        drawCircle(
            color = Color.White.copy(alpha = alpha * 0.30f),
            radius = radius * 0.40f,
            center = Offset(x - radius * 0.26f, y - radius * 0.28f),
        )
      }
    }
  }
}

/** 确定性伪随机：同一个下标永远得到同一个值，避免每帧抖动。 */
private fun pseudoRandom(seed: Int): Float {
  var x = seed * 1664525 + 1013904223
  x = x xor (x ushr 13)
  x *= 1274126177
  return ((x ushr 8) and 0xFFFF) / 65535f
}

private const val ParticleCount = 72

/**
 * 删除前的确认弹窗。
 *
 * 规则、路径映射、模板这类「点一下就没了」的条目必须二次确认：它们多半是手工敲的 绝对路径，误删后只能凭记忆重写。文案统一带上被删内容，避免确认框里看不出删哪条。
 */
@Composable
internal fun ConfirmDeleteDialog(
    show: Boolean,
    title: String,
    subject: String,
    confirmText: String = "删除",
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
) {
  CenteredDialog(
      show = show,
      title = title,
      summary = "将删除：$subject",
      onDismiss = onDismiss,
  ) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
      Box(Modifier.weight(1f)) {
        GlassTextButton("取消", onClick = onDismiss, modifier = Modifier.fillMaxWidth())
      }
      Box(Modifier.weight(1f)) {
        GlassTextButton(
            confirmText,
            onClick = onConfirm,
            modifier = Modifier.fillMaxWidth(),
            danger = true,
        )
      }
    }
  }
}
