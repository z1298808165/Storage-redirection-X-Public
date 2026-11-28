package org.srx.manager.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MonitorLogParserTest {
  @Test
  fun escapedFileNamesKeepResultAndCaller() {
    for (name in listOf("普通文件.txt", "file|ret=0|caller_uid=0", "%7C%0A%25", "first\r\nsecond")) {
      val path = "/storage/emulated/0/Download/$name"
      val encoded = encodeMonitorLogField(path)
      val entry =
          parseMonitorLogEntries(
                  "2026-11-16 12:34:00|com.example.app|com.example.app|OPEN|$encoded|field_encoding=percent|ret=-1|errno=13|identify_method=caller|caller_uid=10123|op=open|from=$encoded"
              )
              .single()
      assertEquals(path, entry.path)
      assertEquals(path, entry.fromPath)
      assertFalse(entry.ok)
      assertEquals(10123, entry.callerUid)
    }
    val legacy =
        parseMonitorLogEntries(
                "2026-11-16 12:34:00|com.example.app|com.example.app|OPEN|/storage/emulated/0/Download/%7C|ret=0"
            )
            .single()
    assertTrue(legacy.path.endsWith("/%7C"))
  }

  @Test
  fun conflictingLogFieldsKeepTheResultUnknown() {
    for (fields in
        listOf(
            "ret=-1|ret=0",
            "ret=0|ret=-1",
            "ret=0|caller_uid=0|caller_uid=10123",
            "ret=0|record_integrity=ambiguous",
        )) {
      val entry =
          parseMonitorLogEntries(
                  "2026-11-16 12:34:00|com.example.app|com.example.app|OPEN|/storage/emulated/0/Download/file|$fields"
              )
              .single()
      assertFalse(entry.ok)
      assertEquals("结果未知：日志字段冲突", entry.errorText)
      assertEquals("unknown", entry.identifyMethod)
      assertEquals("unverified", entry.callerBoundary)
      assertEquals("未知来源", entry.label)
    }
  }

  @Test
  fun distinctRequestTasksAndBoundariesNeverCoalesce() {
    val base = "2026-11-16 12:34:01|com.android.providers.media.module|"
    val tail =
        "|OPEN|/storage/emulated/0/Download/identity.txt|ret=3|errno=0|event_role=request|op=open:write|"
    val app =
        base +
            "com.example.app" +
            tail +
            "identify_method=caller|identify_reliability=high|caller_uid=10123|caller_pid=42|caller_start_time=55"
    val variants =
        listOf(
            base +
                "su" +
                tail +
                "identify_method=executor|caller_uid=0|caller_pid=43|caller_boundary=root|native=su",
            base + "android.uid.test" + tail + "identify_method=shared_uid|caller_uid=10123",
            base +
                "-" +
                tail +
                "identify_method=unknown|caller_uid=10123|caller_boundary=unverified",
            app.replace("caller_pid=42", "caller_pid=43"),
            app.replace("caller_start_time=55", "caller_start_time=56"),
            app.replace("caller_uid=10123", "caller_uid=110123"),
            "$app|native=changed-native",
            "$app|caller_context=u:r:another_app:s0",
        )
    for (other in variants) {
      for (records in listOf(listOf(app, other), listOf(other, app))) {
        assertEquals(2, parseMonitorLogEntries(records.joinToString("\n")).size)
      }
    }
    assertEquals(1, parseMonitorLogEntries("$app\n$app").size)
  }

  @Test
  fun correlationDoesNotHideDistinctRequestsOrResults() {
    val success =
        "2026-11-16 12:34:01|com.example.app|com.example.app|OPEN|" +
            "/storage/emulated/0/Download/a.txt|ret=3|errno=0|event_role=request|" +
            "op=open|identify_method=caller|identify_reliability=high|caller_uid=10123|" +
            "caller_pid=42|caller_start_time=55|correlation_id=p1-1"
    val failure = success.replace("ret=3|errno=0", "ret=-1|errno=13")
    for (other in listOf(failure, failure.replace("caller_pid=42", "caller_pid=43"))) {
      for (raw in listOf("$success\n$other", "$other\n$success")) {
        val entries = parseMonitorLogEntries(raw)
        assertEquals(2, entries.size)
        assertEquals(1, entries.count { !it.ok })
      }
    }
    assertEquals(1, parseMonitorLogEntries("$success\n$success").size)
  }

  @Test
  fun successfulCreateDoesNotHideFailedWrite() {
    val create =
        "2026-11-16 12:34:01|com.example.app|com.example.app|CREATE|" +
            "/storage/emulated/0/Download/a.txt|ret=0|errno=0|event_role=request|" +
            "op=fuse_create|identify_method=fuse_redirect|identify_reliability=high|" +
            "source=fuse_redirect|backend=/data/media/0/Download/a.txt"
    val write =
        create.replace("op=fuse_create", "op=write").replace("ret=0|errno=0", "ret=-1|errno=30")
    for (raw in listOf("$create\n$write", "$write\n$create")) {
      val entries = parseMonitorLogEntries(raw)
      assertEquals(2, entries.size)
      assertEquals(1, entries.count { !it.ok })
      assertEquals("write", entries.single { !it.ok }.operation)
    }
  }

  @Test
  fun executorBoundaryKeepsRawTask() {
    for (boundary in listOf("root", "shell", "privileged", "unverified")) {
      val raw =
          "2026-11-16 00:00:00|com.android.providers.media.module|sh|OPEN|" +
              "/storage/emulated/0/Download/test|ret=0|errno=0|event_role=request|op=open:write|" +
              "identify_method=executor|identify_reliability=high|caller_uid=10123|caller_pid=42|" +
              "caller_start_time=55|caller_context=u:r:su:s0|caller_boundary=$boundary|native=sh"
      val entry =
          parseMonitorLogEntries(
                  raw,
                  callerUidResolver = { error("Executor must not resolve an App") },
                  labelResolver = { error("Executor must keep its native name") },
              )
              .single()
      assertEquals("sh", entry.packageName)
      assertEquals(10123, entry.callerUid)
      assertEquals(42, entry.callerPid)
      assertEquals(55L, entry.callerStartTime)
      assertEquals("u:r:su:s0", entry.callerContext)
      assertEquals(boundary, entry.callerBoundary)
      assertEquals("sh", entry.nativeIdentity)
      assertFalse(entry.hasApplicationIdentity)
    }
  }

  @Test
  fun encodedExecutorsKeepColumnsAndStayNative() {
    for (name in listOf("复制+worker", "worker!", "a|ret=0\r\n%7C")) {
      val encoded = encodeMonitorLogField(name)
      val entry =
          parseMonitorLogEntries(
                  "2026-11-16 00:00:00|$encoded|$encoded|OPEN|/storage/emulated/0/Download/test|" +
                      "field_encoding=percent|ret=-1|errno=13|event_role=request|op=open:write|" +
                      "identify_method=executor|caller_boundary=root|caller_uid=0|native=$encoded",
                  callerUidResolver = { error("Executor must not resolve an App") },
                  labelResolver = { error("Executor must keep its name") },
              )
              .single()
      assertEquals(name, entry.processPackage)
      assertEquals(name, entry.callerPackage)
      assertEquals(name, entry.nativeIdentity)
      assertEquals(name, entry.label)
      assertFalse(entry.ok)
      assertFalse(entry.hasApplicationIdentity)
    }
  }

  @Test
  fun sharedUserLabelIsNotAnApplication() {
    val raw =
        "2026-11-16 00:00:00|com.android.providers.media.module|android.uid.system|OPEN|" +
            "/storage/emulated/0/Download/test|ret=0|errno=0|event_role=request|op=open:write|" +
            "identify_method=shared_uid|identify_reliability=fallback|caller_uid=1000"
    val entry =
        parseMonitorLogEntries(
                raw,
                callerUidResolver = { error("Group must not resolve an App") },
                labelResolver = { error("Group must keep its PackageManager label") },
            )
            .single()
    assertEquals("android.uid.system", entry.packageName)
    assertEquals("android.uid.system", entry.label)
    assertEquals("fallback", entry.identifyReliability)
    assertFalse(entry.hasApplicationIdentity)
  }

  @Test
  fun keepsGalleryCreatorForOuaClassifierDirectories() {
    val raw =
        listOf(
                "2026-08-06 00:35:49|com.android.providers.media.module|com.coloros.gallery3d|MKDIR|" +
                    "/storage/emulated/0/oua_classifier|ret=0|errno=0|identify_method=caller|" +
                    "identify_reliability=high|op=mkdir",
                "2026-08-06 00:35:49|com.android.providers.media.module|com.coloros.gallery3d|MKDIR|" +
                    "/storage/emulated/0/oua_classifier/debug|ret=0|errno=0|identify_method=caller|" +
                    "identify_reliability=high|op=mkdir",
                "2026-08-06 00:35:49|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
                    "/storage/emulated/0/oua_classifier|ret=0|errno=0|identify_method=media_provider_fallback|" +
                    "identify_reliability=fallback|op=inotify|source=public_root|watch_package=com.coloros.gallery3d",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)

    assertEquals(3, entries.size)
    assertEquals(
        setOf("com.coloros.gallery3d", "com.android.providers.media.module"),
        entries.map { it.packageName }.toSet(),
    )
    assertTrue(
        entries.filter { it.operation == "mkdir" }.all { it.packageName == "com.coloros.gallery3d" }
    )
  }

  @Test
  fun classifiesFilesystemFallbackAsObservation() {
    val raw =
        "2026-10-03 10:37:27|com.android.providers.media.module|com.android.providers.media.module|OPEN|" +
            "/storage/emulated/0/Pictures/.gs_fs0/.nomedia|ret=0|errno=0|" +
            "identify_method=media_provider_fallback|identify_reliability=fallback|" +
            "event_role=observation|op=open:write|source=allowed_real_path|" +
            "watch_package=com.taobao.idlefish"

    val entry = parseMonitorLogEntries(raw).single()

    assertEquals("observation", entry.eventRole)
    assertEquals("com.android.providers.media.module", entry.packageName)
    assertEquals("com.taobao.idlefish", entry.watchPackage)
  }

  @Test
  fun infersObservationRoleForLegacyFallbackRecord() {
    val raw =
        "2026-10-03 10:37:27|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
            "/storage/emulated/0/Pictures/.gs_fs0|ret=0|errno=0|" +
            "identify_method=media_provider_fallback|identify_reliability=fallback|op=inotify|" +
            "source=allowed_real_path|watch_package=com.taobao.idlefish"

    val entry = parseMonitorLogEntries(raw).single()

    assertEquals("observation", entry.eventRole)
  }

  @Test
  fun prefersCallerOverObservation() {
    val raw =
        listOf(
                "2026-10-03 10:38:56|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
                    "/storage/emulated/0/Pictures/.gs_fs0/.0.jpg|ret=0|errno=0|" +
                    "identify_method=media_provider_fallback|identify_reliability=fallback|" +
                    "event_role=observation|correlation_id=p1-1|op=inotify|source=allowed_real_path|" +
                    "observer_package=com.android.providers.media.module",
                "2026-10-03 10:38:56|com.android.providers.media.module|com.taobao.idlefish|OPEN|" +
                    "/storage/emulated/0/Pictures/.gs_fs0/.0.jpg|ret=194|errno=0|" +
                    "identify_method=caller|identify_reliability=high|event_role=request|" +
                    "correlation_id=p1-1|op=open|op_filter=open:create|flags=0x20242",
            )
            .joinToString("\n")

    val entry = parseMonitorLogEntries(raw).single()

    assertEquals("com.taobao.idlefish", entry.callerPackage)
    assertEquals("request", entry.eventRole)
    assertEquals("caller", entry.identifyMethod)
    assertEquals(1, entry.observationCount)
    assertEquals("com.android.providers.media.module", entry.observerPackage)
  }

  @Test
  fun coalescesProviderMkdirWithUncorrelatedObservation() {
    val raw =
        listOf(
                "2026-10-03 13:29:40|com.android.providers.media.module|com.taobao.idlefish|MKDIR|" +
                    "/storage/emulated/0/Pictures/.gs/|ret=0|errno=0|" +
                    "identify_method=provider_mkdir|identify_reliability=high|event_role=request|" +
                    "op=mkdir|op_filter=mkdir|source=media_provider_java",
                "2026-10-03 13:29:40|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
                    "/storage/emulated/0/Pictures/.gs|ret=0|errno=0|" +
                    "identify_method=media_provider_fallback|identify_reliability=fallback|" +
                    "event_role=observation|op=inotify|source=allowed_real_path|" +
                    "observer_package=srx_daemon",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)
    assertEquals(
        entries.joinToString("; ") {
          "${it.eventRole}/${it.identifyMethod}/${it.path}/${it.timestamp}"
        },
        1,
        entries.size,
    )
    val entry = entries.single()

    assertEquals("com.taobao.idlefish", entry.packageName)
    assertEquals("provider_mkdir", entry.identifyMethod)
    assertEquals("request", entry.eventRole)
    assertEquals(1, entry.observationCount)
  }

  @Test
  fun coalescesMappedFuseRequestWithInotifyObservation() {
    val raw =
        listOf(
                "2026-10-03 13:30:12|com.tencent.mobileqq|com.tencent.mobileqq|OPEN|" +
                    "/storage/emulated/0/Download/QQ/storage.redirect.x.zip|ret=0|errno=0|" +
                    "identify_method=fuse_redirect|identify_reliability=high|event_role=request|" +
                    "op=open:write|source=path_mapping|" +
                    "backend=/data/media/0/Download/第三方下载/QQ/storage.redirect.x.zip",
                "2026-10-03 13:30:12|com.tencent.mobileqq|com.tencent.mobileqq|OPEN|" +
                    "/storage/emulated/0/Download/QQ/storage.redirect.x.zip|ret=0|errno=0|" +
                    "identify_method=daemon_inotify|identify_reliability=medium|event_role=observation|" +
                    "op=inotify|source=path_mapping|observer_package=srx_daemon|" +
                    "backend=/data/media/0/Download/第三方下载/QQ/storage.redirect.x.zip|" +
                    "from=/storage/emulated/0/Android/data/com.tencent.mobileqq/Tencent/QQfile_recv/storage.redirect.x.zip",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)

    assertEquals(1, entries.size)
    val entry = entries.single()
    assertEquals("com.tencent.mobileqq", entry.packageName)
    assertEquals("fuse_redirect", entry.identifyMethod)
    assertEquals("request", entry.eventRole)
    assertEquals(1, entry.observationCount)
    assertEquals("srx_daemon", entry.observerPackage)
  }

  @Test
  fun parsesEntriesBeyondPreviousPreviewLimit() {
    val raw =
        (0..500).joinToString("\n") { index ->
          "2026-06-10 10:20:${(index % 60).toString().padStart(2, '0')}|" +
              "com.example.app|com.example.app|OPEN|/storage/emulated/0/Download/$index.txt|" +
              "op=open|op_filter=open:read|ret=3"
        }

    val entries = parseMonitorLogEntries(raw)

    assertEquals(501, entries.size)
  }

  @Test
  fun prefersRealCaller() {
    val raw =
        "2026-06-10 10:20:30.000|com.android.providers.media.module|com.example.camera|open|" +
            "file:///storage/emulated/0/DCIM/.pending.js|op=open:create|ret=3|identify_method=caller|identify_reliability=high"

    val entries = parseMonitorLogEntries(raw) { "Label:$it" }

    assertEquals(1, entries.size)
    val entry = entries.single()
    assertEquals("com.example.camera", entry.packageName)
    assertEquals("Label:com.example.camera", entry.label)
    assertEquals("open", entry.operation)
    assertEquals("create", entry.operationIntent)
    assertEquals("带创建意图的文件打开", entry.action)
    assertEquals("/storage/emulated/0/DCIM/pending", entry.path)
    assertTrue(entry.ok)
  }

  @Test
  fun keepsRequestsFromDifferentStorageUsers() {
    val raw =
        listOf(
                "2026-06-10 10:20:01.000|com.android.providers.media.module|com.android.providers.media.module|open|" +
                    "/storage/emulated/0/DCIM/a.jpg|op=open:create|ret=3",
                "2026-06-10 10:20:20.000|com.android.providers.media.module|com.example.camera|open|" +
                    "/storage/emulated/10/DCIM/a.jpg|op=open:create|ret=3|from=/data/media/10/DCIM/a.jpg|" +
                    "identify_method=caller|identify_reliability=high",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)

    assertEquals(2, entries.size)
    assertEquals(
        setOf("/storage/emulated/0/DCIM/a.jpg", "/storage/emulated/10/DCIM/a.jpg"),
        entries.map { it.path }.toSet(),
    )
    val entry = entries.single { it.packageName == "com.example.camera" }
    assertEquals("com.example.camera", entry.packageName)
    assertEquals("com.example.camera", entry.callerPackage)
    assertEquals("/data/media/10/DCIM/a.jpg", entry.fromPath)
  }

  @Test
  fun keepsMountPreparationAlongsideCallerMkdir() {
    val raw =
        listOf(
                "2026-06-10 10:20:01|com.coloros.gallery3d|com.coloros.gallery3d|MKDIR|" +
                    "/storage/emulated/0/.MediaTrash|ret=0|errno=0|identify_method=mount_prep|" +
                    "identify_reliability=high|op=mkdir|source=mount_prep|" +
                    "backend=/data/media/0/Android/data/com.coloros.gallery3d/sdcard/.MediaTrash",
                "2026-06-10 10:20:20|com.android.providers.media.module|bin.mt.plus|MKDIR|" +
                    "/storage/emulated/0/.MediaTrash|ret=0|errno=0|identify_method=caller|" +
                    "identify_reliability=high|op=mkdir",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)

    assertEquals(2, entries.size)
    assertEquals("mount_prep", entries.last().identifyMethod)
    assertEquals("bin.mt.plus", entries.first().callerPackage)
  }

  @Test
  fun keepsUncorrelatedProviderObservation() {
    val raw =
        listOf(
                "2026-06-23 12:10:04|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
                    "/storage/emulated/0/Download/Nnngram/save.jpg|" +
                    "ret=0|errno=0|identify_method=media_provider_fallback|identify_reliability=fallback|" +
                    "op=inotify|source=public_root|watch_package=xyz.nextalone.nnngram",
                "2026-06-23 12:10:03|xyz.nextalone.nnngram|xyz.nextalone.nnngram|OPEN|" +
                    "/storage/emulated/0/Download/Nnngram/save.jpg|" +
                    "ret=5|errno=0|identify_method=caller|identify_reliability=high|" +
                    "op=open|op_filter=open:write|flags=0x8001",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw) { "Label:$it" }

    assertEquals(2, entries.size)
    assertEquals(setOf("request", "observation"), entries.map { it.eventRole }.toSet())
    val entry = entries.single { it.eventRole == "request" }
    assertEquals("xyz.nextalone.nnngram", entry.packageName)
    assertEquals("Label:xyz.nextalone.nnngram", entry.label)
    assertEquals("open", entry.operation)
    assertEquals("write", entry.operationIntent)
    assertEquals("caller", entry.identifyMethod)
  }

  @Test
  fun parserCoalescesFuseCreateAheadOfNormalCreate() {
    val raw =
        listOf(
                "2026-06-23 12:20:03|com.tencent.mobileqq|com.tencent.mobileqq|CREATE|" +
                    "/storage/emulated/0/Download/QQ/save.jpg|" +
                    "ret=0|errno=0|identify_method=daemon_inotify|identify_reliability=medium|" +
                    "op=inotify|source=redirect_root|" +
                    "backend=/data/media/0/Android/data/com.tencent.mobileqq/sdcard/Download/QQ/save.jpg|" +
                    "from=/storage/emulated/0/Download/QQ/save.jpg",
                "2026-06-23 12:20:03|com.tencent.mobileqq|com.tencent.mobileqq|CREATE|" +
                    "/storage/emulated/0/Download/QQ/save.jpg|" +
                    "ret=0|errno=0|identify_method=fuse_redirect|identify_reliability=high|" +
                    "op=fuse_create|source=fuse_redirect|backend=/data/media/0/Android/data/com.tencent.mobileqq/sdcard/Download/QQ/save.jpg",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw) { "Label:$it" }

    assertEquals(1, entries.size)
    val entry = entries.single()
    assertEquals("com.tencent.mobileqq", entry.packageName)
    assertEquals("Label:com.tencent.mobileqq", entry.label)
    assertEquals("fuse_create", entry.operation)
    assertEquals("fuse_redirect", entry.identifyMethod)
    assertEquals("fuse_redirect", entry.source)
  }

  @Test
  fun keepsUncorrelatedFuseCreateAndLaterObservation() {
    val raw =
        listOf(
                "2026-06-23 18:33:13|com.tencent.mobileqq|com.tencent.mobileqq|CREATE|" +
                    "/storage/emulated/0/Download/QQ/storage.redirect.x.zip|" +
                    "ret=0|errno=0|identify_method=fuse_redirect|identify_reliability=high|" +
                    "op=fuse_create|source=fuse_redirect|" +
                    "backend=/data/media/0/Download/第三方下载/QQ/storage.redirect.x.zip",
                "2026-06-23 18:33:17|com.tencent.mobileqq|com.tencent.mobileqq|CREATE|" +
                    "/storage/emulated/0/Download/第三方下载/QQ/storage.redirect.x.zip|" +
                    "ret=0|errno=0|identify_method=owner_uid|identify_reliability=high|" +
                    "op=inotify|source=path_mapping|mask=0x100|" +
                    "backend=/data/media/0/Download/第三方下载/QQ/storage.redirect.x.zip|" +
                    "from=/storage/emulated/0/Download/QQ/storage.redirect.x.zip",
                "2026-06-23 18:33:17|com.tencent.mobileqq|com.tencent.mobileqq|CREATE|" +
                    "/storage/emulated/0/Download/第三方下载/QQ/storage.redirect.x.zip|" +
                    "ret=0|errno=0|identify_method=owner_uid|identify_reliability=high|" +
                    "op=inotify|source=read_only_path|mask=0x100|" +
                    "backend=/data/media/0/Download/第三方下载/QQ/storage.redirect.x.zip",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)
    assertEquals(2, entries.size)
    assertEquals(1, entries.count { it.eventRole == "observation" })
    val entry = entries.single { it.eventRole == "request" }

    assertEquals("com.tencent.mobileqq", entry.packageName)
    assertEquals("fuse_redirect", entry.identifyMethod)
    assertEquals("/data/media/0/Download/第三方下载/QQ/storage.redirect.x.zip", entry.backendPath)
  }

  @Test
  fun coalescesFuseCreateWithSameSecondWrite() {
    val raw =
        listOf(
                "2026-10-04 09:08:17|com.taobao.idlefish|com.taobao.idlefish|CREATE|" +
                    "/storage/emulated/0/Pictures/.gs/com.taobao.idlefish/.gs_fs0/0/a.jpg|" +
                    "ret=0|errno=0|identify_method=fuse_redirect|identify_reliability=high|" +
                    "op=fuse_create|source=fuse_redirect|" +
                    "backend=/data/media/0/Pictures/.gs/com.taobao.idlefish/.gs_fs0/0/a.jpg",
                "2026-10-04 09:08:17|com.taobao.idlefish|com.taobao.idlefish|WRITE|" +
                    "/storage/emulated/0/Pictures/.gs/com.taobao.idlefish/.gs_fs0/0/a.jpg|" +
                    "ret=0|errno=0|identify_method=fuse_redirect|identify_reliability=high|" +
                    "event_role=request|op=write|source=fuse_redirect|" +
                    "backend=/data/media/0/Pictures/.gs/com.taobao.idlefish/.gs_fs0/0/a.jpg",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)

    assertEquals(1, entries.size)
    val entry = entries.single()
    assertEquals("com.taobao.idlefish", entry.packageName)
    assertEquals("fuse_create", entry.operation)
    assertEquals("创建并写入", entry.action)
  }

  @Test
  fun prefersMappedObservationOverFallback() {
    val raw =
        listOf(
                "2026-10-04 07:12:08|com.tencent.mobileqq|com.tencent.mobileqq|CREATE|" +
                    "/storage/emulated/0/Download/第三方下载/QQ/nt_qq_cab28b4c420bb9feb8d24642887f7579|" +
                    "ret=0|errno=0|identify_method=daemon_inotify|identify_reliability=medium|" +
                    "event_role=observation|op=inotify|source=path_mapping|mask=0x40000100|" +
                    "backend=/data/media/0/Download/第三方下载/QQ/nt_qq_cab28b4c420bb9feb8d24642887f7579|" +
                    "from=/storage/emulated/0/Android/data/com.tencent.mobileqq/Tencent/QQfile_recv/nt_qq_cab28b4c420bb9feb8d24642887f7579|" +
                    "observer_package=srx_daemon",
                "2026-10-04 07:12:08|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
                    "/storage/emulated/0/Download/第三方下载/QQ/nt_qq_cab28b4c420bb9feb8d24642887f7579|" +
                    "ret=0|errno=0|identify_method=media_provider_fallback|identify_reliability=fallback|" +
                    "event_role=observation|op=inotify|source=read_only_path|mask=0x40000100|" +
                    "backend=/data/media/0/Download/第三方下载/QQ/nt_qq_cab28b4c420bb9feb8d24642887f7579|" +
                    "watch_package=com.aliyun.tongyi|observer_package=srx_daemon",
            )
            .joinToString("\n")

    val entry = parseMonitorLogEntries(raw).single()

    assertEquals("com.tencent.mobileqq", entry.packageName)
    assertEquals("daemon_inotify", entry.identifyMethod)
    assertEquals("path_mapping", entry.source)
    assertEquals("observation", entry.eventRole)
  }

  @Test
  fun keepsUncorrelatedMappedObservation() {
    val raw =
        listOf(
                "2026-06-26 16:52:45|com.android.providers.media.module|com.tencent.mm|OPEN|" +
                    "/storage/emulated/0/Download/第三方下载/微信/storage.redirect.x-v1.2.55-local.zip|" +
                    "ret=183|errno=0|identify_method=path_config|identify_reliability=medium|" +
                    "op=open|op_filter=open:create|flags=0x20242",
                "2026-06-26 16:52:45|com.tencent.mm|com.tencent.mm|CREATE|" +
                    "/storage/emulated/0/Download/第三方下载/微信/storage.redirect.x-v1.2.55-local.zip|" +
                    "ret=0|errno=0|identify_method=watch_package|identify_reliability=medium|" +
                    "op=inotify|source=path_mapping|mask=0x100|" +
                    "backend=/data/media/0/Download/第三方下载/微信/storage.redirect.x-v1.2.55-local.zip|" +
                    "from=/storage/emulated/0/Download/Weixin/storage.redirect.x-v1.2.55-local.zip",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)
    assertEquals(2, entries.size)
    assertEquals("open", entries.single { it.eventRole == "request" }.operation)
    val entry = entries.single { it.eventRole == "observation" }

    assertEquals("com.tencent.mm", entry.packageName)
    assertEquals("inotify", entry.operation)
    assertEquals("watch_package", entry.identifyMethod)
    assertEquals("path_mapping", entry.source)
    assertEquals(
        "/storage/emulated/0/Download/Weixin/storage.redirect.x-v1.2.55-local.zip",
        entry.fromPath,
    )
  }

  @Test
  fun parserDropsMediaStorePendingIntermediateRecords() {
    val raw =
        listOf(
                "2026-06-26 17:24:30|com.android.providers.media.module|com.tencent.mm|OPEN|" +
                    "/storage/emulated/0/Download/Weixin/.pending-1783070670-storage.redirect.x-v1.2.55-local.zip|" +
                    "ret=190|errno=0|identify_method=path_config|identify_reliability=medium|" +
                    "op=open|op_filter=open:create|flags=0x88042",
                "2026-06-26 17:24:30|com.android.providers.media.module|com.tencent.mm|OPEN|" +
                    "/storage/emulated/0/Download/第三方下载/微信/.pending-1783070670-storage.redirect.x-v1.2.55-local.zip|" +
                    "ret=-1|errno=2|identify_method=path_config|identify_reliability=medium|" +
                    "op=open|op_filter=open:write|flags=0x28002|" +
                    "from=/storage/emulated/0/Download/Weixin/.pending-1783070670-storage.redirect.x-v1.2.55-local.zip|" +
                    "source=path_mapping",
                "2026-06-26 17:24:30|com.android.providers.media.module|com.tencent.mm|OPEN|" +
                    "/storage/emulated/0/Download/Weixin/.pending-1783070670-storage.redirect.x-v1.2.55-local.zip|" +
                    "ret=-1|errno=13|identify_method=path_config|identify_reliability=medium|" +
                    "op=open|op_filter=open:create|flags=0x88042",
                "2026-06-26 17:24:31|com.tencent.mm|com.tencent.mm|CREATE|" +
                    "/storage/emulated/0/Download/第三方下载/微信/storage.redirect.x-v1.2.55-local.zip|" +
                    "ret=0|errno=0|identify_method=watch_package|identify_reliability=medium|" +
                    "op=inotify|source=path_mapping|mask=0x100|" +
                    "backend=/data/media/0/Download/第三方下载/微信/storage.redirect.x-v1.2.55-local.zip|" +
                    "from=/storage/emulated/0/Download/Weixin/storage.redirect.x-v1.2.55-local.zip",
            )
            .joinToString("\n")

    val entry = parseMonitorLogEntries(raw).single()

    assertEquals(
        "/storage/emulated/0/Download/第三方下载/微信/storage.redirect.x-v1.2.55-local.zip",
        entry.path,
    )
    assertEquals(
        "/storage/emulated/0/Download/Weixin/storage.redirect.x-v1.2.55-local.zip",
        entry.fromPath,
    )
    assertEquals("path_mapping", entry.source)
  }

  @Test
  fun parserKeepsMediaStorePendingCommitFinalRecord() {
    val raw =
        "2026-07-03 08:47:24|com.android.providers.media.module|com.tencent.mm|OPEN|" +
            "/storage/emulated/0/Download/WeiXin/storage-redirect-x-backup-20260619-130840.srxbak.zip|" +
            "ret=0|errno=0|identify_method=caller|identify_reliability=high|op=rename|" +
            "op_filter=open:create|from=/storage/emulated/0/Download/WeiXin/.pending-1783058689-storage-redirect-x-backup-20260619-130840.srxbak.zip|" +
            "source=media_store_pending_commit"

    val entry = parseMonitorLogEntries(raw).single()

    assertEquals("com.tencent.mm", entry.packageName)
    assertEquals("rename", entry.operation)
    assertEquals("create", entry.operationIntent)
    assertEquals("带创建意图的文件打开", entry.action)
    assertEquals(
        "/storage/emulated/0/Download/WeiXin/storage-redirect-x-backup-20260619-130840.srxbak.zip",
        entry.path,
    )
    assertEquals(
        "/storage/emulated/0/Download/WeiXin/.pending-1783058689-storage-redirect-x-backup-20260619-130840.srxbak.zip",
        entry.fromPath,
    )
    assertEquals("media_store_pending_commit", entry.source)
  }

  @Test
  fun describesOpenIntentSeparately() {
    val readEntry =
        parseMonitorLogEntries(
                "2026-07-28 21:37:20|com.android.providers.media.module|com.coloros.gallery3d|OPEN|" +
                    "/storage/emulated/0/.MediaTrash/.mediaTrash|ret=131|errno=0|" +
                    "identify_method=caller|identify_reliability=high|op=open|op_filter=open:read|flags=0x84000",
                filters =
                    FileMonitorFilters(
                        excludedPaths = emptyList(),
                        excludedOperations = emptyList(),
                    ),
            )
            .single()
    val writeEntry =
        parseMonitorLogEntries(
                "2026-07-28 21:39:05|com.android.providers.media.module|com.example.app|OPEN|" +
                    "/storage/emulated/0/Download/a.json|ret=131|errno=0|" +
                    "identify_method=caller|identify_reliability=high|op=open|op_filter=open:write|flags=0x20202",
                filters = FileMonitorFilters(excludedPaths = emptyList()),
            )
            .single()
    val plainEntry =
        parseMonitorLogEntries(
                "2026-07-28 21:39:06|com.example.app|com.example.app|OPEN|" +
                    "/storage/emulated/0/Download/b.json|ret=5|errno=0|op=open",
                filters = FileMonitorFilters(excludedPaths = emptyList()),
            )
            .single()

    assertEquals("read", readEntry.operationIntent)
    assertEquals("文件读取请求", readEntry.action)
    assertEquals("write", writeEntry.operationIntent)
    assertEquals("文件写入请求", writeEntry.action)
    assertEquals("", plainEntry.operationIntent)
    assertEquals("文件打开请求", plainEntry.action)
  }

  @Test
  fun describesProviderOpenWithoutIntentSuffix() {
    val entry =
        parseMonitorLogEntries(
                "2026-07-28 21:40:00|com.android.externalstorage|com.example.app|OPEN|" +
                    "/storage/emulated/0/Download/c.apk|ret=0|errno=0|" +
                    "identify_method=saf_provider|identify_reliability=high|op=provider_open|" +
                    "op_filter=provider_open|source=saf_provider|caller_uid=10360"
            )
            .single()

    assertEquals("provider_open", entry.operation)
    assertEquals("", entry.operationIntent)
    assertEquals("Provider 打开请求", entry.action)
  }

  @Test
  fun parserSkipsMonitorWatchRecordsAndMalformedLines() {
    val raw =
        listOf(
                "malformed",
                "2026-06-10 10:20:30.000|com.example.app|com.example.app|inotify|/storage/emulated/0/Download/a.txt|op=monitor_watch|ret=0",
                "2026-06-10 10:21:30.000|com.example.app|com.example.app|inotify|/storage/emulated/0/Download/b.txt|op=inotify|ret=0",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)

    assertEquals(1, entries.size)
    assertEquals("inotify", entries.single().operation)
    assertEquals("/storage/emulated/0/Download/b.txt", entries.single().path)
  }

  @Test
  fun parserCachesLabelsPerPackage() {
    var resolveCount = 0
    val raw =
        listOf(
                "2026-06-10 10:20:30.000|com.example.app|com.example.app|mkdir|/storage/emulated/0/Download/a|op=mkdir|ret=0",
                "2026-06-10 10:21:30.000|com.example.app|com.example.app|mkdir|/storage/emulated/0/Download/b|op=mkdir|ret=0",
            )
            .joinToString("\n")

    val entries =
        parseMonitorLogEntries(raw) {
          resolveCount += 1
          "Label:$it"
        }

    assertEquals(2, entries.size)
    assertEquals(1, resolveCount)
    assertTrue(entries.all { it.label == "Label:com.example.app" })
  }

  @Test
  fun parserMapsFailedErrnoToReadableText() {
    val raw =
        "2026-06-10 10:20:30.000|com.example.app|com.example.app|unlink|" +
            "/storage/emulated/0/Download/a.txt|op=unlink|ret=-1|errno=13"

    val entry = parseMonitorLogEntries(raw).single()

    assertFalse(entry.ok)
    assertEquals("失败：权限被拒绝", entry.errorText)
    assertEquals("文件操作：unlink", entry.action)
  }

  @Test
  fun parserShowsReadOnlyRuleDenyReason() {
    val raw =
        "2026-06-10 10:20:30.000|com.example.app|com.example.app|open|" +
            "/storage/emulated/0/DCIM/a.jpg|op=open|ret=-1|errno=30|deny_reason=read_only_rule"

    val entry = parseMonitorLogEntries(raw).single()

    assertFalse(entry.ok)
    assertEquals("失败：命中只读模式规则", entry.errorText)
  }

  @Test
  fun keepsReadOnlyFailureSeparate() {
    val raw =
        listOf(
                "2026-06-11 14:35:04|com.android.providers.media.module|xyz.nextalone.nnngram|OPEN|" +
                    "/storage/emulated/0/Download/Nnngram/a.apk|ret=189|errno=0|op=open|op_filter=open:create|flags=0x88042",
                "2026-06-11 14:35:54|com.android.providers.media.module|xyz.nextalone.nnngram|OPEN|" +
                    "/storage/emulated/0/Download/Nnngram/a.apk|ret=-1|errno=30|op=openAssetFile|op_filter=open:create|" +
                    "flags=0x241|from=/storage/emulated/0/Download/第三方下载/Nnngram/a.apk|source=media_provider_open|" +
                    "caller_uid=10312|deny_reason=read_only_rule",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)

    assertEquals(2, entries.size)
    assertFalse(entries[0].ok)
    assertEquals("失败：命中只读模式规则", entries[0].errorText)
    assertTrue(entries[1].ok)
  }

  @Test
  fun ignoresArchiveNameOnly() {
    val raw =
        "2026-06-13 19:16:41|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
            "/storage/emulated/0/Download/storage-redirect-x-logs-20260613-111638.tar.gz|" +
            "ret=0|errno=0|identify_method=owner_uid|identify_reliability=high|op=inotify"

    val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

    assertFalse(entry.isModuleWebUiExport)
    assertEquals("com.android.providers.media.module", entry.packageName)
    assertEquals("Label:com.android.providers.media.module", entry.label)
    assertEquals("inotify", entry.operation)
    assertEquals("创建类文件操作", entry.action)
  }

  @Test
  fun ignoresArchiveProviderFallback() {
    val raw =
        "2026-06-13 19:16:41|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
            "/storage/emulated/0/Download/storage-redirect-x-logs-20260613-111638.tar.gz|" +
            "ret=0|errno=0|identify_method=media_provider_fallback|identify_reliability=fallback|" +
            "op=inotify|source=public_root|watch_package=cn.wps.moffice_eng"

    val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

    assertFalse(entry.isModuleWebUiExport)
    assertEquals("com.android.providers.media.module", entry.packageName)
    assertEquals("Label:com.android.providers.media.module", entry.label)
    assertEquals("inotify", entry.operation)
    assertEquals("media_provider_fallback", entry.identifyMethod)
    assertEquals("cn.wps.moffice_eng", entry.watchPackage)
  }

  @Test
  fun labelsModuleDiagnosticExport() {
    val raw =
        "2026-06-13 19:16:41|storage.redirect.x|storage.redirect.x|OPEN|" +
            "/storage/emulated/0/Download/storage-redirect-x-logs-20260613-111638.tar.gz|" +
            "ret=0|errno=0|identify_method=module_export|identify_reliability=high|" +
            "op=provider_open|op_filter=provider_open:write|source=webui_export|export_kind=diagnostic"

    val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

    assertTrue(entry.isModuleWebUiExport)
    assertEquals("storage.redirect.x", entry.packageName)
    assertEquals("存储重定向X", entry.label)
    assertEquals("export", entry.operation)
    assertEquals("日志包导出", entry.action)
    assertEquals("module_export", entry.identifyMethod)
  }

  @Test
  fun labelsUnattributedIntermediateAsUnknownSource() {
    val raw =
        "2026-11-02 22:30:48|com.android.providers.media.module|-|MKDIR|" +
            "/storage/emulated/0/MIUI/Mirror/MainCache|ret=0|errno=0|" +
            "identify_method=unknown|identify_reliability=none|event_role=request|op=mkdir"

    val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

    assertEquals("com.android.providers.media.module", entry.packageName)
    assertEquals("未知来源", entry.label)
    assertEquals("-", entry.callerPackage)
    assertEquals("unknown", entry.identifyMethod)
    assertEquals("none", entry.identifyReliability)
  }

  @Test
  fun keepsResolvedLabelWhenIntermediateRecordHasCaller() {
    val raw =
        "2026-11-02 22:30:40|com.android.providers.media.module|com.miui.daemon|OPEN|" +
            "/storage/emulated/0/MIUI/debug_log/powerinfo/result_reason|ret=168|errno=0|" +
            "identify_method=caller|identify_reliability=high|event_role=request|" +
            "op=open|op_filter=open:write|flags=0x20002|caller_uid=10000"

    val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

    assertEquals("com.miui.daemon", entry.packageName)
    assertEquals("Label:com.miui.daemon", entry.label)
  }

  @Test
  fun resolvesDotlessNativeIdentityViaCallerUid() {
    // issue #2：MT 管理器 root 复制 .boot_ok 落表为原生 basename mtio，
    // caller_uid=10000 是内核层面的发起方证据，显示层应反查到真实包名。
    val raw =
        "2026-11-02 14:47:58|com.android.providers.media.module|mtio|OPEN|" +
            "/storage/emulated/0/.boot_ok|ret=210|errno=0|" +
            "identify_method=caller|identify_reliability=high|event_role=request|" +
            "op=open|op_filter=open:create|flags=0x20242|caller_uid=10000"

    val entry =
        parseMonitorLogEntries(
                raw,
                callerUidResolver = { uid -> if (uid == 10000) "bin.mt.plus" else null },
            ) {
              "Label:$it"
            }
            .single()

    assertEquals("bin.mt.plus", entry.packageName)
    assertEquals("Label:bin.mt.plus", entry.label)
    assertEquals("caller", entry.identifyMethod)
  }

  @Test
  fun keepsDotlessIdentityWithoutUidResolver() {
    // 无 PackageManager 反查（WebUI 侧）时，保留原生身份展示，不退回媒体组件标签。
    val raw =
        "2026-11-02 14:47:58|com.android.providers.media.module|mtio|OPEN|" +
            "/storage/emulated/0/.boot_ok|ret=210|errno=0|" +
            "identify_method=caller|identify_reliability=high|event_role=request|" +
            "op=open|op_filter=open:create|flags=0x20242|caller_uid=10000"

    val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

    assertEquals("mtio", entry.packageName)
    assertEquals("Label:mtio", entry.label)
  }

  @Test
  fun keepsDotlessIdentityForSharedUidGroup() {
    // 共享 uid 组（多包）无法区分成员：反查返回 null，维持原身份，不猜。
    val raw =
        "2026-11-02 14:47:58|com.android.providers.media.module|mtio|OPEN|" +
            "/storage/emulated/0/.boot_ok|ret=210|errno=0|" +
            "identify_method=caller|identify_reliability=high|event_role=request|" +
            "op=open|op_filter=open:create|flags=0x20242|caller_uid=10000"

    val entry =
        parseMonitorLogEntries(raw, callerUidResolver = { _ -> null }) { "Label:$it" }.single()

    assertEquals("mtio", entry.packageName)
  }

  @Test
  fun ignoresDotlessCallerWhenProcessPackageIsNormalApp() {
    // 落盘进程是普通应用时维持既有优先级：显示进程包，不显示无点原生身份。
    val raw =
        "2026-11-02 14:47:58|com.example.app|sh|OPEN|" +
            "/storage/emulated/0/Download/x|ret=0|errno=0|" +
            "identify_method=caller|identify_reliability=high|event_role=request|" +
            "op=open|op_filter=open:write|caller_uid=1000"

    val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

    assertEquals("com.example.app", entry.packageName)
  }

  @Test
  fun labelsVendorPickerVariantsAsUnknownSource() {
    val raw =
        "2026-11-02 15:19:00|com.coloros.mediapicker|-|MKDIR|" +
            "/storage/emulated/0/Download/update|ret=0|errno=0|" +
            "identify_method=unknown|identify_reliability=none|event_role=request|op=mkdir"

    val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

    assertEquals("未知来源", entry.label)
  }

  @Test
  fun keepsBackupExportAndProviderRequest() {
    val raw =
        listOf(
                "2026-06-24 12:06:52|com.android.providers.media.module|com.android.providers.media.module|OPEN|" +
                    "/storage/emulated/0/Download/storage-redirect-x-backup-20260624-120650.srxbak.zip|" +
                    "ret=186|errno=0|identify_method=unknown|identify_reliability=none|" +
                    "op=open|op_filter=open:create|flags=0x200c2",
                "2026-06-24 12:06:52|storage.redirect.x|storage.redirect.x|OPEN|" +
                    "/storage/emulated/0/Download/storage-redirect-x-backup-20260624-120650.srxbak.zip|" +
                    "ret=0|errno=0|identify_method=module_export|identify_reliability=high|" +
                    "op=provider_open|op_filter=provider_open:write|source=webui_backup|export_kind=backup",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)
    assertEquals(2, entries.size)
    val entry = entries.single { it.packageName == "storage.redirect.x" }

    assertTrue(entry.isModuleWebUiExport)
    assertEquals("storage.redirect.x", entry.packageName)
    assertEquals("存储重定向X", entry.label)
    assertEquals("export", entry.operation)
    assertEquals("备份导出", entry.action)
    assertEquals("module_export", entry.identifyMethod)
  }

  @Test
  fun showsProviderFallbackForUnknownSaf() {
    val raw =
        "2026-06-13 19:16:41|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
            "/storage/emulated/0/Download/export.zip|" +
            "ret=0|errno=0|identify_method=media_provider_fallback|identify_reliability=fallback|" +
            "op=inotify|source=public_root|watch_package=cn.wps.moffice_eng"

    val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

    assertFalse(entry.isModuleWebUiExport)
    assertEquals("com.android.providers.media.module", entry.packageName)
    assertEquals("Label:com.android.providers.media.module", entry.label)
    assertEquals("media_provider_fallback", entry.identifyMethod)
  }

  @Test
  fun parserKeepsManagerAppDiagnosticArchiveAttribution() {
    val raw =
        "2026-06-13 19:16:41|org.srx.manager|org.srx.manager|CREATE|" +
            "/storage/emulated/0/Download/storage-redirect-x-logs-20260613-111638.tar.gz|" +
            "ret=0|errno=0|identify_method=caller|identify_reliability=high|op=open"

    val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

    assertFalse(entry.isModuleWebUiExport)
    assertEquals("org.srx.manager", entry.packageName)
    assertEquals("Label:org.srx.manager", entry.label)
  }

  @Test
  fun keepsManagerArchiveAndProviderRequest() {
    val raw =
        listOf(
                "2026-06-18 18:30:22|com.android.providers.media.module|-|OPEN|" +
                    "/storage/emulated/0/Download/第三方下载/storage-redirect-x-logs-20260618-183022.tar.gz|" +
                    "ret=258|errno=0|identify_method=unknown|identify_reliability=none|op=open|op_filter=open:create|flags=0x200c2",
                "2026-06-18 18:30:22|com.android.externalstorage|org.srx.manager|OPEN|" +
                    "/storage/emulated/0/Download/第三方下载/storage-redirect-x-logs-20260618-183022.tar.gz|" +
                    "ret=122|errno=0|identify_method=caller|identify_reliability=high|op=open|op_filter=open:create|flags=0xc2",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw) { "Label:$it" }

    assertEquals(2, entries.size)
    val entry = entries.single { it.packageName == "org.srx.manager" }
    assertFalse(entry.isModuleWebUiExport)
    assertEquals("org.srx.manager", entry.packageName)
    assertEquals("Label:org.srx.manager", entry.label)
  }

  @Test
  fun keepsMappedArchiveRequests() {
    val raw =
        listOf(
                "2026-06-18 18:30:22|com.android.providers.media.module|-|OPEN|" +
                    "/storage/emulated/0/Download/storage-redirect-x-logs-20260618-183022.tar.gz|" +
                    "ret=258|errno=0|identify_method=unknown|identify_reliability=none|op=open|op_filter=open:create|flags=0x200c2",
                "2026-06-18 18:30:22|com.android.externalstorage|org.srx.manager|OPEN|" +
                    "/storage/emulated/0/Download/第三方下载/storage-redirect-x-logs-20260618-183022.tar.gz|" +
                    "ret=122|errno=0|identify_method=caller|identify_reliability=high|op=open|op_filter=open:create|flags=0xc2",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)

    assertEquals(2, entries.size)
    assertFalse(entries.single { it.packageName == "org.srx.manager" }.isModuleWebUiExport)
    assertEquals(
        "org.srx.manager",
        entries.single { it.packageName == "org.srx.manager" }.packageName,
    )
  }

  @Test
  fun keepsAppExportAndProviderRequest() {
    val raw =
        listOf(
                "2026-06-24 18:30:22|com.android.providers.media.module|-|OPEN|" +
                    "/storage/emulated/0/Download/备份/storage-redirect-x-logs-20260624-183022.tar.gz|" +
                    "ret=258|errno=0|identify_method=unknown|identify_reliability=none|op=open|op_filter=open:create|flags=0x200c2",
                "2026-06-24 18:30:22|org.srx.manager|org.srx.manager|OPEN|" +
                    "/storage/emulated/0/Download/备份/storage-redirect-x-logs-20260624-183022.tar.gz|" +
                    "ret=0|errno=0|identify_method=caller|identify_reliability=high|" +
                    "op=provider_open|op_filter=provider_open:write|source=app_export|export_kind=diagnostic",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw) { "Label:$it" }

    assertEquals(2, entries.size)
    val entry = entries.single { it.packageName == "org.srx.manager" }
    assertFalse(entry.isModuleWebUiExport)
    assertEquals("org.srx.manager", entry.packageName)
    assertEquals("Label:org.srx.manager", entry.label)
    assertEquals("caller", entry.identifyMethod)
  }

  @Test
  fun keepsPrivateOwnerAndDownloadProvider() {
    val raw =
        listOf(
                "2026-06-18 18:30:02|com.android.providers.media.module|com.eg.android.AlipayGphone|OPEN|" +
                    "/storage/emulated/0/Download/搞机备份/模块设置备份/XRadiant_backup_20260618_183000.json|" +
                    "ret=256|errno=0|identify_method=recent_private_owner|identify_reliability=medium|" +
                    "op=open|op_filter=open:create|flags=0x200c2",
                "2026-06-18 18:30:02|com.android.mtp,com.android.providers.downloads,com.android.providers.media,com.android.soundpicker|" +
                    "com.android.providers.downloads|OPEN|" +
                    "/storage/emulated/0/Download/搞机备份/模块设置备份/XRadiant_backup_20260618_183000.json|" +
                    "ret=150|errno=0|identify_method=java_stack|identify_reliability=medium|op=open|op_filter=open:create|flags=0xc2",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)

    assertEquals(2, entries.size)
    assertEquals(
        "com.eg.android.AlipayGphone",
        entries.single { it.packageName == "com.eg.android.AlipayGphone" }.packageName,
    )
    assertEquals(
        "recent_private_owner",
        entries.single { it.packageName == "com.eg.android.AlipayGphone" }.identifyMethod,
    )
  }

  @Test
  fun keepsSafAndAnonymousRequests() {
    val raw =
        listOf(
                "2026-06-23 17:07:53|com.android.providers.media.module|com.eg.android.AlipayGphone|OPEN|" +
                    "/storage/emulated/0/Download/XRadiant_backup_20260623_170751.json|" +
                    "ret=212|errno=0|identify_method=recent_private_owner|identify_reliability=medium|" +
                    "op=open|op_filter=open:create|flags=0x200c2",
                "2026-06-23 17:07:53|com.android.providers.media.module|-|OPEN|" +
                    "/storage/emulated/0/Download/XRadiant_backup_20260623_170751.json|" +
                    "ret=212|errno=0|identify_method=unknown|identify_reliability=none|" +
                    "op=open|op_filter=open:write|flags=0x20202",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)

    assertEquals(2, entries.size)
    assertEquals(
        "com.eg.android.AlipayGphone",
        entries.single { it.packageName == "com.eg.android.AlipayGphone" }.packageName,
    )
    assertEquals(
        "recent_private_owner",
        entries.single { it.packageName == "com.eg.android.AlipayGphone" }.identifyMethod,
    )
  }

  @Test
  fun keepsProviderAndPrivateOwnerRequests() {
    val raw =
        listOf(
                "2026-06-23 17:07:53|com.android.providers.media.module|com.eg.android.AlipayGphone|OPEN|" +
                    "/storage/emulated/0/Download/XRadiant_backup_20260623_170751.json|" +
                    "ret=212|errno=0|identify_method=recent_private_owner|identify_reliability=medium|" +
                    "op=open|op_filter=open:create|flags=0x200c2",
                "2026-06-23 17:07:53|com.android.providers.media.module|com.leo.xposed.xradiant|OPEN|" +
                    "/storage/emulated/0/Download/XRadiant_backup_20260623_170751.json|" +
                    "ret=212|errno=0|identify_method=provider_open|identify_reliability=high|" +
                    "op=open|op_filter=open:write|flags=0x20202",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)
    assertEquals(2, entries.size)
    val entry = entries.single { it.packageName == "com.leo.xposed.xradiant" }

    assertEquals("com.leo.xposed.xradiant", entry.packageName)
    assertEquals("provider_open", entry.identifyMethod)
  }

  @Test
  fun keepsSafAndEarlierProviderRequests() {
    val raw =
        listOf(
                "2026-06-24 12:06:52|com.android.providers.media.module|com.android.providers.media.module|OPEN|" +
                    "/storage/emulated/0/Download/备份/storage-redirect-x-backup-20260624-120650.srxbak.zip|" +
                    "ret=186|errno=0|identify_method=unknown|identify_reliability=none|" +
                    "op=open|op_filter=open:create|flags=0x200c2",
                "2026-06-24 12:06:53|com.android.externalstorage|org.srx.manager|OPEN|" +
                    "/storage/emulated/0/Download/备份/storage-redirect-x-backup-20260624-120650.srxbak.zip|" +
                    "ret=0|errno=0|identify_method=provider_open|identify_reliability=high|" +
                    "op=provider_open|op_filter=provider_open:write|source=saf_provider|caller_uid=10357",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)
    assertEquals(2, entries.size)
    val entry = entries.single { it.packageName == "org.srx.manager" }

    assertEquals("org.srx.manager", entry.packageName)
    assertEquals("org.srx.manager", entry.callerPackage)
    assertEquals("provider_open", entry.identifyMethod)
    assertEquals("provider_open", entry.operation)
    assertEquals("write", entry.operationIntent)
  }

  @Test
  fun keepsProviderOperationSuffixes() {
    val readEntry =
        parseMonitorLogEntries(
                "2026-07-02 21:54:54|com.android.externalstorage|com.yxer.packageinstalles|OPEN|" +
                    "/storage/emulated/0/Download/第三方下载/1DMP/app.apk|" +
                    "ret=0|errno=0|identify_method=saf_provider|identify_reliability=high|" +
                    "op=provider_open|op_filter=provider_open:read|source=saf_provider|caller_uid=10360",
            )
            .single()
    val createEntry =
        parseMonitorLogEntries(
                "2026-07-02 21:58:15|com.android.externalstorage|org.srx.manager|OPEN|" +
                    "/storage/emulated/0/Download/第三方下载/storage-redirect-x-logs-20260702-215719.tar.gz|" +
                    "ret=0|errno=0|identify_method=saf_provider|identify_reliability=high|" +
                    "op=provider_open|op_filter=provider_open:create|source=saf_provider|caller_uid=10266",
            )
            .single()

    assertEquals("provider_open", readEntry.operation)
    assertEquals("read", readEntry.operationIntent)
    assertEquals("provider_open", createEntry.operation)
    assertEquals("create", createEntry.operationIntent)
  }

  @Test
  fun foldsOneSaveBurstIntoThePickerRecord() {
    // 真机实测一次 SAF 保存会留下四条：MP 的 create/write、picker 的 provider_open、
    // 以及模块自己的 FUSE 观察。它们的操作名与调用方都不同，按 key 配对永远配不上，
    // 这里断言按「同一路径 + 时间差不超过 3 秒」折叠成一条，且条数记在 observationCount 上。
    val raw =
        listOf(
            "2026-11-28 00:08:06|com.android.providers.media.module|com.android.externalstorage|OPEN|" +
                "/storage/emulated/0/Download/第三方下载/backup.srxbak.zip|field_encoding=percent|" +
                "ret=197|errno=0|identify_method=caller|identify_reliability=high|event_role=request|" +
                "op=open|op_filter=open:create|flags=0x200c2|action=passthrough",
            "2026-11-28 00:08:06|com.google.android.documentsui|org.srx.manager|OPEN|" +
                "/storage/emulated/0/Download/第三方下载/backup.srxbak.zip|field_encoding=percent|" +
                "ret=0|errno=0|identify_method=saf_provider|identify_reliability=high|" +
                "event_role=request|op=provider_open|op_filter=provider_open:create|" +
                "source=saf_provider|save_group=1|caller_uid=10357",
            "2026-11-28 00:08:08|com.android.providers.media.module|com.android.externalstorage|OPEN|" +
                "/storage/emulated/0/Download/第三方下载/backup.srxbak.zip|field_encoding=percent|" +
                "ret=197|errno=0|identify_method=caller|identify_reliability=high|event_role=request|" +
                "op=open|op_filter=open:write|flags=0x20002|action=passthrough",
            "2026-11-28 00:08:06|org.srx.manager|org.srx.manager|OPEN|" +
                "/storage/emulated/0/Download/第三方下载/backup.srxbak.zip|field_encoding=percent|" +
                "ret=0|errno=0|identify_method=provider_open|identify_reliability=high|" +
                "event_role=observation|op=open:write|source=read_only_path|mask=0x8",
        )
    val entries = parseMonitorLogEntries(raw.joinToString("\n"))

    assertEquals(1, entries.size)
    val entry = entries.single()
    assertEquals("com.google.android.documentsui", entry.processPackage)
    assertEquals("org.srx.manager", entry.callerPackage)
    assertEquals("saf_provider", entry.identifyMethod)
    // 更早的「请求 + 观察」合并会先吃掉一到两条，折叠计数不一定等于原始行数；
    // 这里只断言确实有多次落盘被并进来，UI 上的效果是「一条记录 + 已关联 N 条」。
    assertTrue(
        "expected folded count >= 2, got ${entry.observationCount}",
        entry.observationCount >= 2,
    )
  }

  @Test
  fun keepsEachPickerRecordWhenTwoSavesHappenCloseTogether() {
    // 用户连点两次保存、或两个应用往同一路径各存一份时，两条 saf_provider 记录的时间差
    // 同样落在 3 秒窗口内。锚点只能当折叠目标、不能互相折叠，否则日志里少一次独立保存
    // 的证据（两条记录按请求者身份区分，所以不会被更早的按 key 合并吃掉）。
    val path = "/storage/emulated/0/Download/第三方下载/backup.srxbak.zip"
    val raw =
        listOf(
            "2026-11-28 00:08:06|com.google.android.documentsui|org.srx.manager|OPEN|$path|" +
                "field_encoding=percent|ret=0|errno=0|identify_method=saf_provider|" +
                "identify_reliability=high|event_role=request|op=provider_open|" +
                "op_filter=provider_open:create|source=saf_provider|save_group=1|caller_uid=10357",
            "2026-11-28 00:08:08|com.google.android.documentsui|com.example.browser|OPEN|$path|" +
                "field_encoding=percent|ret=0|errno=0|identify_method=saf_provider|" +
                "identify_reliability=high|event_role=request|op=provider_open|" +
                "op_filter=provider_open:create|source=saf_provider|save_group=2|caller_uid=10400",
        )
    val entries = parseMonitorLogEntries(raw.joinToString("\n"))

    assertEquals(2, entries.size)
    assertTrue(entries.all { it.identifyMethod == "saf_provider" })
    // 列表按时间倒序展示，这里只断言两条请求者身份都在、且各自独立。
    assertEquals(
        setOf("org.srx.manager", "com.example.browser"),
        entries.map { it.callerPackage }.toSet(),
    )
  }

  @Test
  fun keepsPrivateAndAnonymousRequests() {
    val raw =
        listOf(
                "2026-06-23 18:33:31|com.android.providers.media.module|com.leo.xposed.xradiant|OPEN|" +
                    "/storage/emulated/0/Download/XRadiant_backup_20260623_183330.json|" +
                    "ret=206|errno=0|identify_method=recent_private_caller|identify_reliability=medium|" +
                    "op=open|op_filter=open:create|flags=0x200c2",
                "2026-06-23 18:33:31|com.android.providers.media.module|-|OPEN|" +
                    "/storage/emulated/0/Download/XRadiant_backup_20260623_183330.json|" +
                    "ret=208|errno=0|identify_method=unknown|identify_reliability=none|" +
                    "op=open|op_filter=open:write|flags=0x20202",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)
    assertEquals(2, entries.size)
    val entry = entries.single { it.packageName == "com.leo.xposed.xradiant" }

    assertEquals("com.leo.xposed.xradiant", entry.packageName)
    assertEquals("recent_private_caller", entry.identifyMethod)
  }

  @Test
  fun keepsDistinctPrivateCallerRequests() {
    val raw =
        listOf(
                "2026-06-23 18:33:31|com.android.providers.media.module|com.eg.android.AlipayGphone|OPEN|" +
                    "/storage/emulated/0/Download/XRadiant_backup_20260623_183330.json|" +
                    "ret=206|errno=0|identify_method=recent_private_owner|identify_reliability=medium|" +
                    "op=open|op_filter=open:create|flags=0x200c2",
                "2026-06-23 18:33:31|com.android.providers.media.module|com.leo.xposed.xradiant|OPEN|" +
                    "/storage/emulated/0/Download/XRadiant_backup_20260623_183330.json|" +
                    "ret=208|errno=0|identify_method=recent_private_caller|identify_reliability=medium|" +
                    "op=open|op_filter=open:write|flags=0x20202",
            )
            .joinToString("\n")

    val entries = parseMonitorLogEntries(raw)
    assertEquals(2, entries.size)
    val entry = entries.single { it.packageName == "com.leo.xposed.xradiant" }

    assertEquals("com.leo.xposed.xradiant", entry.packageName)
    assertEquals("recent_private_caller", entry.identifyMethod)
  }

  @Test
  fun ignoresAllowedPathWatcherContext() {
    val raw =
        "2026-06-18 04:35:21|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
            "/storage/emulated/0/Download/第三方下载/DLManager/thumbs|" +
            "ret=0|errno=0|identify_method=owner_uid|identify_reliability=high|op=inotify|" +
            "source=allowed_real_path|mask=0x40000100|" +
            "backend=/data/media/0/Download/第三方下载/DLManager/thumbs|watch_package=cn.wps.moffice_eng"

    val entry = parseMonitorLogEntries(raw).single()

    assertEquals("com.android.providers.media.module", entry.packageName)
    assertEquals("cn.wps.moffice_eng", entry.watchPackage)
  }

  @Test
  fun coalescesMappedDownloadBeforeWatchContext() {
    val raw =
        listOf(
                "2026-06-18 04:35:21|com.android.providers.downloads.ui|com.android.providers.downloads.ui|CREATE|" +
                    "/storage/emulated/0/Download/第三方下载/DLManager/thumbs|" +
                    "ret=0|errno=0|identify_method=watch_package|identify_reliability=medium|op=inotify|" +
                    "source=path_mapping|mask=0x40000100|" +
                    "backend=/data/media/0/Download/第三方下载/DLManager/thumbs|" +
                    "from=/storage/emulated/0/Download/DLManager/thumbs",
                "2026-06-18 04:35:21|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
                    "/storage/emulated/0/Download/第三方下载/DLManager/thumbs|" +
                    "ret=0|errno=0|identify_method=owner_uid|identify_reliability=high|op=inotify|" +
                    "source=allowed_real_path|mask=0x40000100|" +
                    "backend=/data/media/0/Download/第三方下载/DLManager/thumbs|watch_package=cn.wps.moffice_eng",
            )
            .joinToString("\n")

    val entry = parseMonitorLogEntries(raw).single()

    assertEquals("com.android.providers.downloads.ui", entry.packageName)
    assertEquals("/storage/emulated/0/Download/DLManager/thumbs", entry.fromPath)
    assertEquals("path_mapping", entry.source)
  }

  @Test
  fun parserAppliesMonitorPathFiltersToBackendPath() {
    val raw =
        "2026-07-03 12:30:41|com.android.mtp,com.android.providers.downloads,com.android.providers.media,com.android.soundpicker|" +
            "com.android.providers.downloads|MKDIR|/storage/emulated/0/.xlDownload|" +
            "ret=0|errno=0|identify_method=java_stack|identify_reliability=medium|op=mkdir|" +
            "backend=/data/media/0/Android/data/com.android.providers.downloads/sdcard/.xlDownload|" +
            "from=/storage/emulated/0/.xlDownload|source=sandbox_path"

    val entries =
        parseMonitorLogEntries(
            raw,
            filters = FileMonitorFilters(excludedPaths = listOf("Android/data")),
        )

    assertTrue(entries.isEmpty())
  }

  @Test
  fun appliesOperationFiltersToExistingLogs() {
    val raw =
        "2026-07-03 12:30:41|com.example.app|com.example.app|OPEN|" +
            "/storage/emulated/0/Download/a.txt|ret=10|errno=0|op=open|op_filter=open:read"

    val entries =
        parseMonitorLogEntries(raw, filters = FileMonitorFilters(excludedPaths = emptyList()))

    assertTrue(entries.isEmpty())
  }

  @Test
  fun preservesRawOperationAndCreateIntent() {
    val raw =
        "2026-07-12 12:34:56|com.example.app|com.example.app|OPEN|" +
            "/storage/emulated/0/Download/a.txt|ret=8|errno=0|" +
            "op=openat2|op_filter=openat2:create|flags=0x40"

    val entry =
        parseMonitorLogEntries(raw, filters = FileMonitorFilters(excludedPaths = emptyList()))
            .single()

    assertEquals("openat2", entry.operation)
    assertEquals("openat2:create", entry.filterOperation)
    assertEquals("create", entry.operationIntent)
    assertTrue(
        parseMonitorLogEntries(
                raw,
                filters =
                    FileMonitorFilters(
                        excludedPaths = emptyList(),
                        excludedOperations = listOf("*:create"),
                    ),
            )
            .isEmpty()
    )
    assertFalse(
        parseMonitorLogEntries(
                raw,
                filters =
                    FileMonitorFilters(
                        excludedPaths = emptyList(),
                        excludedOperations = listOf("open*:read"),
                    ),
            )
            .isEmpty()
    )
  }

  @Test
  fun preservesWildcardFilterSemantics() {
    val raw =
        listOf(
                monitorLine("open:create", "Download/alpha/file.txt"),
                monitorLine("open:read", "Download/beta/file.txt"),
                monitorLine("mkdir", "Android/data/com.example/cache/item"),
            )
            .joinToString("\n")

    val entries =
        parseMonitorLogEntries(
            raw,
            filters =
                FileMonitorFilters(
                    excludedPaths = listOf("Download/al**a/file.?xt", "Android/data/**"),
                    excludedOperations = listOf("**:create"),
                ),
        )

    assertEquals(listOf("/storage/emulated/0/Download/beta/file.txt"), entries.map { it.path })
  }

  private fun monitorLine(operation: String, relativePath: String): String =
      "2026-07-25 12:30:41|com.example.app|com.example.app|OPEN|" +
          "/storage/emulated/0/$relativePath|ret=10|errno=0|op=${operation.substringBefore(':')}|" +
          "op_filter=$operation"
}
