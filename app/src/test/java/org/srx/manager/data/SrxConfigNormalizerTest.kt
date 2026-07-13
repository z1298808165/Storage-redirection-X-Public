package org.srx.manager.data

import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SrxConfigNormalizerTest {
  private val json = Json {
    encodeDefaults = true
    explicitNulls = false
  }

  @Test
  fun normalizeGlobalConfigSanitizesAutoTemplateId() {
    val normalized =
        SrxConfigNormalizer.normalizeGlobalConfig(
            GlobalConfig(
                autoEnableRedirectForNewApps = true,
                autoEnableNewAppsTemplateId = " bad/id ",
            ),
        )

    assertTrue(normalized.autoEnableRedirectForNewApps)
    assertEquals("", normalized.autoEnableNewAppsTemplateId)
  }

  @Test
  fun normalizeAppConfigMergesExcludesAndSanitizesPaths() {
    val config =
        AppConfig(
            users =
                mapOf(
                    "bad/user" to UserProfile(enabled = true),
                    "10" to UserProfile(enabled = true),
                    "0" to
                        UserProfile(
                            allowedRealPaths =
                                listOf(
                                    "/storage/emulated/0/Download/Public",
                                    "Download//Public",
                                    "Download/../Private",
                                ),
                            excludedRealPaths = listOf("Download/Public/tmp", "!Pictures/KeepOut"),
                            sandboxedPaths =
                                listOf("/sdcard/.xlDownload", "Bad/../Path", "Android/media/App"),
                            pathMappings =
                                mapOf(
                                    "/storage/emulated/0/DCIM/MyApp" to
                                        "/data/media/0/Pictures/MyApp",
                                    "Same" to "Same",
                                    "../Bad" to "Target",
                                ),
                        ),
                ),
        )

    val normalized = SrxConfigNormalizer.normalizeAppConfig(config)
    val user0 = normalized.users.getValue("0")

    assertEquals(listOf("0", "10"), normalized.users.keys.toList())
    assertEquals(
        listOf("Download/Public", "!Download/Public/tmp", "!Pictures/KeepOut"),
        user0.allowedRealPaths,
    )
    assertEquals(emptyList<String>(), user0.excludedRealPaths)
    assertEquals(listOf(".xlDownload", "Android/media/App"), user0.sandboxedPaths)
    assertEquals(mapOf("DCIM/MyApp" to "Pictures/MyApp"), user0.pathMappings)
  }

  @Test
  fun keepsExcludeOnAllowedPathConflict() {
    val normalized =
        SrxConfigNormalizer.normalizeAppConfig(
            AppConfig(
                users =
                    mapOf(
                        "0" to
                            UserProfile(
                                allowedRealPaths = listOf("Download", "Pictures"),
                                excludedRealPaths = listOf("Download"),
                            ),
                    ),
            ),
        )

    assertEquals(listOf("!Download", "Pictures"), normalized.users.getValue("0").allowedRealPaths)
    assertEquals(emptyList<String>(), normalized.users.getValue("0").excludedRealPaths)
  }

  @Test
  fun keepsReadOnlyRulesWithExclusions() {
    val normalized =
        SrxConfigNormalizer.normalizeAppConfig(
            AppConfig(
                users =
                    mapOf(
                        "0" to
                            UserProfile(
                                allowedRealPaths = listOf("Pictures"),
                                excludedRealPaths = listOf("Secret"),
                                readOnlyPaths =
                                    listOf(
                                        "/storage/emulated/0/Documents",
                                        "!Documents/tmp",
                                        "Download/Public",
                                        "Download/*",
                                        "Secret",
                                    ),
                            ),
                    ),
            ),
        )

    val user = normalized.users.getValue("0")
    assertEquals(
        listOf("Pictures", "!Secret"),
        user.allowedRealPaths,
    )
    assertEquals(
        listOf("Documents", "!Documents/tmp", "Download/*", "Download/Public"),
        user.readOnlyPaths,
    )
  }

  @Test
  fun normalizeAppConfigDropsCyclicPathMappings() {
    val normalized =
        SrxConfigNormalizer.normalizeAppConfig(
            AppConfig(
                users =
                    mapOf(
                        "0" to
                            UserProfile(
                                pathMappings =
                                    mapOf(
                                        "A" to "B",
                                        "B" to "C",
                                        "C" to "A",
                                        "Keep" to "Target",
                                    ),
                            ),
                    ),
            ),
        )

    assertEquals(mapOf("Keep" to "Target"), normalized.users.getValue("0").pathMappings)
  }

  @Test
  fun normalizeAppConfigDropsOverlyDeepPathMappings() {
    val deepMappings =
        (0..11).associate { index -> "P$index" to "P${index + 1}" } + ("Keep" to "Target")
    val normalized =
        SrxConfigNormalizer.normalizeAppConfig(
            AppConfig(users = mapOf("0" to UserProfile(pathMappings = deepMappings))),
        )
    val mappings = normalized.users.getValue("0").pathMappings

    assertFalse("P0" in mappings)
    assertFalse("P1" in mappings)
    assertTrue("P2" in mappings)
    assertEquals("Target", mappings["Keep"])
  }

  @Test
  fun normalizeAppConfigDropsPrivatePathMappingTargets() {
    val normalized =
        SrxConfigNormalizer.normalizeAppConfig(
            AppConfig(
                users =
                    mapOf(
                        "0" to
                            UserProfile(
                                pathMappings =
                                    mapOf(
                                        "Download/Game" to "Android/data/com.example.game/files",
                                        "Download/Obb" to "Android/obb/com.example.game",
                                        "Download/Media" to "Android/media/com.example.game/cache",
                                        "DCIM/App" to "Pictures/App",
                                    ),
                            ),
                    ),
            ),
        )

    assertEquals(
        mapOf(
            "DCIM/App" to "Pictures/App",
            "Download/Media" to "Android/media/com.example.game/cache",
        ),
        normalized.users.getValue("0").pathMappings,
    )
  }

  @Test
  fun sanitizeEditablePathRejectsUnsafeConfigPaths() {
    assertEquals(
        "",
        SrxConfigNormalizer.sanitizeEditablePath("storage/emulated", allowRuleSyntax = false),
    )
    assertEquals(
        "",
        SrxConfigNormalizer.sanitizeEditablePath("data/media", allowRuleSyntax = false),
    )
    assertEquals(
        "",
        SrxConfigNormalizer.sanitizeEditablePath(
            "Download/${"x".repeat(520)}",
            allowRuleSyntax = false,
        ),
    )
    assertEquals(
        "",
        SrxConfigNormalizer.sanitizeEditablePath(
            "Download" + 0.toChar() + "Bad",
            allowRuleSyntax = false,
        ),
    )
    assertEquals("", SrxConfigNormalizer.sanitizeEditablePath("!Download", allowRuleSyntax = false))
    assertEquals(
        "",
        SrxConfigNormalizer.sanitizeEditablePath("Download/*", allowRuleSyntax = false),
    )
    assertEquals(
        "!Download/tmp",
        SrxConfigNormalizer.sanitizeEditablePath("!Download/tmp", allowRuleSyntax = true),
    )
    assertEquals(
        "Download/*",
        SrxConfigNormalizer.sanitizeEditablePath("Download/*", allowRuleSyntax = true),
    )
  }

  @Test
  fun normalizeFileMonitorFiltersSanitizesRelativePaths() {
    val normalized =
        SrxConfigNormalizer.normalizeFileMonitorFilters(
            FileMonitorFilters(
                excludedPaths =
                    listOf(
                        "Download",
                        "/Android/data",
                        "/storage/emulated/0/MIUI/",
                        "/data/media/0/MIUI/",
                        "storage/emulated/0/Pictures",
                        "Android//media",
                        "Android/../bad",
                        "bad:path",
                        "Download",
                    ),
                excludedOperations = listOf("open:read", " rename* ", "", "open:read"),
            ),
        )

    assertEquals(listOf("Download", "Android/data", "Android/media"), normalized.excludedPaths)
    assertEquals(listOf("open:read", "rename*"), normalized.excludedOperations)
  }

  @Test
  fun normalizeFileMonitorFiltersPreservesInsertionOrder() {
    val normalized =
        SrxConfigNormalizer.normalizeFileMonitorFilters(
            FileMonitorFilters(
                excludedPaths = listOf("Pictures", "Download", "Android/media"),
                excludedOperations = listOf("rename*", "*:create", "open*:read"),
            ),
        )

    assertEquals(listOf("Pictures", "Download", "Android/media"), normalized.excludedPaths)
    assertEquals(
        listOf("rename*", "*:create", "open*:read"),
        normalized.excludedOperations,
    )
  }

  @Test
  fun upgradesLegacyMonitorOps() {
    val normalized =
        SrxConfigNormalizer.normalizeFileMonitorFilters(
            FileMonitorFilters(
                excludedPaths = listOf("Android/data"),
                excludedOperations = listOf("open:read", "rename*", "unlink*", "delete*"),
            ),
        )

    assertEquals(FileMonitorFilters().excludedOperations, normalized.excludedOperations)
  }

  @Test
  fun upgradesFullLegacyMonitorOps() {
    val normalized =
        SrxConfigNormalizer.normalizeFileMonitorFilters(
            FileMonitorFilters(
                excludedPaths = listOf("Android/data"),
                excludedOperations =
                    listOf(
                        "open:read",
                        "open*:read",
                        "rename*",
                        "unlink*",
                        "delete*",
                        "rmdir*",
                        "link*",
                        "symlink*",
                        "truncate*",
                        "ftruncate*",
                        "chmod*",
                        "fchmod*",
                        "utimens*",
                        "futimens*",
                        "attrib*",
                    ),
            ),
        )

    assertEquals(FileMonitorFilters().excludedOperations, normalized.excludedOperations)
  }

  @Test
  fun rejectsStorageRootsForMonitorUi() {
    assertEquals(
        "Download",
        SrxConfigNormalizer.sanitizeMonitorFilterPath("Download", allowLegacyAbsolute = false),
    )
    assertEquals(
        "",
        SrxConfigNormalizer.sanitizeMonitorFilterPath("/Download", allowLegacyAbsolute = false),
    )
    assertEquals(
        "Download",
        SrxConfigNormalizer.sanitizeMonitorFilterPath("/Download", allowLegacyAbsolute = true),
    )
    assertEquals(
        "",
        SrxConfigNormalizer.sanitizeMonitorFilterPath(
            "/storage/emulated/0/MIUI/",
            allowLegacyAbsolute = true,
        ),
    )
    assertEquals(
        "",
        SrxConfigNormalizer.sanitizeMonitorFilterPath(
            "/data/media/0/MIUI/",
            allowLegacyAbsolute = true,
        ),
    )
  }

  @Test
  fun normalizeTemplatesDropsInvalidAndDuplicateIds() {
    val templates =
        SrxConfigNormalizer.normalizeTemplates(
            listOf(
                ConfigTemplate("bad/id", "Invalid"),
                ConfigTemplate("dup", "First"),
                ConfigTemplate("dup", "Second"),
                ConfigTemplate("b", "Beta"),
                ConfigTemplate(
                    "a",
                    " Alpha ",
                    AppConfig(users = mapOf("bad/user" to UserProfile())),
                ),
            ),
        )

    assertEquals(listOf("a", "b", "dup"), templates.map { it.id })
    assertEquals(listOf("Alpha", "Beta", "First"), templates.map { it.name })
    assertTrue(
        templates.first().config.users.isEmpty(),
    )
  }

  @Test
  fun includesCompatibilityDigestVariants() {
    val data =
        BackupData(
            global =
                GlobalConfig(autoEnableRedirectForNewApps = true, verboseLoggingEnabled = true),
            templates = listOf(ConfigTemplate("template", "Template")),
            ui =
                BackupUiPreferences(
                    predictiveBack = true,
                    floatingBottomBar = false,
                    liquidGlass = true,
                    blurEffect = false,
                    dynamicColor = true,
                    accentColor = -14575885,
                    colorStyle = UiColorStyle.Vibrant,
                    colorSpec = UiColorSpec.Spec2021,
                    themeMode = UiThemeMode.Dark,
                    pageScale = 1.1f,
                    autoCheckUpdates = false,
                    updateChannel = UpdateChannel.Beta,
                ),
        )

    val normalized = SrxConfigNormalizer.normalizeBackupData(data)
    val candidates = SrxConfigNormalizer.backupDigestCandidates(json, normalized)
    val canonical = SrxConfigNormalizer.stableJson(json, normalized)
    val fullDigest = SrxConfigNormalizer.sha256Hex(canonical)
    val withoutUiDigest =
        SrxConfigNormalizer.sha256Hex(
            SrxConfigNormalizer.stableJson(json, normalized, includeUiPreferences = false),
        )
    val withoutTemplateIdDigest =
        SrxConfigNormalizer.sha256Hex(
            SrxConfigNormalizer.stableJson(
                json,
                normalized,
                includeAutoEnableNewAppsTemplateId = false,
            ),
        )
    val withoutAutoEnableDigest =
        SrxConfigNormalizer.sha256Hex(
            SrxConfigNormalizer.stableJson(json, normalized, includeAutoEnableNewApps = false),
        )
    val withoutVerboseLoggingDigest =
        SrxConfigNormalizer.sha256Hex(
            SrxConfigNormalizer.stableJson(json, normalized, includeVerboseLogging = false),
        )
    val withoutMonitorFiltersDigest =
        SrxConfigNormalizer.sha256Hex(
            SrxConfigNormalizer.stableJson(json, normalized, includeMonitorFilters = false),
        )
    val legacyMonitorDigest =
        SrxConfigNormalizer.sha256Hex(
            SrxConfigNormalizer.stableJson(
                json,
                normalized.copy(
                    monitorFilters =
                        normalized.monitorFilters.copy(
                            excludedOperations =
                                listOf("open:read", "rename*", "unlink*", "delete*"),
                        ),
                ),
            ),
        )
    val legacyFullMonitorDigest =
        SrxConfigNormalizer.sha256Hex(
            SrxConfigNormalizer.stableJson(
                json,
                normalized.copy(
                    monitorFilters =
                        normalized.monitorFilters.copy(
                            excludedOperations =
                                listOf(
                                    "open:read",
                                    "open*:read",
                                    "rename*",
                                    "unlink*",
                                    "delete*",
                                    "rmdir*",
                                    "link*",
                                    "symlink*",
                                    "truncate*",
                                    "ftruncate*",
                                    "chmod*",
                                    "fchmod*",
                                    "utimens*",
                                    "futimens*",
                                    "attrib*",
                                ),
                        ),
                ),
            ),
        )

    assertEquals(96, candidates.size)
    assertTrue(fullDigest in candidates)
    assertTrue(withoutUiDigest in candidates)
    assertTrue(withoutTemplateIdDigest in candidates)
    assertTrue(withoutAutoEnableDigest in candidates)
    assertTrue(withoutVerboseLoggingDigest in candidates)
    assertTrue(withoutMonitorFiltersDigest in candidates)
    assertTrue(legacyMonitorDigest in candidates)
    assertTrue(legacyFullMonitorDigest in candidates)
    assertTrue(canonical.contains("\"auto_check_updates\":false"))
    assertTrue(canonical.contains("\"update_channel\":\"Beta\""))
    assertTrue(canonical.contains("\"floating_bottom_bar\":false"))
    assertTrue(canonical.contains("\"liquid_glass\":true"))
    assertTrue(canonical.contains("\"blur_effect\":false"))
    assertTrue(canonical.contains("\"dynamic_color\":true"))
    assertTrue(canonical.contains("\"accent_color\":-14575885"))
    assertTrue(canonical.contains("\"color_style\":\"Vibrant\""))
    assertTrue(canonical.contains("\"color_spec\":\"Spec2021\""))
    assertTrue(canonical.contains("\"theme_mode\":\"Dark\""))
    assertTrue(canonical.contains("\"page_scale\":1.1"))
    assertFalse("" in candidates)
  }
}
