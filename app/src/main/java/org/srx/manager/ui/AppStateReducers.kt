package org.srx.manager.ui

import org.srx.manager.data.DiagnosticArchiveProgress
import org.srx.manager.data.FileMonitorFilters
import org.srx.manager.data.InstalledApp
import org.srx.manager.data.LogEntry

internal sealed interface BusyStateChange {
  data class Started(
      val message: String? = null,
      val progress: Float? = null,
  ) : BusyStateChange

  data class Progress(val value: DiagnosticArchiveProgress) : BusyStateChange

  data object Finished : BusyStateChange
}

internal fun AppUiState.reduceBusy(change: BusyStateChange): AppUiState =
    when (change) {
      is BusyStateChange.Started ->
          copy(
              busy = true,
              busyMessage = change.message,
              busyProgress = change.progress?.coerceIn(0f, 1f),
          )
      is BusyStateChange.Progress -> {
        val nextProgress = change.value.percent.coerceIn(0, 100) / 100f
        copy(
            busy = true,
            busyMessage = change.value.message,
            busyProgress = maxOf(busyProgress ?: 0f, nextProgress),
        )
      }
      BusyStateChange.Finished ->
          copy(
              busy = false,
              busyMessage = null,
              busyProgress = null,
          )
    }

internal sealed interface LogStateChange {
  data object RefreshStarted : LogStateChange

  data class RefreshSucceeded(
      val logs: List<LogEntry>,
      val filters: FileMonitorFilters,
  ) : LogStateChange

  data object RefreshFailed : LogStateChange

  data class AppsResolved(val apps: List<InstalledApp>) : LogStateChange

  data object Cleared : LogStateChange
}

internal fun AppUiState.reduceLogs(change: LogStateChange): AppUiState =
    when (change) {
      LogStateChange.RefreshStarted -> copy(logsRefreshing = true)
      is LogStateChange.RefreshSucceeded ->
          copy(
              logs = change.logs,
              fileMonitorFilters = change.filters,
              logsRefreshing = false,
          )
      LogStateChange.RefreshFailed -> copy(logsRefreshing = false)
      is LogStateChange.AppsResolved -> copy(logApps = change.apps)
      LogStateChange.Cleared ->
          copy(logs = emptyList(), logApps = emptyList(), logsRefreshing = false)
    }
