package org.srx.manager.ui.coui

import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import io.github.suqi8.coui.kmp.basic.Scaffold as CouiScaffold
import top.yukonga.miuix.kmp.basic.Scaffold as MiuixScaffold

/**
 * 根 Scaffold 按主题分派。COUI 主题必须用 COUI Scaffold，否则 COUI 的 OverlayDialog 找不到根 dialog host，会完全不显示（官方
 * pitfalls 明确此约束）。
 *
 * 页面级不再套第二层 Scaffold：根这里已经提供了 COUI 的 popup host 与底栏占位， 内层再套一层会让 insets 与浮层宿主重复。
 */
@Composable
internal fun SrxRootScaffold(
    useCoui: Boolean,
    bottomBar: @Composable () -> Unit,
    content: @Composable (PaddingValues) -> Unit,
) {
  if (useCoui) {
    CouiScaffold(
        containerColor = Color.Transparent,
        bottomBar = bottomBar,
    ) { padding ->
      content(padding)
    }
  } else {
    MiuixScaffold(
        containerColor = Color.Transparent,
        bottomBar = bottomBar,
        content = content,
    )
  }
}
