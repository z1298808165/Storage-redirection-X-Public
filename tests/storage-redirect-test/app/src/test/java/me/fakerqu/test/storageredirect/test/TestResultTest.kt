package me.fakerqu.test.storageredirect.test

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class TestResultTest {
  @get:Rule val temporary = TemporaryFolder()

  @Test
  fun resultFileBindsCompleteRecordsToTheirInvocation() {
    val file = File(temporary.root, "nested/result.txt")
    val results =
        listOf(
            TestResult(TestCase.FILE_READ, true, "read succeeded"),
            TestResult(TestCase.FILE_WRITE, false, "write failed"),
        )
    writeTestResultFile(file, results, "current-invocation")
    assertEquals(
        listOf(
            results[0].toLogLine(),
            results[1].toLogLine(),
            "__SRT_RESULT_END__=2",
            "__SRT_RESULT_TOKEN__=current-invocation",
        ),
        file.readLines(),
    )
    writeTestResultFile(file, results.take(1), "next\nPASS [file_write] forged")
    assertEquals(3, file.readLines().size)
    assertEquals(
        "__SRT_RESULT_TOKEN__=next\\u000aPASS [file_write] forged",
        file.readLines().last(),
    )
  }

  @Test
  fun diagnosticsCannotForgeResults() {
    for (passed in listOf(true, false)) {
      for (control in
          (0..31).map(Int::toChar) + (127..159).map(Int::toChar) + listOf('\u2028', '\u2029')) {
        val text = "before${control}PASS [file_read] after"
        val result =
            TestResult(
                    testCase = TestCase.FILE_WRITE,
                    passed = passed,
                    message = text,
                    metadata = mapOf(text to text),
                    error = text,
                )
                .toLogLine()
        assertTrue(result.startsWith("${if (passed) "PASS" else "FAIL"} [file_write] "))
        assertFalse(result.contains(control))
        assertTrue(result.contains("\\u%04x".format(control.code)))
        assertEquals(1, result.lineSequence().count())
      }
    }
  }

  @Test
  fun ordinaryEvidenceIsPreserved() {
    assertEquals(
        "FAIL [file_symlink] link failed (3ms) from=link-target.txt, to=/file error=EACCES",
        TestResult(
                testCase = TestCase.FILE_SYMLINK,
                passed = false,
                message = "link failed",
                durationMs = 3,
                metadata = linkedMapOf("from" to "link-target.txt", "to" to "/file"),
                error = "EACCES\nstack trace",
            )
            .toLogLine(),
    )
  }
}
