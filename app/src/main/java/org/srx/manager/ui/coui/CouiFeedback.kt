package org.srx.manager.ui.coui

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.suqi8.coui.kmp.basic.CircularProgressIndicator
import io.github.suqi8.coui.kmp.basic.InfiniteProgressIndicator
import io.github.suqi8.coui.kmp.basic.LinearProgressIndicator
import io.github.suqi8.coui.kmp.basic.Text
import io.github.suqi8.coui.kmp.theme.COUITheme
import kotlin.math.roundToInt
import org.srx.manager.RingSweep
import org.srx.manager.WaveSheen
import org.srx.manager.ui.theme.SrxShapes
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.srxAccentColor
import org.srx.manager.ui.theme.srxElevatedSurfaceColor
import org.srx.manager.ui.theme.srxPrimaryTextColor
import org.srx.manager.ui.theme.srxSurfaceColor

/** COUI 主题的忙碌遮罩：COUI 进度环/进度条 + COUI 面板配色。 */
@Composable
internal fun CouiBusyOverlay(message: String, progress: Float? = null) {
  val safeProgress = progress?.coerceIn(0f, 1f)
  val panel =
      if (isSrxDarkTheme()) srxElevatedSurfaceColor()
      else COUITheme.colorScheme.backgroundElevatedWithCard
  val animatedProgress by
      animateFloatAsState(
          targetValue = safeProgress ?: 0f,
          animationSpec = tween(durationMillis = 420, easing = FastOutSlowInEasing),
          label = "couiBusyProgress",
      )
  val shownProgress = safeProgress?.let { animatedProgress }
  Column(
      modifier =
          Modifier.widthIn(min = 220.dp, max = 320.dp)
              .clip(RoundedCornerShape(SrxShapes.card))
              .background(panel)
              .padding(horizontal = 24.dp, vertical = 22.dp),
      horizontalAlignment = Alignment.CenterHorizontally,
  ) {
    if (safeProgress == null) {
      InfiniteProgressIndicator(modifier = Modifier.size(34.dp), size = 34.dp)
    } else {
      val percentText = "${((shownProgress ?: 0f) * 100f).roundToInt().coerceIn(0, 100)}%"
      Box(modifier = Modifier.size(44.dp), contentAlignment = Alignment.Center) {
        CircularProgressIndicator(
            progress = shownProgress ?: 0f,
            modifier = Modifier.size(44.dp),
            size = 44.dp,
        )
        RingSweep(Modifier.size(44.dp), srxAccentColor(), strokeWidth = 4.dp)
        Text(
            text = percentText,
            color = srxPrimaryTextColor(),
            fontSize = 10.sp,
            fontWeight = FontWeight.Black,
        )
      }
    }
    Text(
        text = message,
        color = srxPrimaryTextColor(),
        fontSize = 15.sp,
        fontWeight = FontWeight.Bold,
        textAlign = TextAlign.Center,
    )
    if (safeProgress != null) {
      val barShape = RoundedCornerShape(3.dp)
      Box(modifier = Modifier.fillMaxWidth().height(5.dp).clip(barShape)) {
        LinearProgressIndicator(progress = shownProgress ?: 0f, modifier = Modifier.fillMaxSize())
        // 与 Miuix 侧同一套波浪：进度停住时仍持续流动。
        WaveSheen(Modifier.fillMaxSize(), srxSurfaceColor())
      }
    }
  }
}

/** COUI 主题的轻提示胶囊。 */
@Composable
internal fun CouiToastPill(text: String, modifier: Modifier = Modifier) {
  val panel =
      if (isSrxDarkTheme()) srxElevatedSurfaceColor()
      else COUITheme.colorScheme.backgroundElevatedWithCard
  Row(
      modifier =
          modifier
              .clip(CircleShape)
              .background(panel)
              .padding(horizontal = 18.dp, vertical = 11.dp),
      verticalAlignment = Alignment.CenterVertically,
  ) {
    Text(
        text = text,
        color = srxPrimaryTextColor(),
        fontWeight = FontWeight.SemiBold,
        fontSize = 13.sp,
    )
  }
}
