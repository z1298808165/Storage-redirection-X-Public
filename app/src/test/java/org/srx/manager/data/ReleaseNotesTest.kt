package org.srx.manager.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class ReleaseNotesTest {
  @Test
  fun parsesNonEmptyComponentSectionsAndDropsCommitList() {
    val sections =
        parseReleaseNoteSections(
            """
            ## Release 更新日志

            ## 模块更新
            - 修复 **模块** 行为。

            ## App 更新
            - 增加设置入口。

            ### 提交列表
            - `abc1234` 内部提交
            """
                .trimIndent()
        )

    assertEquals(
        listOf(ReleaseNoteComponent.Module, ReleaseNoteComponent.App),
        sections.map { it.component },
    )
    assertFalse(sections.joinToString { it.markdown }.contains("提交列表"))
  }

  @Test
  fun legacyNotesFallBackToOtherSection() {
    val sections = parseReleaseNoteSections("### 修复了什么问题\n- 修复旧版日志。")

    assertEquals(1, sections.size)
    assertEquals(ReleaseNoteComponent.Other, sections.single().component)
  }
}
