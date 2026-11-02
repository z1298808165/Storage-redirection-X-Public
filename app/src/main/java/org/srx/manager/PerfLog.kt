package org.srx.manager

/**
 * 首屏性能日志。
 *
 * 只写 logcat（tag `SrxPerf`），不参与任何业务判断。计时用 [System.nanoTime] 而不是
 * `android.os.SystemClock`，日志调用也兜住异常：单元测试跑在桩 `android.jar` 上， 直接调用 Android 类会抛 "not
 * mocked"，诊断代码不该把测试和业务流程带崩。
 */
internal object PerfLog {
  const val Tag = "SrxPerf"

  fun mark(): Long = System.nanoTime()

  fun sinceMs(mark: Long): Long = (System.nanoTime() - mark) / 1_000_000

  fun log(message: String) {
    runCatching { android.util.Log.i(Tag, message) }
  }
}
