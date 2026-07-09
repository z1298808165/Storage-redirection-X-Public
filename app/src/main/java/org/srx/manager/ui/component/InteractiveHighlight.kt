package org.srx.manager.ui.component

import android.annotation.SuppressLint
import android.graphics.RuntimeShader
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.VectorConverter
import androidx.compose.animation.core.VisibilityThreshold
import androidx.compose.animation.core.spring
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ShaderBrush
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.util.fastCoerceIn
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.launch

@SuppressLint("NewApi")
class InteractiveHighlight(
    private val animationScope: CoroutineScope,
    private val position: (size: Size, offset: Offset) -> Offset = { _, offset -> offset },
) {
  private val pressSpec = spring(0.5f, 300f, 0.001f)
  private val positionSpec = spring(0.5f, 300f, Offset.VisibilityThreshold)
  private val pressAnim = Animatable(0f, 0.001f)
  private val positionAnim =
      Animatable(Offset.Zero, Offset.VectorConverter, Offset.VisibilityThreshold)
  private var startPosition = Offset.Zero

  private val shader =
      RuntimeShader(
          """
          uniform float2 size;
          layout(color) uniform half4 color;
          uniform float radius;
          uniform float2 position;
          half4 main(float2 coord) {
              float dist = distance(coord, position);
              float intensity = smoothstep(radius, radius * 0.5, dist);
              return color * intensity;
          }
          """
              .trimIndent(),
      )

  val modifier: Modifier =
      Modifier.drawWithContent {
        val progress = pressAnim.value
        if (progress > 0f) {
          drawRect(Color.White.copy(alpha = 0.06f * progress), blendMode = BlendMode.Plus)
          shader.apply {
            val p = position(size, positionAnim.value)
            setFloatUniform("size", size.width, size.height)
            setColorUniform("color", Color.White.copy(alpha = 0.12f * progress).toArgb())
            setFloatUniform("radius", size.minDimension * 1.2f)
            setFloatUniform(
                "position",
                p.x.fastCoerceIn(0f, size.width),
                p.y.fastCoerceIn(0f, size.height),
            )
          }
          drawRect(ShaderBrush(shader), blendMode = BlendMode.Plus)
        }
        drawContent()
      }

  val gestureModifier: Modifier =
      Modifier.pointerInput(animationScope) {
        inspectDragGestures(
            onDragStart = { down ->
              startPosition = down.position
              animationScope.launch {
                launch { pressAnim.animateTo(1f, pressSpec) }
                launch { positionAnim.snapTo(startPosition) }
              }
            },
            onDragEnd = {
              animationScope.launch {
                launch { pressAnim.animateTo(0f, pressSpec) }
                launch { positionAnim.animateTo(startPosition, positionSpec) }
              }
            },
            onDragCancel = {
              animationScope.launch {
                launch { pressAnim.animateTo(0f, pressSpec) }
                launch { positionAnim.animateTo(startPosition, positionSpec) }
              }
            },
        ) { change, _ ->
          animationScope.launch { positionAnim.snapTo(change.position) }
        }
      }
}
