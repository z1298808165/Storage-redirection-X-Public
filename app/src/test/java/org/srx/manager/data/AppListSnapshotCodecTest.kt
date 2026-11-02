package org.srx.manager.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AppListSnapshotCodecTest {
  private fun sampleApps() =
      listOf(
          InstalledApp(
              packageName = "com.example.enabled",
              label = "已启用",
              isSystem = false,
              appInfo = null,
              config = AppConfig(users = mapOf("999" to UserProfile(enabled = true))),
              isInstalled = true,
              userId = "999",
          ),
          InstalledApp(
              packageName = "com.example.disabled",
              label = "已配置未启用",
              isSystem = false,
              appInfo = null,
              config = AppConfig(users = mapOf("999" to UserProfile(enabled = false))),
              isInstalled = true,
              userId = "999",
          ),
          InstalledApp(
              packageName = "com.example.plain",
              label = "未配置",
              isSystem = true,
              appInfo = null,
              config = null,
              isInstalled = true,
              userId = "999",
          ),
      )

  @Test
  fun roundTripKeepsRenderStateWithoutConfigContent() {
    val encoded = AppListSnapshotCodec.encode("999", sampleApps(), 1_700_000_000_000L)
    val decoded = AppListSnapshotCodec.decode("999", encoded) ?: error("decode failed")

    assertEquals(3, decoded.size)
    val enabled = decoded.first { it.packageName == "com.example.enabled" }
    assertTrue(enabled.isEnabled)
    assertTrue(enabled.isConfigured)
    assertTrue(!decoded.first { it.packageName == "com.example.disabled" }.isEnabled)
    val plain = decoded.first { it.packageName == "com.example.plain" }
    assertTrue(plain.isSystem)
    assertTrue(!plain.isConfigured)
    // 快照不承载配置内容：重建出来的档案只有该用户的启用位。
    assertEquals(setOf("999"), enabled.config?.users?.keys)
  }

  @Test
  fun decodeRejectsOtherUser() {
    val encoded = AppListSnapshotCodec.encode("999", sampleApps(), 0L)

    assertNull(AppListSnapshotCodec.decode("0", encoded))
  }

  @Test
  fun decodeRejectsUnknownVersion() {
    val encoded =
        AppListSnapshotCodec.encode("0", sampleApps(), 0L)
            .replace(Regex("\"version\"\\s*:\\s*1"), "\"version\":9")

    assertNull(AppListSnapshotCodec.decode("0", encoded))
  }

  @Test
  fun decodeDropsUnsafePackageNames() {
    val encoded =
        AppListSnapshotCodec.encode(
            "0",
            listOf(
                InstalledApp(
                    packageName = "com.example.ok",
                    label = "ok",
                    isSystem = false,
                    appInfo = null,
                    config = null,
                    userId = "0",
                )
            ),
            0L,
        )

    val decoded = AppListSnapshotCodec.decode("0", encoded) ?: error("decode failed")
    assertEquals(listOf("com.example.ok"), decoded.map { it.packageName })
    assertNull(AppListSnapshotCodec.decode("0;rm", encoded))
  }

  @Test
  fun decodeToleratesCorruptedPayload() {
    assertNull(AppListSnapshotCodec.decode("0", "not-json"))
  }
}
