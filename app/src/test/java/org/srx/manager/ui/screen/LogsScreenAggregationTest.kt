package org.srx.manager.ui.screen

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test
import org.srx.manager.data.InstalledApp
import org.srx.manager.data.LogEntry

class LogsScreenAggregationTest {
  private fun entry(
      processPackage: String,
      packageName: String = processPackage,
      callerPackage: String = packageName,
      watchPackage: String = "",
      callerPid: Int? = null,
      path: String = "/storage/emulated/0/Download/第三方下载/1DMP/storage.redirect.x-v2.3.2-ci.7.zip",
  ): LogEntry =
      LogEntry(
          timestamp = "2026-11-28 11:36:32",
          timeText = "11:36",
          processPackage = processPackage,
          callerPackage = callerPackage,
          packageName = packageName,
          label = packageName,
          operation = "open",
          action = "",
          path = path,
          ok = true,
          watchPackage = watchPackage,
          identifyMethod = "caller",
          identifyReliability = "high",
          callerPid = callerPid,
          eventRole = "request",
          filterOperation = "open:write",
      )

  private fun appIndex(vararg packages: String): Map<String, InstalledApp> =
      packages.associateWith {
        InstalledApp(packageName = it, label = it, isSystem = false, appInfo = null, config = null)
      }

  @Test
  fun sameSecondOpensFromDifferentPidsGetDistinctKeys() {
    // 同一秒内 IDM 的两个下载线程各开一个 fd：内容指纹相同（指纹不含 callerPid）。
    val entries = listOf(entry(IdmProcess, callerPid = 25851), entry(IdmProcess, callerPid = 25852))

    val keys = buildLogEntryKeys(entries)

    assertEquals(entries.size, keys.toSet().size)
    assertEquals(entries[0].contentKey(), keys[0])
    assertEquals("${entries[1].contentKey()}#1", keys[1])
  }

  @Test
  fun identicalEntriesGetDistinctKeys() {
    val entries = listOf(entry(IdmProcess, callerPid = 25851), entry(IdmProcess, callerPid = 25851))

    val keys = buildLogEntryKeys(entries)

    assertEquals(entries.size, keys.toSet().size)
    assertNotEquals(keys[0], keys[1])
  }

  @Test
  fun appendingNewerEntryKeepsExistingKeys() {
    val older = entry(IdmProcess, callerPid = 25851, path = "/storage/emulated/0/Download/a.zip")
    val latest = entry(IdmProcess, callerPid = 25852, path = "/storage/emulated/0/Download/b.zip")
    val before = buildLogEntryKeys(listOf(older))
    val after = buildLogEntryKeys(listOf(latest, older))

    assertEquals(listOf(before.single()), listOf(after[1]))
  }

  @Test
  fun subProcessRecordGroupsUnderItsApplication() {
    val apps = appIndex(IdmPackage, "com.android.providers.media.module")

    val groupKey = entry(IdmProcess, packageName = IdmPackage).aggregateGroupPackage(apps)

    assertEquals(IdmPackage, groupKey)
  }

  @Test
  fun mainProcessAndSubProcessShareOneGroup() {
    val apps = appIndex(IdmPackage)
    val main = entry(IdmPackage).aggregateGroupPackage(apps)
    val service = entry(IdmProcess).aggregateGroupPackage(apps)

    assertEquals(main, service)
  }

  @Test
  fun callerPackageWinsWhenWriterPackageIsNotInstalled() {
    val apps = appIndex("com.example.editor")

    val groupKey =
        entry(
                "com.android.providers.media.module",
                packageName = "com.android.providers.media.module",
                callerPackage = "com.example.editor",
            )
            .aggregateGroupPackage(apps)

    assertEquals("com.example.editor", groupKey)
  }

  @Test
  fun identityWithoutInstalledAppKeepsItsOwnGroup() {
    val apps = appIndex(IdmPackage)
    val unattributed = entry("com.android.providers.media.module")

    val groupKey = unattributed.aggregateGroupPackage(apps)

    assertEquals("com.android.providers.media.module", groupKey)
    assertNotEquals(IdmPackage, groupKey)
  }

  @Test
  fun blankIdentityUsesWriterPackage() {
    val apps = appIndex(IdmPackage)

    val groupKey =
        entry(processPackage = IdmProcess, packageName = "", callerPackage = "-")
            .aggregateGroupPackage(apps)

    assertEquals(IdmPackage, groupKey)
  }

  private companion object {
    private const val IdmPackage = "idm.internet.download.manager.plus"
    private const val IdmProcess = "$IdmPackage:DownloadService"
  }
}
