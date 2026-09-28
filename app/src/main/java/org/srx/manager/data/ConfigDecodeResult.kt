package org.srx.manager.data

/** 配置读取结果，区分空文件、有效配置和损坏内容，避免调用方静默吞掉格式错误。 */
internal sealed interface ConfigDecodeResult<out T> {
  data class Success<T>(val value: T) : ConfigDecodeResult<T>

  data object Empty : ConfigDecodeResult<Nothing>

  data class Invalid(val reason: String) : ConfigDecodeResult<Nothing>
}
