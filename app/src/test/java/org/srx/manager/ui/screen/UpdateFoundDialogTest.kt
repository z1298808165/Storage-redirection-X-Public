package org.srx.manager.ui.screen

import org.junit.Assert.assertEquals
import org.junit.Test
import org.srx.manager.data.ComponentUpdate
import org.srx.manager.data.ReleaseNoteComponent
import org.srx.manager.data.ReleaseUpdate
import org.srx.manager.data.UpdateAvailability
import org.srx.manager.data.UpdateChannel
import org.srx.manager.data.UpdateComponent
import org.srx.manager.data.parseReleaseNoteSections

class UpdateFoundDialogTest {
  private val notes =
      """
      ## 模块更新
      - 修复模块。

      ## App 更新
      - 修复 App。

      ## 其它更新
      - 其它说明。
      """
          .trimIndent()

  private fun availability(app: Boolean, module: Boolean): UpdateAvailability =
      UpdateAvailability(
          release =
              ReleaseUpdate(
                  tagName = "ci-build-2",
                  versionName = "1.2.56-ci.2",
                  title = "CI Build 1.2.56-ci.2",
                  htmlUrl = "https://example/release",
                  channel = UpdateChannel.Beta,
                  prerelease = true,
                  releaseNotes = notes,
              ),
          appUpdate =
              if (app) ComponentUpdate(UpdateComponent.App, "1.2.56-ci.1", "1.2.56-ci.2") else null,
          moduleUpdate =
              if (module) ComponentUpdate(UpdateComponent.Module, "1.2.56-ci.1", "1.2.56-ci.2")
              else null,
      )

  private fun componentsOf(update: UpdateAvailability): List<ReleaseNoteComponent> =
      visibleNoteSections(update, parseReleaseNoteSections(update.release.releaseNotes)).map {
        it.component
      }

  @Test
  fun appOnlyUpdateHidesModuleNotes() {
    assertEquals(
        listOf(ReleaseNoteComponent.App, ReleaseNoteComponent.Other),
        componentsOf(availability(app = true, module = false)),
    )
  }

  @Test
  fun moduleOnlyUpdateHidesAppNotes() {
    assertEquals(
        listOf(ReleaseNoteComponent.Module, ReleaseNoteComponent.Other),
        componentsOf(availability(app = false, module = true)),
    )
  }

  @Test
  fun bothUpdatedKeepsEverySection() {
    assertEquals(
        listOf(
            ReleaseNoteComponent.Module,
            ReleaseNoteComponent.App,
            ReleaseNoteComponent.Other,
        ),
        componentsOf(availability(app = true, module = true)),
    )
  }

  @Test
  fun summaryNamesTheChangedComponents() {
    assertEquals("本次仅 App 更新。", updateScopeSummary(availability(app = true, module = false)))
    assertEquals("本次仅模块更新。", updateScopeSummary(availability(app = false, module = true)))
    assertEquals(
        "App 与模块都有新版本。",
        updateScopeSummary(availability(app = true, module = true)),
    )
  }
}
