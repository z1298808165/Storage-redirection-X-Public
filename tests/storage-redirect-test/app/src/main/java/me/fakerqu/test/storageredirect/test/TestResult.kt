package me.fakerqu.test.storageredirect.test

import java.io.File

data class TestResult(
    val testCase: TestCase,
    val passed: Boolean,
    val message: String,
    val durationMs: Long = 0,
    val error: String? = null,
    val metadata: Map<String, String> = emptyMap(),
) {
  fun toLogLine(): String =
      buildString {
            append(if (passed) "PASS" else "FAIL")
            append(" [")
            append(testCase.id)
            append("] ")
            append(message)
            append(" (")
            append(durationMs)
            append("ms)")
            if (metadata.isNotEmpty()) {
              append(" ")
              append(metadata.entries.joinToString(", ") { "${it.key}=${it.value}" })
            }
            error?.let {
              append(" error=")
              append(it.lineSequence().first())
            }
          }
          .escapeResultControls()
}

private fun String.escapeResultControls(): String =
    replace(Regex("[\\p{Cc}\\u2028\\u2029]")) { "\\u%04x".format(it.value[0].code) }

internal fun writeTestResultFile(file: File, results: List<TestResult>, resultToken: String) {
  file.parentFile?.mkdirs()
  file.bufferedWriter().use { writer ->
    results.forEach { result ->
      writer.write(result.toLogLine())
      writer.write("\n")
    }
    writer.write("__SRT_RESULT_END__=${results.size}\n")
    writer.write("__SRT_RESULT_TOKEN__=${resultToken.escapeResultControls()}\n")
  }
}
