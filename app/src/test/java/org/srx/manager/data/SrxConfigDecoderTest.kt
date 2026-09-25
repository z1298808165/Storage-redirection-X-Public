package org.srx.manager.data

import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SrxConfigDecoderTest {
  @Serializable data class Fixture(val enabled: Boolean = false)

  private val json = Json { ignoreUnknownKeys = true }

  @Test
  fun blankTextIsReportedAsEmpty() {
    assertEquals(ConfigDecodeResult.Empty, SrxConfigDecoder.decode<Fixture>(json, "  "))
  }

  @Test
  fun validTextIsDecoded() {
    val result = SrxConfigDecoder.decode<Fixture>(json, "{\"enabled\":true}")
    assertEquals(ConfigDecodeResult.Success(Fixture(true)), result)
  }

  @Test
  fun malformedTextIsReportedAsInvalid() {
    val result = SrxConfigDecoder.decode<Fixture>(json, "{invalid}")
    assertTrue(result is ConfigDecodeResult.Invalid)
  }
}
