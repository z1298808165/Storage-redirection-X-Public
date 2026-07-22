package org.srx.manager.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.srx.manager.data.DiagnosticArchiveProgress
import org.srx.manager.data.FileMonitorFilters

class AppStateReducersTest {
  @Test
  fun busyProgressNeverMovesBackward() {
    val started = AppUiState().reduceBusy(BusyStateChange.Started("starting", 0.4f))
    val progressed =
        started.reduceBusy(
            BusyStateChange.Progress(DiagnosticArchiveProgress(20, "files", "collecting"))
        )

    assertTrue(progressed.busy)
    assertEquals(0.4f, progressed.busyProgress ?: 0f, 0.001f)
    assertEquals("collecting", progressed.busyMessage)
  }

  @Test
  fun finishingBusyWorkClearsTransientDetails() {
    val finished =
        AppUiState()
            .reduceBusy(BusyStateChange.Started("working", 0.5f))
            .reduceBusy(BusyStateChange.Finished)

    assertFalse(finished.busy)
    assertNull(finished.busyMessage)
    assertNull(finished.busyProgress)
  }

  @Test
  fun logRefreshTransitionsAreIndependentFromOtherState() {
    val filters = FileMonitorFilters(excludedPaths = listOf("Download/cache"))
    val refreshing = AppUiState(selectedUser = "10").reduceLogs(LogStateChange.RefreshStarted)
    val completed = refreshing.reduceLogs(LogStateChange.RefreshSucceeded(emptyList(), filters))

    assertTrue(refreshing.logsRefreshing)
    assertFalse(completed.logsRefreshing)
    assertEquals("10", completed.selectedUser)
    assertEquals(filters, completed.fileMonitorFilters)
  }

  @Test
  fun clearingLogsClearsTransientLogState() {
    val cleared = AppUiState(logsRefreshing = true).reduceLogs(LogStateChange.Cleared)

    assertTrue(cleared.logs.isEmpty())
    assertTrue(cleared.logApps.isEmpty())
    assertFalse(cleared.logsRefreshing)
  }
}
