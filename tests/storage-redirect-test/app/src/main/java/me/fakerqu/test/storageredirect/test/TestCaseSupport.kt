package me.fakerqu.test.storageredirect.test

internal inline fun TestCase.measure(block: () -> TestResult): TestResult {
  val startedAt = System.currentTimeMillis()
  return try {
    val outcome = block()
    if (outcome.testCase != this) {
      outcome.copy(
          testCase = this,
          durationMs = System.currentTimeMillis() - startedAt,
      )
    } else {
      outcome.copy(durationMs = System.currentTimeMillis() - startedAt)
    }
  } catch (e: Exception) {
    TestResult(
        testCase = this,
        passed = false,
        message = e.message ?: e.javaClass.simpleName,
        durationMs = System.currentTimeMillis() - startedAt,
        error = e.stackTraceToString(),
    )
  }
}

internal fun TestCase.pass(
    message: String,
    metadata: Map<String, String> = emptyMap(),
): TestResult = TestResult(testCase = this, passed = true, message = message, metadata = metadata)

internal fun TestCase.fail(
    message: String,
    metadata: Map<String, String> = emptyMap(),
): TestResult = TestResult(testCase = this, passed = false, message = message, metadata = metadata)
