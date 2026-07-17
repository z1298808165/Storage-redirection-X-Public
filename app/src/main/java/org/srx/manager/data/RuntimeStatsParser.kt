package org.srx.manager.data

private const val RuntimeStatsSchema = "2"

private data class CompactCountUnit(val divisor: ULong, val suffix: String)

private val CompactCountUnits =
    listOf(
        CompactCountUnit(1_000_000_000_000_000_000uL, "Qi"),
        CompactCountUnit(1_000_000_000_000_000uL, "Q"),
        CompactCountUnit(1_000_000_000_000uL, "T"),
        CompactCountUnit(1_000_000_000uL, "B"),
        CompactCountUnit(1_000_000uL, "M"),
        CompactCountUnit(1_000uL, "K"),
    )

internal fun parseRuntimeActivationCount(raw: String): String {
  val values =
      raw.lineSequence()
          .mapNotNull { line ->
            val separator = line.indexOf('=')
            if (separator <= 0) null
            else line.substring(0, separator).trim() to line.substring(separator + 1).trim()
          }
          .toMap()
  if (values["schema"] != RuntimeStatsSchema) return "0"
  val count = values["runtime_activations"] ?: return "0"
  return count.toULongOrNull()?.toString() ?: "0"
}

internal fun formatCompactRuntimeActivationCount(raw: String): String {
  val value = raw.toULongOrNull() ?: return "0"
  val unitIndex = CompactCountUnits.indexOfFirst { value >= it.divisor }
  if (unitIndex < 0) return value.toString()

  val unit = CompactCountUnits[unitIndex]
  var whole = value / unit.divisor
  val decimalDivisor = unit.divisor / 10uL
  var decimal = (value % unit.divisor) / decimalDivisor
  val roundingRemainder = (value % unit.divisor) % decimalDivisor
  if (roundingRemainder >= decimalDivisor / 2uL) decimal += 1uL
  if (decimal == 10uL) {
    whole += 1uL
    decimal = 0uL
  }
  if (whole == 1_000uL && unitIndex > 0) return "1${CompactCountUnits[unitIndex - 1].suffix}"
  return if (decimal == 0uL) "$whole${unit.suffix}" else "$whole.$decimal${unit.suffix}"
}
