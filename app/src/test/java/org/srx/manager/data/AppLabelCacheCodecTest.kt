package org.srx.manager.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class AppLabelCacheCodecTest {
  private val now = 1_700_000_000_000L

  private fun sample() =
      mapOf(
          "com.example.alpha" to CachedAppLabel("/data/app/alpha/base.apk", "Alpha"),
          "com.example.beta" to CachedAppLabel("/data/app/beta/base.apk", "测试应用"),
      )

  @Test
  fun roundTripKeepsSourceDirAndLabel() {
    val decoded = AppLabelCacheCodec.decode(AppLabelCacheCodec.encode(sample(), now), now)

    assertEquals(sample(), decoded)
  }

  @Test
  fun decodeRejectsExpiredCache() {
    val encoded = AppLabelCacheCodec.encode(sample(), now)

    assertTrue(AppLabelCacheCodec.decode(encoded, now + 31L * 24 * 60 * 60 * 1000).isEmpty())
  }

  @Test
  fun decodeRejectsUnknownVersion() {
    val encoded =
        AppLabelCacheCodec.encode(sample(), now)
            .replace(Regex("\"version\"\\s*:\\s*1"), "\"version\":9")

    assertTrue(AppLabelCacheCodec.decode(encoded, now).isEmpty())
  }

  @Test
  fun decodeDropsUnsafePackageNamesAndBlankLabels() {
    val encoded =
        AppLabelCacheCodec.encode(
            mapOf(
                "com.example.ok" to CachedAppLabel("/data/app/ok/base.apk", "ok"),
                "bad/pkg" to CachedAppLabel("/data/app/bad/base.apk", "坏包"),
                "com.example.blank" to CachedAppLabel("/data/app/blank/base.apk", "  "),
            ),
            now,
        )

    val decoded = AppLabelCacheCodec.decode(encoded, now)

    assertEquals(setOf("com.example.ok"), decoded.keys)
  }

  @Test
  fun decodeToleratesCorruptedPayload() {
    assertTrue(AppLabelCacheCodec.decode("not-json", now).isEmpty())
  }
}
