package org.srx.manager.data

import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ConfiguredAppConfigsParserTest {
  private val json = Json {
    ignoreUnknownKeys = true
    prettyPrint = false
    encodeDefaults = true
    explicitNulls = false
  }
  private val marker = ConfiguredAppConfigMarker

  @Test
  fun parserReadsMultipleMarkedAppConfigsInOrder() {
    val first = AppConfig(users = mapOf("0" to UserProfile(enabled = true)))
    val second = AppConfig(users = mapOf("10" to UserProfile(enabled = false)))
    val dump = buildString {
      appendLine()
      appendLine("$marker com.example.first")
      appendLine(json.encodeToString(first))
      appendLine("$marker com.example.second")
      appendLine(json.encodeToString(second))
    }

    val result = parseConfiguredAppConfigDump(dump, marker, json)

    assertEquals(listOf("com.example.first", "com.example.second"), result.keys.toList())
    assertTrue(
        result.getValue("com.example.first").users.getValue("0").enabled,
    )
    assertFalse(
        result.getValue("com.example.second").users.getValue("10").enabled,
    )
  }

  @Test
  fun parserSkipsUnsafePackageNamesAndInvalidJson() {
    val valid = AppConfig(users = mapOf("0" to UserProfile()))
    val dump = buildString {
      appendLine("$marker com.example.valid")
      appendLine(json.encodeToString(valid))
      appendLine("$marker ../../bad")
      appendLine(json.encodeToString(valid))
      appendLine("$marker com.example.broken")
      appendLine("{")
    }

    val result = parseConfiguredAppConfigDump(dump, marker, json)

    assertEquals(setOf("com.example.valid"), result.keys)
  }

  @Test
  fun parserNormalizesAppConfigProfiles() {
    val dump = buildString {
      appendLine("$marker com.example.app")
      appendLine(
          """
          {
            "users": {
              "0": {
                "allowed_real_paths": ["  /storage/emulated/0/Download//Demo  "],
                "excluded_real_paths": ["  Pictures/tmp  "],
                "sandboxed_paths": ["  .cache  "],
                "path_mappings": {
                  " Download/From ": " Pictures/To "
                }
              }
            }
          }
          """
              .trimIndent(),
      )
    }

    val profile =
        parseConfiguredAppConfigDump(dump, marker, json)
            .getValue("com.example.app")
            .users
            .getValue("0")

    assertEquals(listOf("Download/Demo", "!Pictures/tmp"), profile.allowedRealPaths)
    assertEquals(emptyList<String>(), profile.excludedRealPaths)
    assertEquals(listOf(".cache"), profile.sandboxedPaths)
    assertEquals(mapOf("Download/From" to "Pictures/To"), profile.pathMappings)
  }
}
