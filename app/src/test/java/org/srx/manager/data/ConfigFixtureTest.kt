package org.srx.manager.data

import java.nio.file.Path
import kotlin.io.path.exists
import kotlin.io.path.readText
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ConfigFixtureTest {
  private val json = Json {
    ignoreUnknownKeys = true
    prettyPrint = false
  }

  @Test
  fun globalDefaultsFixtureMatchesUiModelDefaults() {
    val config = json.decodeFromString<GlobalConfig>(fixture("global-defaults.json"))

    assertFalse(config.fileMonitorEnabled)
    assertTrue(config.fuseFixEnabled)
    assertFalse(config.autoEnableRedirectForNewApps)
    assertEquals("", config.autoEnableNewAppsTemplateId)
    assertFalse(config.appConfigAutoSave)
    assertEquals(GlobalConfig(), config)
  }

  @Test
  fun fullAppProfileFixtureKeepsPathContractStable() {
    val config = json.decodeFromString<AppConfig>(fixture("app-profile-full.json"))
    val user = config.users.getValue("0")

    assertTrue(user.enabled)
    assertFalse(user.mappingModeOnly)
    assertEquals(
        listOf("Download/Public", "!Download/Public/tmp"),
        user.allowedRealPaths,
    )
    assertEquals(emptyList<String>(), user.excludedRealPaths)
    assertEquals(listOf(".xlDownload"), user.sandboxedPaths)
    assertEquals(listOf("Documents/MyApp"), user.readOnlyPaths)
    assertEquals(
        mapOf(
            "DCIM/MyApp" to "Pictures/MyApp",
            "Download/Cache" to "Android/media/com.example/cache",
        ),
        user.pathMappings,
    )
  }

  @Test
  fun normalizationFixtureMatchesUiNormalizerOutput() {
    val input = json.decodeFromString<AppConfig>(fixture("app-profile-normalization-input.json"))
    val expected =
        json.decodeFromString<AppConfig>(fixture("app-profile-normalization-output.json"))

    assertEquals(expected, SrxConfigNormalizer.normalizeAppConfig(input))
  }

  @Test
  fun backupV2FixtureCanBeRestoredByUiModel() {
    val backup = json.decodeFromString<BackupPayload>(fixture("backup-v2-minimal.json"))

    assertEquals("storage.redirect.x.backup", backup.magic)
    assertEquals(2, backup.schema)
    assertEquals("storage.redirect.x", backup.module.id)
    assertEquals("1.2.52", backup.module.version)
    assertEquals(0, backup.summary.appCount)
    assertEquals(0, backup.summary.userCount)
    assertEquals("SHA-256", backup.integrity.algorithm)
    assertEquals(GlobalConfig(), backup.data.global)
    assertTrue(backup.data.apps.isEmpty())
    assertTrue(backup.data.templates.isEmpty())
    assertEquals(false, backup.data.ui?.predictiveBack)
  }

  private fun fixture(name: String): String {
    val repoRoot = discoverRepoRoot()
    return repoRoot.resolve("docs/config-fixtures/$name").readText()
  }

  private fun discoverRepoRoot(): Path {
    val candidates =
        generateSequence(Path.of(System.getProperty("user.dir"))) { it.parent }
            .flatMap { sequenceOf(it, it.parent?.resolve("srx_core")).filterNotNull() }

    return candidates.firstOrNull { it.resolve("docs/config-fixtures").exists() }
        ?: error("Unable to locate docs/config-fixtures from ${System.getProperty("user.dir")}")
  }
}
