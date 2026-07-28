package org.srx.manager.data

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** 验证保存配置时保留模块或 WebUI 新增的未知字段，而不是静默清空。 */
class ConfigMergeTest {
  private val json = Json

  @Test
  fun preservesUnknownTopLevelKeysFromDisk() {
    // 模块新增了 App 还不认识的字段，用户改任意开关后该字段必须仍在。
    val incoming = """{"file_monitor_enabled":true}"""
    val existing = """{"file_monitor_enabled":false,"some_future_switch":true,"future_count":7}"""

    val merged = SrxConfigNormalizer.mergeUnknownTopLevelKeys(incoming, existing)
    val result = json.parseToJsonElement(merged).jsonObject

    // App 覆盖的键取新值。
    assertEquals("true", result["file_monitor_enabled"]?.jsonPrimitive?.content)
    // App 不认识的键保留磁盘上的值。
    assertEquals("true", result["some_future_switch"]?.jsonPrimitive?.content)
    assertEquals("7", result["future_count"]?.jsonPrimitive?.content)
  }

  @Test
  fun incomingKeysAlwaysWinOverDisk() {
    val incoming = """{"verbose_logging_enabled":false}"""
    val existing = """{"verbose_logging_enabled":true}"""

    val merged = SrxConfigNormalizer.mergeUnknownTopLevelKeys(incoming, existing)
    val result = json.parseToJsonElement(merged).jsonObject

    assertEquals(1, result.size)
    assertEquals("false", result["verbose_logging_enabled"]?.jsonPrimitive?.content)
  }

  @Test
  fun nestedObjectsAreReplacedNotDeepMerged() {
    // users 由 App 完整建模并整体负责。深合并会让用户删除的用户条目无法真正删除。
    val incoming = """{"users":{"0":{"enabled":true}}}"""
    val existing = """{"users":{"0":{"enabled":false},"10":{"enabled":true}}}"""

    val merged = SrxConfigNormalizer.mergeUnknownTopLevelKeys(incoming, existing)
    val users = json.parseToJsonElement(merged).jsonObject["users"]?.jsonObject

    assertEquals(1, users?.size)
    assertTrue(users?.containsKey("10") == false)
  }

  @Test
  fun blankDiskContentWritesIncomingUnchanged() {
    val incoming = """{"file_monitor_enabled":true}"""

    assertEquals(incoming, SrxConfigNormalizer.mergeUnknownTopLevelKeys(incoming, ""))
    assertEquals(incoming, SrxConfigNormalizer.mergeUnknownTopLevelKeys(incoming, "   "))
  }

  @Test
  fun corruptedDiskContentDoesNotBlockSaving() {
    // 磁盘内容损坏时按原样写入，不能因此让保存失败。
    val incoming = """{"file_monitor_enabled":true}"""

    assertEquals(
        incoming,
        SrxConfigNormalizer.mergeUnknownTopLevelKeys(incoming, """{"truncated": tr"""),
    )
    assertEquals(incoming, SrxConfigNormalizer.mergeUnknownTopLevelKeys(incoming, "not json"))
  }

  @Test
  fun nonObjectDiskContentIsIgnored() {
    // 顶层不是对象时无法做键合并，按原样写入。
    val incoming = """{"file_monitor_enabled":true}"""

    assertEquals(incoming, SrxConfigNormalizer.mergeUnknownTopLevelKeys(incoming, "[1,2,3]"))
  }
}
