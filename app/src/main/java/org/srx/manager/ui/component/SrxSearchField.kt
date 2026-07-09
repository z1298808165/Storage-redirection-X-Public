package org.srx.manager.ui.component

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.basic.Search
import top.yukonga.miuix.kmp.icon.basic.SearchCleanup
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
fun SrxSearchField(
    query: String,
    onQueryChange: (String) -> Unit,
    label: String,
    modifier: Modifier = Modifier,
) {
  val colors = MiuixTheme.colorScheme
  val dark = isSrxDarkTheme()
  val liquid = isSrxLiquidGlassEnabled()
  val backgroundColor =
      if (!liquid) {
        colors.surfaceContainerHigh
      } else if (dark) {
        colors.surfaceContainerHigh.copy(alpha = 0.74f)
      } else {
        colors.surface.copy(alpha = 0.84f)
      }
  BasicTextField(
      value = query,
      onValueChange = onQueryChange,
      singleLine = true,
      textStyle =
          TextStyle(
              fontWeight = FontWeight.Medium,
              fontSize = 14.sp,
              color = colors.onSurface,
          ),
      cursorBrush = SolidColor(colors.primary),
      keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
      modifier =
          modifier
              .fillMaxWidth()
              .heightIn(min = 50.dp)
              .background(
                  backgroundColor,
                  CircleShape,
              ),
      decorationBox = { inner ->
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
        ) {
          Icon(
              imageVector = MiuixIcons.Basic.Search,
              contentDescription = null,
              modifier = Modifier.size(42.dp).padding(start = 16.dp, end = 8.dp),
              tint = colors.onSurfaceVariantSummary,
          )
          Box(modifier = Modifier.weight(1f)) {
            if (query.isBlank()) {
              Text(text = label, color = colors.onSurfaceVariantSummary, fontSize = 14.sp)
            }
            inner()
          }
          AnimatedVisibility(
              visible = query.isNotEmpty(),
              enter = fadeIn() + scaleIn(),
              exit = fadeOut() + scaleOut(),
          ) {
            Icon(
                imageVector = MiuixIcons.Basic.SearchCleanup,
                contentDescription = null,
                modifier =
                    Modifier.size(42.dp).padding(start = 8.dp, end = 16.dp).clickable(
                        interactionSource = null,
                        indication = null,
                    ) {
                      onQueryChange("")
                    },
                tint = colors.onSurface,
            )
          }
        }
      },
  )
}
