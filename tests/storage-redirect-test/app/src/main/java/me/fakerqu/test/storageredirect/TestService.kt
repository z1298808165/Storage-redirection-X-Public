package me.fakerqu.test.storageredirect

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import java.io.File
import java.io.IOException
import java.util.concurrent.Executors
import me.fakerqu.test.storageredirect.receiver.TestCaseReceiver
import me.fakerqu.test.storageredirect.test.StorageRedirectTestRunner
import me.fakerqu.test.storageredirect.test.TestCase
import me.fakerqu.test.storageredirect.test.TestCaseArgs
import me.fakerqu.test.storageredirect.test.TestResult

class TestService : Service() {
  private val executor = Executors.newSingleThreadExecutor()
  private val receiver = TestCaseReceiver()

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onCreate() {
    Log.i(TAG, "TestService onCreate")
    registerReceiver(receiver, IntentFilter(TestCaseReceiver.ACTION_TEST_CASE))
    super.onCreate()
  }

  override fun onStartCommand(
      intent: Intent?,
      flags: Int,
      startId: Int,
  ): Int {
    Log.i(TAG, "TestService onStartCommand action=${intent?.action} startId=$startId")
    if (intent?.action != TestCaseReceiver.ACTION_TEST_CASE) {
      Log.w(TAG, "ignore unexpected action=${intent?.action}")
      stopSelfResult(startId)
      return START_NOT_STICKY
    }

    try {
      promoteToForeground()
    } catch (e: Throwable) {
      Log.e(TAG, "startForeground failed", e)
      writeImmediateFailure("startForeground", e)
      stopSelfResult(startId)
      return START_NOT_STICKY
    }

    val testCase = TestCase.fromId(intent.getStringExtra(TestCaseReceiver.EXTRA_TEST_CASE))
    val args = TestCaseArgs.fromIntent(intent)
    executor.execute {
      try {
        val results =
            try {
              Log.i(TAG, "run test case ${testCase.id}")
              StorageRedirectTestRunner(applicationContext).run(testCase, args)
            } catch (e: Throwable) {
              Log.e(TAG, "runner crashed", e)
              listOf(
                  TestResult(
                      testCase = testCase,
                      passed = false,
                      message = "runner crashed: ${e.javaClass.simpleName}",
                      error = e.stackTraceToString(),
                  ),
              )
            }
        val failed = results.count { !it.passed }
        reportResults(results)

        if (failed > 0) {
          Log.w(
              TAG,
              "completed with $failed failure(s) out of ${results.size}",
          )
        } else {
          Log.i(TAG, "completed ${results.size} result(s)")
        }
      } finally {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
          stopForeground(STOP_FOREGROUND_REMOVE)
        } else {
          @Suppress("DEPRECATION") stopForeground(true)
        }
        stopSelfResult(startId)
      }
    }
    return START_NOT_STICKY
  }

  override fun onDestroy() {
    Log.i(TAG, "TestService onDestroy")
    unregisterReceiver(receiver)
    executor.shutdownNow()
    super.onDestroy()
  }

  private fun promoteToForeground() {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
      startForeground(
          NOTIFICATION_ID,
          createNotification(),
          ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE or
              ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROCESSING,
      )
    } else {
      startForeground(NOTIFICATION_ID, createNotification())
    }
  }

  private fun createNotification(): Notification {
    ensureNotificationChannel()
    return NotificationCompat.Builder(this, CHANNEL_ID)
        .setContentTitle(getString(R.string.test_service_notification_title))
        .setContentText(getString(R.string.test_service_notification_text))
        .setSmallIcon(R.mipmap.ic_launcher)
        .setOngoing(true)
        .build()
  }

  private fun ensureNotificationChannel() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val manager = getSystemService(NotificationManager::class.java) ?: return
    val channel =
        NotificationChannel(
            CHANNEL_ID,
            getString(R.string.test_service_channel_name),
            NotificationManager.IMPORTANCE_LOW,
        )
    manager.createNotificationChannel(channel)
  }

  companion object {
    private const val CHANNEL_ID = "storage_redirect_test"
    private const val NOTIFICATION_ID = 10_000
    private const val RESULT_CURRENT_FILE = "result_current.txt"
    private const val TAG = "StorageRedirectTest"

    fun createIntent(
        context: Context,
        broadcast: Intent,
    ): Intent =
        Intent(context, TestService::class.java).apply {
          action = TestCaseReceiver.ACTION_TEST_CASE
          TestCaseArgs.copyExtras(broadcast, this)
        }
  }

  /**
   * 结果上报按候选目录逐个尝试，任一目录写入成功即视为上报完成。
   *
   * 被测应用自身的外部专属目录同样处于重定向范围内：进程可能在宿主把挂载从 planned 变为已应用之前就开始执行任务，此时 `getExternalFilesDir()`
   * 返回的可见路径尚不可建， 直接写入会抛 `FileNotFoundException: ENOENT`。该异常一旦逃出执行器线程会终止整个
   * 进程，导致所有候选目录都拿不到结果文件，设备测试流只能超时失败。因此这里显式吞掉 单个目录的失败并回退到内部 `filesDir`，两处路径都在测试脚本的结果扫描范围内。
   */
  private fun reportResults(results: List<TestResult>) {
    val targets = ArrayList<File>(2)
    getExternalFilesDir("test_case_result")?.let { targets.add(it) }
    targets.add(File(filesDir, "test_case_result"))

    var succeeded = false
    var lastError: Throwable? = null
    for (target in targets) {
      try {
        if (!ensureResultDir(target)) {
          throw IOException("结果目录不可用: ${target.absolutePath}")
        }
        writeResultFile(File(target, "result_${System.currentTimeMillis()}.txt"), results)
        writeCurrentResultFile(target, results)
        succeeded = true
      } catch (e: Throwable) {
        lastError = e
        Log.w(TAG, "结果目录写入失败 dir=${target.absolutePath}", e)
      }
    }
    if (!succeeded) {
      Log.e(TAG, "全部结果目录写入失败", lastError)
    }
  }

  private fun ensureResultDir(dir: File): Boolean {
    if (dir.isDirectory) return true
    dir.mkdirs()
    return dir.isDirectory
  }

  private fun writeResultFile(
      file: File,
      results: List<TestResult>,
  ) {
    file.parentFile?.mkdirs()
    file.bufferedWriter().use { writer ->
      results.forEach { result ->
        writer.write(result.toLogLine())
        writer.write("\n")
      }
      writer.flush()
    }
  }

  private fun writeCurrentResultFile(
      resultDir: File,
      results: List<TestResult>,
  ) {
    resultDir.mkdirs()
    val file = File(resultDir, RESULT_CURRENT_FILE)
    val temp = File(resultDir, "$RESULT_CURRENT_FILE.tmp")
    if (temp.exists()) temp.delete()
    writeResultFile(temp, results)
    if (file.exists()) file.delete()
    if (!temp.renameTo(file)) {
      writeResultFile(file, results)
      temp.delete()
    }
  }

  private fun writeImmediateFailure(
      stage: String,
      error: Throwable,
  ) {
    val result =
        TestResult(
            testCase = TestCase.ALL_EXCEPT_DELETE,
            passed = false,
            message = "$stage failed: ${error.javaClass.simpleName}",
            error = error.stackTraceToString(),
        )
    try {
      reportResults(listOf(result))
    } catch (e: Throwable) {
      Log.e(TAG, "上报即时失败结果异常", e)
    }
  }
}
