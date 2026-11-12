package me.fakerqu.test.storageredirect.test

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class FileMutationLifecycleTest {
  @get:Rule val temporary = TemporaryFolder()

  @Test
  fun lifecycleChecksContentsAndLeavesOnlyMarker() {
    val root = temporary.newFolder("路径 with spaces")
    runFileMutationLifecycle(root)
    assertEquals(listOf("complete.txt"), root.list()?.toList())
    assertEquals("srx-mutation-lifecycle-v1", File(root, "complete.txt").readText())
  }

  @Test
  fun staleFixtureFailsWithoutRemovingExistingData() {
    val root = temporary.newFolder()
    val existing = File(root, "existing.txt").apply { writeText("keep") }
    assertThrows(IllegalStateException::class.java) { runFileMutationLifecycle(root) }
    assertEquals("keep", existing.readText())
    assertEquals(listOf(existing.name), root.list()?.toList())
  }

  @Test
  fun missingDirectoryFailsWithoutCreatingIt() {
    val root = File(temporary.root, "missing")
    assertThrows(IllegalStateException::class.java) { runFileMutationLifecycle(root) }
    assertEquals(false, root.exists())
  }
}
