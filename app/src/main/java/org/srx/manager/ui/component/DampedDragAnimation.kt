package org.srx.manager.ui.component

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.spring
import androidx.compose.foundation.MutatorMutex
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.unit.IntSize
import kotlin.math.abs
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.android.awaitFrame
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch

class DampedDragAnimation(
    private val animationScope: CoroutineScope,
    initialValue: Float,
    private val valueRange: ClosedRange<Float>,
    visibilityThreshold: Float,
    private val initialScale: Float,
    private val pressedScale: Float,
    private val canDrag: (Offset) -> Boolean = { true },
    private val onDragStarted: DampedDragAnimation.(Offset) -> Unit,
    private val onDragStopped: DampedDragAnimation.() -> Unit,
    private val onDrag: DampedDragAnimation.(IntSize, Offset) -> Unit,
) {
  private val valueSpec = spring(1f, 1000f, visibilityThreshold)
  private val velocitySpec = spring(0.5f, 300f, visibilityThreshold * 10f)
  private val pressSpec = spring(1f, 1000f, 0.001f)
  private val scaleXSpec = spring(0.6f, 250f, 0.001f)
  private val scaleYSpec = spring(0.7f, 250f, 0.001f)

  private val valueAnim = Animatable(initialValue, visibilityThreshold)
  private val velocityAnim = Animatable(0f, 5f)
  private val pressAnim = Animatable(0f, 0.001f)
  private val scaleXAnim = Animatable(initialScale, 0.001f)
  private val scaleYAnim = Animatable(initialScale, 0.001f)
  private val mutex = MutatorMutex()
  private val velocityTracker = VelocityTracker()

  val value: Float
    get() = valueAnim.value

  val targetValue: Float
    get() = valueAnim.targetValue

  val pressProgress: Float
    get() = pressAnim.value

  val scaleX: Float
    get() = scaleXAnim.value

  val scaleY: Float
    get() = scaleYAnim.value

  val velocity: Float
    get() = velocityAnim.value

  val modifier: Modifier =
      Modifier.pointerInput(Unit) {
        inspectDragGestures(
            onDragStart = { down ->
              onDragStarted(down.position)
              press()
            },
            onDragEnd = {
              onDragStopped()
              release()
            },
            onDragCancel = {
              onDragStopped()
              release()
            },
        ) { change, dragAmount ->
          if (canDrag(change.position) && canDrag(change.previousPosition)) onDrag(size, dragAmount)
        }
      }

  fun press() {
    velocityTracker.resetTracking()
    animationScope.launch {
      launch { pressAnim.animateTo(1f, pressSpec) }
      launch { scaleXAnim.animateTo(pressedScale, scaleXSpec) }
      launch { scaleYAnim.animateTo(pressedScale, scaleYSpec) }
    }
  }

  fun release() {
    animationScope.launch {
      awaitFrame()
      if (value != targetValue) {
        val threshold = (valueRange.endInclusive - valueRange.start) * 0.025f
        snapshotFlow { valueAnim.value }
            .filter { abs(it - valueAnim.targetValue) < threshold }
            .first()
      }
      launch { pressAnim.animateTo(0f, pressSpec) }
      launch { scaleXAnim.animateTo(initialScale, scaleXSpec) }
      launch { scaleYAnim.animateTo(initialScale, scaleYSpec) }
    }
  }

  fun updateValue(value: Float) {
    val target = value.coerceIn(valueRange)
    animationScope.launch { valueAnim.animateTo(target, valueSpec) { updateVelocity() } }
  }

  fun animateToValue(value: Float) {
    animationScope.launch {
      mutex.mutate {
        press()
        launch { valueAnim.animateTo(value.coerceIn(valueRange), valueSpec) }
        if (velocity != 0f) launch { velocityAnim.animateTo(0f, velocitySpec) }
        release()
      }
    }
  }

  private fun updateVelocity() {
    velocityTracker.addPosition(System.currentTimeMillis(), Offset(value, 0f))
    val targetVelocity =
        velocityTracker.calculateVelocity().x / (valueRange.endInclusive - valueRange.start)
    animationScope.launch { velocityAnim.animateTo(targetVelocity, velocitySpec) }
  }
}
