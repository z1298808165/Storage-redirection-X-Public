package org.srx.manager.data

import android.content.Context
import android.net.Uri
import android.provider.DocumentsContract
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.srx.manager.root.RootShell
import org.srx.manager.root.isSafePackageName
import org.srx.manager.root.shellQuote

/**
 * 诊断日志包导出边界。
 *
 * 这里集中处理「root 侧归档文件 → 用户目标位置」的写通道：content URI、SAF 文档树， 以及主存储树不可用时的 /data/media
 * 回退，并在导出成功后补记一条文件监控记录。 SrxRepository 只保留业务入口，不再承担写出细节。
 */
internal class DiagnosticArchiveExporter(
    private val context: Context,
    private val shell: RootShell,
    private val fileStore: RootFileStore,
) {
  private companion object {
    /** 导出读取 root 文件的超时，与 RootShell 默认超时保持一致。 */
    const val RootFileCopyTimeoutMs = 120_000L
    /** 等待 stderr 消费线程收尾的时间，仅为回收线程，不影响导出结果。 */
    const val StderrDrainJoinMs = 1_000L
    /** tar.gz 需要保留双扩展名，使用通用 MIME 兼容不识别 application/gzip 的文件客户端。 */
    const val DiagnosticArchiveMimeType = "application/octet-stream"
    const val DefaultArchiveFileName = "storage-redirect-x-logs.tar.gz"
  }

  suspend fun exportToUri(
      uri: Uri,
      onProgress: ((DiagnosticArchiveProgress) -> Unit)? = null,
  ): Boolean =
      withContext(Dispatchers.IO) {
        val archivePath = fileStore.createDiagnosticArchive(onProgress) ?: return@withContext false
        try {
          onProgress?.invoke(DiagnosticArchiveProgress(99, "copy", "正在写入目标文件"))
          val ok = copyRootFileToUri(archivePath, uri)
          if (ok) onProgress?.invoke(DiagnosticArchiveProgress(100, "done", "日志包已保存"))
          ok
        } finally {
          fileStore.removeFile(archivePath)
        }
      }

  suspend fun exportToDirectory(
      directoryUri: Uri,
      fileName: String,
      onProgress: ((DiagnosticArchiveProgress) -> Unit)? = null,
  ): Boolean =
      withContext(Dispatchers.IO) {
        val safeName =
            fileName.replace(Regex("[\\\\/:*?\"<>|\\u0000-\\u001f]"), "_").ifBlank {
              DefaultArchiveFileName
            }
        val monitorTargetPath =
            resolvePrimaryTreePublicStorageDirectory(directoryUri)?.let { joinPath(it, safeName) }
        val archivePath = fileStore.createDiagnosticArchive(onProgress) ?: return@withContext false
        try {
          onProgress?.invoke(DiagnosticArchiveProgress(99, "copy", "正在写入目标文件"))
          val ok =
              exportViaDocumentTree(archivePath, directoryUri, safeName) ||
                  copyToPrimaryTree(archivePath, directoryUri, safeName)
          if (ok) {
            onProgress?.invoke(DiagnosticArchiveProgress(100, "done", "日志包已保存"))
            recordAppExportMonitor(monitorTargetPath, "diagnostic")
          }
          ok
        } finally {
          fileStore.removeFile(archivePath)
        }
      }

  private fun exportViaDocumentTree(
      archivePath: String,
      directoryUri: Uri,
      fileName: String,
  ): Boolean {
    val resolver = context.contentResolver
    val treeDocumentId =
        runCatching { DocumentsContract.getTreeDocumentId(directoryUri) }.getOrNull()
            ?: return false
    val parentUri = DocumentsContract.buildDocumentUriUsingTree(directoryUri, treeDocumentId)
    val fileUri =
        runCatching {
              DocumentsContract.createDocument(
                  resolver,
                  parentUri,
                  DiagnosticArchiveMimeType,
                  fileName,
              )
            }
            .getOrNull() ?: return false
    val ok = copyRootFileToUri(archivePath, fileUri)
    if (!ok) runCatching { DocumentsContract.deleteDocument(resolver, fileUri) }
    return ok
  }

  private fun copyRootFileToUri(archivePath: String, uri: Uri): Boolean {
    // 这里不能复用 RootShell.exec：导出需要把 stdout 直接streaming 到 content URI，
    // 而 exec 会把输出收成字符串。因此在本地补上 RootShell 已有的两项保护：
    // 必须消费 stderr（否则 su 授权提示或 SELinux 告警写满管道缓冲区会让进程卡死），
    // 且 waitFor 必须带超时（否则授权对话框无人应答时会永久阻塞，导出进度条无法取消）。
    val proc =
        try {
          ProcessBuilder("su", "-c", "cat ${shellQuote(archivePath)}")
              .redirectErrorStream(false)
              .start()
        } catch (_: Exception) {
          return false
        }
    val stderrDrain =
        Thread { runCatching { proc.errorStream.use { it.readBytes() } } }
            .apply {
              isDaemon = true
              start()
            }
    return try {
      context.contentResolver.openOutputStream(uri, "w")?.use { output ->
        proc.inputStream.use { input -> input.copyTo(output) }
      } ?: return false
      if (!proc.waitFor(RootFileCopyTimeoutMs, TimeUnit.MILLISECONDS)) {
        return false
      }
      proc.exitValue() == 0
    } catch (_: Exception) {
      false
    } finally {
      runCatching { proc.destroyForcibly() }
      runCatching { stderrDrain.join(StderrDrainJoinMs) }
    }
  }

  private suspend fun recordAppExportMonitor(targetPath: String?, kind: String) {
    val path = targetPath?.takeIf { it.isNotBlank() } ?: return
    val packageName = context.packageName.takeIf { isSafePackageName(it) } ?: "org.srx.manager"
    val exportKind =
        when (kind.lowercase()) {
          "backup" -> "backup"
          else -> "diagnostic"
        }
    val source = if (exportKind == "backup") "app_backup" else "app_export"
    val command =
        "mkdir -p ${shellQuote(LogsDir)} && " +
            "ts=\$(date '+%Y-%m-%d %H:%M:%S' 2>/dev/null || toybox date '+%Y-%m-%d %H:%M:%S' 2>/dev/null); " +
            "printf '%s|%s|%s|OPEN|%s|ret=0|errno=0|identify_method=caller|identify_reliability=high|op=provider_open|op_filter=provider_open:write|source=%s|export_kind=%s\\n' " +
            "\"\${ts:-unknown}\" ${shellQuote(packageName)} ${shellQuote(packageName)} ${shellQuote(path)} ${shellQuote(source)} ${shellQuote(exportKind)} >> ${shellQuote(FileMonitorLogPath)}; " +
            "chmod 666 ${shellQuote(FileMonitorLogPath)} 2>/dev/null || true"
    runCatching { shell.exec(command, timeoutMs = 10_000L) }
  }

  private suspend fun copyToPrimaryTree(
      archivePath: String,
      directoryUri: Uri,
      fileName: String,
  ): Boolean {
    val directoryPath = resolvePrimaryTreeDataMediaDirectory(directoryUri) ?: return false
    val targetPath = joinPath(directoryPath, fileName)
    val command =
        "dir=${shellQuote(directoryPath)}; " +
            "target=${shellQuote(targetPath)}; " +
            "archive=${shellQuote(archivePath)}; " +
            "mkdir -p \"\$dir\" || exit 1; " +
            "chown 1023:1023 \"\$dir\" 2>/dev/null || true; " +
            "chmod 2775 \"\$dir\" 2>/dev/null || true; " +
            "cat \"\$archive\" > \"\$target\"; " +
            "rc=\$?; " +
            "if [ \$rc -eq 0 ]; then chown 1023:1023 \"\$target\" 2>/dev/null || true; chmod 0644 \"\$target\" 2>/dev/null || true; [ -s \"\$target\" ] || rc=1; fi; " +
            "exit \$rc"
    return shell.exec(command).isSuccess
  }

  private fun resolvePrimaryTreeDataMediaDirectory(directoryUri: Uri): String? {
    val tree = resolvePrimaryTreePath(directoryUri) ?: return null
    return buildPrimaryTreePath("/data/media/${tree.userId}", tree.segments)
  }

  private fun resolvePrimaryTreePublicStorageDirectory(directoryUri: Uri): String? {
    val tree = resolvePrimaryTreePath(directoryUri) ?: return null
    return buildPrimaryTreePath("/storage/emulated/${tree.userId}", tree.segments)
  }

  private fun resolvePrimaryTreePath(directoryUri: Uri): PrimaryTreePath? {
    val documentId =
        runCatching { DocumentsContract.getTreeDocumentId(directoryUri) }.getOrNull() ?: return null
    if (!documentId.startsWith("primary:")) return null
    val relativePath = documentId.removePrefix("primary:").trim('/')
    val segments =
        if (relativePath.isEmpty()) {
          emptyList()
        } else {
          relativePath.split('/').filter { it.isNotEmpty() }
        }
    if (segments.any { it == "." || it == ".." || it.indexOf('\u0000') >= 0 }) return null
    return PrimaryTreePath(android.os.Process.myUid() / 100000, segments)
  }

  private fun buildPrimaryTreePath(root: String, segments: List<String>): String {
    return buildString {
      append(root)
      for (segment in segments) {
        append('/')
        append(segment)
      }
    }
  }

  private fun joinPath(directoryPath: String, fileName: String): String {
    return if (directoryPath.endsWith('/')) "$directoryPath$fileName"
    else "$directoryPath/$fileName"
  }

  private data class PrimaryTreePath(
      val userId: Int,
      val segments: List<String>,
  )
}
