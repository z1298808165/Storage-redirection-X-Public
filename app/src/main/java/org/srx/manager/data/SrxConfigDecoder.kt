package org.srx.manager.data

import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json

internal object SrxConfigDecoder {
  inline fun <reified T> decode(json: Json, text: String): ConfigDecodeResult<T> {
    if (text.isBlank()) return ConfigDecodeResult.Empty
    return try {
      ConfigDecodeResult.Success(json.decodeFromString<T>(text))
    } catch (error: SerializationException) {
      ConfigDecodeResult.Invalid(error.message ?: "配置格式无效")
    } catch (error: IllegalArgumentException) {
      ConfigDecodeResult.Invalid(error.message ?: "配置内容无效")
    }
  }
}
