package org.srx.manager.data

import org.junit.Assert.assertEquals
import org.junit.Test

class RuntimeStatsParserTest {
  @Test
  fun parsesVersionedRuntimeActivationCount() {
    assertEquals("42", parseRuntimeActivationCount("schema=2\nruntime_activations=42\n"))
    assertEquals(
        ULong.MAX_VALUE.toString(),
        parseRuntimeActivationCount("schema=2\nruntime_activations=${ULong.MAX_VALUE}\n"),
    )
  }

  @Test
  fun rejectsLegacyOrMalformedStats() {
    assertEquals("0", parseRuntimeActivationCount("1234\n"))
    assertEquals("0", parseRuntimeActivationCount("schema=1\nruntime_activations=1234\n"))
    assertEquals("0", parseRuntimeActivationCount("schema=2\nruntime_activations=-1\n"))
    assertEquals("0", parseRuntimeActivationCount("schema=2\nruntime_activations=invalid\n"))
  }

  @Test
  fun formatsTokenStyleCompactCounts() {
    assertEquals("999", formatCompactRuntimeActivationCount("999"))
    assertEquals("1K", formatCompactRuntimeActivationCount("1000"))
    assertEquals("1.2K", formatCompactRuntimeActivationCount("1200"))
    assertEquals("10K", formatCompactRuntimeActivationCount("10000"))
    assertEquals("1M", formatCompactRuntimeActivationCount("999950"))
    assertEquals("100M", formatCompactRuntimeActivationCount("100000000"))
    assertEquals("1B", formatCompactRuntimeActivationCount("1000000000"))
    assertEquals("18.4Qi", formatCompactRuntimeActivationCount(ULong.MAX_VALUE.toString()))
  }
}
