package org.srx.manager.data

import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class BackupPayloadCodecTest {
  private val json = Json {
    ignoreUnknownKeys = true
    prettyPrint = true
    encodeDefaults = true
    explicitNulls = false
  }

  @Test
  fun encodeThenDecodeReturnsNormalizedData() {
    val data =
        BackupData(
            apps =
                mapOf(
                    "com.example.app" to
                        AppConfig(
                            users = mapOf("0" to UserProfile(enabled = true)),
                        ),
                ),
        )

    val text = BackupPayloadCodec.encode(json, data, "1.2.3")
    val restored = BackupPayloadCodec.decode(json, text)

    assertTrue(restored.apps.containsKey("com.example.app"))
    assertEquals(
        true,
        restored.apps["com.example.app"]?.users?.get("0")?.enabled,
    )
  }

  @Test
  fun decodeRejectsForeignBackup() {
    val text =
        BackupPayloadCodec.encode(json, BackupData(), "1.2.3")
            .replace(
                BackupPayloadCodec.Magic,
                "other.module.backup",
            )

    assertRejects(text, "不是 Storage Redirect X 备份")
  }

  @Test
  fun decodeRejectsMalformedJson() {
    assertRejects("{not-json", "备份文件不是有效 JSON")
  }

  private fun assertRejects(text: String, message: String) {
    val error = runCatching { BackupPayloadCodec.decode(json, text) }.exceptionOrNull()
    assertTrue(error is IllegalArgumentException)
    assertEquals(message, error?.message)
  }
}
