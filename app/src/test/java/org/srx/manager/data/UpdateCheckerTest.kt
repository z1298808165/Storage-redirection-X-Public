package org.srx.manager.data

import java.io.IOException
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class UpdateCheckerTest {
  private val checker = UpdateChecker("SRX-Manager/test")

  private fun check(
      manifestJson: String = manifest,
      installed: InstalledVersions = InstalledVersions("1.2.56", "1.2.56"),
      channel: UpdateChannel = UpdateChannel.Stable,
  ): UpdateAvailability? =
      checker.checkManifest(
          manifestJson = manifestJson,
          repository = "example/repo",
          installed = installed,
          channel = channel,
      )

  @Test
  fun stableChannelReadsStableManifestEntry() {
    val availability = requireNotNull(check())

    val release = availability.release
    assertEquals("v1.2.57", release.tagName)
    assertEquals("1.2.57", release.versionName)
    assertEquals("Storage Redirect X v1.2.57", release.title)
    assertEquals("https://github.com/example/repo/releases/tag/v1.2.57", release.htmlUrl)
    assertEquals(UpdateChannel.Stable, release.channel)
    assertEquals(false, release.prerelease)
    assertEquals("## 模块更新\n- 修复模块。", release.releaseNotes)
  }

  @Test
  fun legacyManifestReportsBothComponents() {
    // v1 清单没有 app/module 字段：两侧都用条目版本，等价于旧行为。
    val availability = requireNotNull(check())

    assertEquals("1.2.56", availability.appUpdate?.fromVersion)
    assertEquals("1.2.57", availability.appUpdate?.toVersion)
    assertEquals(UpdateComponent.App, availability.appUpdate?.component)
    assertEquals("1.2.57", availability.moduleUpdate?.toVersion)
    assertEquals(UpdateComponent.Module, availability.moduleUpdate?.component)
  }

  @Test
  fun betaChannelReadsBetaManifestEntry() {
    val availability = requireNotNull(check(channel = UpdateChannel.Beta))

    assertEquals("ci-build-123", availability.release.tagName)
    assertEquals("1.2.58-ci.123", availability.release.versionName)
    assertEquals(UpdateChannel.Beta, availability.release.channel)
    assertTrue(availability.release.prerelease)
  }

  @Test
  fun allChannelChoosesHighestVersion() {
    val availability = requireNotNull(check(channel = UpdateChannel.All))

    assertEquals("ci-build-123", availability.release.tagName)
    assertEquals("1.2.58-ci.123", availability.release.versionName)
  }

  @Test
  fun appOnlyReleaseLeavesModuleAlone() {
    val availability =
        requireNotNull(
            check(
                manifestJson = splitManifest,
                installed = InstalledVersions(app = "1.2.56-ci.1", module = "1.2.56-ci.1"),
                channel = UpdateChannel.Beta,
            )
        )

    assertEquals("1.2.56-ci.2", availability.appUpdate?.toVersion)
    assertNull(availability.moduleUpdate)
    assertNotNull(availability.moduleUpdate ?: availability.appUpdate)
  }

  @Test
  fun moduleOnlyReleaseLeavesAppAlone() {
    val availability =
        requireNotNull(
            check(
                manifestJson = moduleOnlyManifest,
                installed = InstalledVersions(app = "1.2.56-ci.2", module = "1.2.56-ci.1"),
                channel = UpdateChannel.Beta,
            )
        )

    assertNull(availability.appUpdate)
    assertEquals("1.2.56-ci.2", availability.moduleUpdate?.toVersion)
  }

  @Test
  fun moduleTargetIgnoresDetectedVersion() {
    val availability =
        requireNotNull(
            check(
                manifestJson = splitManifest,
                installed = InstalledVersions(app = "1.2.56-ci.1", module = "0.9.0"),
                channel = UpdateChannel.Beta,
            )
        )

    assertEquals("1.2.56-ci.2", availability.appUpdate?.toVersion)
    assertEquals("1.2.56-ci.1", availability.moduleUpdate?.toVersion)
  }

  @Test
  fun manifestWithoutModuleFieldDoesNotClaimModuleUpdate() {
    // 带组件字段的清单里缺 module：表示本次没有模块信息，不能拿检测版本顶替。
    val availability =
        requireNotNull(
            check(
                manifestJson = appOnlyFieldManifest,
                installed = InstalledVersions(app = "1.2.56-ci.1", module = "1.2.56-ci.1"),
                channel = UpdateChannel.Beta,
            )
        )

    assertEquals("1.2.56-ci.2", availability.appUpdate?.toVersion)
    assertNull(availability.moduleUpdate)
  }

  @Test
  fun missingInstalledModuleOnlySkipsModuleHalf() {
    val availability =
        requireNotNull(
            check(
                manifestJson = splitManifest,
                installed = InstalledVersions(app = "1.2.56-ci.1", module = ""),
                channel = UpdateChannel.Beta,
            )
        )

    assertNotNull(availability.appUpdate)
    assertNull(availability.moduleUpdate)
  }

  @Test
  fun appDateTimeVersionReplacesLegacySemverLine() {
    val availability =
        requireNotNull(
            check(
                manifestJson = dateTimeAppManifest,
                installed = InstalledVersions(app = "2.3.2-ci.8", module = "2.3.2-ci.7"),
                channel = UpdateChannel.Beta,
            )
        )

    // 跨空间：日期线比历史 semver 新，迁移那次必须提示。
    assertEquals("202611281259", availability.appUpdate?.toVersion)
    assertNull(availability.moduleUpdate)
  }

  @Test
  fun newerDateTimeVersionWinsInsideTheSameLine() {
    val availability =
        requireNotNull(
            check(
                manifestJson = dateTimeAppManifest,
                installed = InstalledVersions(app = "202611281200", module = "2.3.2-ci.7"),
                channel = UpdateChannel.Beta,
            )
        )

    assertEquals("202611281259", availability.appUpdate?.toVersion)
  }

  @Test
  fun installedDateTimeVersionIsNotDowngradedToSemver() {
    // 回退到旧清单（v1）时目标版本是 semver，不能让已装日期线版本被判成「有新版本」。
    assertNull(
        check(
            manifestJson = splitManifest,
            installed = InstalledVersions(app = "202611281259", module = "1.2.56-ci.2"),
            channel = UpdateChannel.Beta,
        )
    )
  }

  @Test
  fun currentOrNewerVersionReturnsNoUpdate() {
    assertNull(
        check(
            manifestJson = splitManifest,
            installed = InstalledVersions("1.2.56-ci.2", "1.2.56-ci.2"),
            channel = UpdateChannel.Beta,
        )
    )
  }

  @Test
  fun newerDetectedVersionIsNotAnUpdate() {
    // 检测版本涨了但两个组件都没变：清单自相矛盾，不能凭检测版本去打扰用户。
    assertNull(
        check(
            manifestJson = staleDetectedVersionManifest,
            installed = InstalledVersions("1.2.56-ci.2", "1.2.56-ci.2"),
            channel = UpdateChannel.Beta,
        )
    )
  }

  @Test
  fun rawGitHubManifestUsesJsDelivrAsFallback() = runBlocking {
    val requests = mutableListOf<String>()
    val fallbackChecker =
        UpdateChecker("SRX-Manager/test") { url ->
          requests += url
          if (url.contains("raw.githubusercontent.com")) throw IOException("HTTP 404")
          manifest
        }

    val availability =
        fallbackChecker.check(
            manifestUrl = "https://raw.githubusercontent.com/example/repo/SRX-R/update.json",
            repository = "example/repo",
            installed = InstalledVersions("1.2.56", "1.2.56"),
            channel = UpdateChannel.Stable,
        )

    requireNotNull(availability)
    assertEquals("1.2.57", availability.release.versionName)
    assertEquals(
        listOf(
            "https://raw.githubusercontent.com/example/repo/SRX-R/update.json",
            "https://cdn.jsdelivr.net/gh/example/repo@SRX-R/update.json",
        ),
        requests,
    )
  }

  @Test
  fun reportsFailureOnlyAfterBothManifestSourcesFail() = runBlocking {
    val requests = mutableListOf<String>()
    val fallbackChecker =
        UpdateChecker("SRX-Manager/test") { url ->
          requests += url
          throw IOException("HTTP 404")
        }

    try {
      fallbackChecker.check(
          manifestUrl = "https://raw.githubusercontent.com/example/repo/SRX-R/update.json",
          repository = "example/repo",
          installed = InstalledVersions("1.2.56", "1.2.56"),
          channel = UpdateChannel.Stable,
      )
      fail("Expected both manifest sources to fail")
    } catch (error: IOException) {
      assertTrue(error.message.orEmpty().contains("首选源与备用镜像均不可用"))
      assertFalse(error.message.orEmpty().contains("请确认仓库分支已提交"))
    }
    assertEquals(2, requests.size)
  }

  @Test
  fun customManifestUrlIsNotRewritten() {
    assertEquals(
        listOf("https://updates.example.com/update.json"),
        updateManifestUrls("https://updates.example.com/update.json"),
    )
  }

  private companion object {
    val manifest =
        """
        {
          "schema": 1,
          "repository": "example/repo",
          "stable": {
            "version": "1.2.57",
            "tag": "v1.2.57",
            "title": "Storage Redirect X v1.2.57",
            "releaseNotes": "## 模块更新\n- 修复模块。"
          },
          "beta": {
            "version": "1.2.58-ci.123",
            "tag": "ci-build-123",
            "title": "CI Build 1.2.58-ci.123"
          },
          "releases": []
        }
        """
            .trimIndent()

    // schema 2：只有 App 重编，模块版本停留在上一版。
    val splitManifest =
        """
        {
          "schema": 2,
          "repository": "example/repo",
          "beta": {
            "version": "1.2.56-ci.2",
            "tag": "ci-build-2",
            "title": "CI Build 1.2.56-ci.2",
            "app": {"version": "1.2.56-ci.2", "url": "https://example/app.apk"},
            "module": {"version": "1.2.56-ci.1"},
            "releaseNotes": "## App 更新\n- 修复 App。"
          },
          "releases": []
        }
        """
            .trimIndent()

    // schema 2：只有 app 字段，没有 module 字段。
    val appOnlyFieldManifest =
        """
                {
                  "schema": 2,
                  "repository": "example/repo",
                  "beta": {
                    "version": "1.2.56-ci.2",
                    "tag": "ci-build-2",
                    "app": {"version": "1.2.56-ci.2"},
                    "releaseNotes": "## App 更新
        - 修复 App。"
                  },
                  "releases": []
                }
        """
            .trimIndent()

    // schema 2：App 走日期时间线，模块仍是 semver。
    val dateTimeAppManifest =
        """
                {
                  "schema": 2,
                  "repository": "example/repo",
                  "beta": {
                    "version": "2.3.2-ci.8",
                    "tag": "ci-build-8",
                    "app": {"version": "202611281259"},
                    "module": {"version": "2.3.2-ci.7"},
                    "releaseNotes": "## App 更新
        - 修复 App。"
                  },
                  "releases": []
                }
        """
            .trimIndent()

    // schema 2：检测版本比两个组件都新（清单自相矛盾）。
    val staleDetectedVersionManifest =
        """
        {
          "schema": 2,
          "repository": "example/repo",
          "beta": {
            "version": "1.2.56-ci.9",
            "tag": "ci-build-9",
            "app": {"version": "1.2.56-ci.2"},
            "module": {"version": "1.2.56-ci.2"}
          },
          "releases": []
        }
        """
            .trimIndent()

    // schema 2：只有模块重编。
    val moduleOnlyManifest =
        """
        {
          "schema": 2,
          "repository": "example/repo",
          "beta": {
            "version": "1.2.56-ci.2",
            "tag": "ci-build-2",
            "title": "CI Build 1.2.56-ci.2",
            "app": {"version": "1.2.56-ci.1"},
            "module": {"version": "1.2.56-ci.2", "url": "https://example/module.zip"},
            "releaseNotes": "## 模块更新\n- 修复模块。"
          },
          "releases": []
        }
        """
            .trimIndent()
  }
}
