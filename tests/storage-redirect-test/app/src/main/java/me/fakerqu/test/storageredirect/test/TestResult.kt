package me.fakerqu.test.storageredirect.test

data class TestResult(
    val testCase: TestCase,
    val passed: Boolean,
    val message: String,
    val durationMs: Long = 0,
    val error: String? = null,
    val metadata: Map<String, String> = emptyMap(),
) {
    fun toLogLine(): String = buildString {
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
}
