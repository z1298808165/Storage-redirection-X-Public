package org.srx.manager.data

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class MonitorLogParserTest {
    @Test
    fun parserPrefersRealCallerOverIntermediateProviderPackage() {
        val raw = "2026-06-10 10:20:30.000|com.android.providers.media.module|com.example.camera|open|" +
            "file:///storage/emulated/0/DCIM/.pending.js|op=open:create|ret=3|identify_method=caller|identify_reliability=high"

        val entries = parseMonitorLogEntries(raw) { "Label:$it" }

        assertEquals(1, entries.size)
        val entry = entries.single()
        assertEquals("com.example.camera", entry.packageName)
        assertEquals("Label:com.example.camera", entry.label)
        assertEquals("open", entry.operation)
        assertEquals("带创建意图的文件打开", entry.action)
        assertEquals("/storage/emulated/0/DCIM/pending", entry.path)
        assertTrue(entry.ok)
    }

    @Test
    fun parserCoalescesSameMinuteSamePathAndKeepsBestAttribution() {
        val raw = listOf(
            "2026-06-10 10:20:01.000|com.android.providers.media.module|com.android.providers.media.module|open|" +
                "/storage/emulated/0/DCIM/a.jpg|op=open:create|ret=3",
            "2026-06-10 10:20:20.000|com.android.providers.media.module|com.example.camera|open|" +
                "/storage/emulated/10/DCIM/a.jpg|op=open:create|ret=3|from=/data/media/10/DCIM/a.jpg|" +
                "identify_method=caller|identify_reliability=high",
        ).joinToString("\n")

        val entries = parseMonitorLogEntries(raw)

        assertEquals(1, entries.size)
        val entry = entries.single()
        assertEquals("com.example.camera", entry.packageName)
        assertEquals("com.example.camera", entry.callerPackage)
        assertEquals("/data/media/10/DCIM/a.jpg", entry.fromPath)
    }

    @Test
    fun parserCoalescesAppOpenWriteAheadOfMediaProviderFallbackCreate() {
        val raw = listOf(
            "2026-06-23 12:10:04|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
                "/storage/emulated/0/Download/Nnngram/save.jpg|" +
                "ret=0|errno=0|identify_method=media_provider_fallback|identify_reliability=fallback|" +
                "op=inotify|source=public_root|watch_package=xyz.nextalone.nnngram",
            "2026-06-23 12:10:03|xyz.nextalone.nnngram|xyz.nextalone.nnngram|OPEN|" +
                "/storage/emulated/0/Download/Nnngram/save.jpg|" +
                "ret=5|errno=0|identify_method=caller|identify_reliability=high|" +
                "op=open|op_filter=open:write|flags=0x8001",
        ).joinToString("\n")

        val entries = parseMonitorLogEntries(raw) { "Label:$it" }

        assertEquals(1, entries.size)
        val entry = entries.single()
        assertEquals("xyz.nextalone.nnngram", entry.packageName)
        assertEquals("Label:xyz.nextalone.nnngram", entry.label)
        assertEquals("open:write", entry.operation)
        assertEquals("caller", entry.identifyMethod)
    }

    @Test
    fun parserCoalescesFuseCreateAheadOfNormalCreate() {
        val raw = listOf(
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
        ).joinToString("\n")

        val entries = parseMonitorLogEntries(raw) { "Label:$it" }

        assertEquals(1, entries.size)
        val entry = entries.single()
        assertEquals("com.tencent.mobileqq", entry.packageName)
        assertEquals("Label:com.tencent.mobileqq", entry.label)
        assertEquals("create", entry.operation)
        assertEquals("fuse_redirect", entry.identifyMethod)
        assertEquals("fuse_redirect", entry.source)
    }

    @Test
    fun parserCoalescesFuseCreateWithMappedBackendInotifyRecords() {
        val raw = listOf(
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
        ).joinToString("\n")

        val entry = parseMonitorLogEntries(raw).single()

        assertEquals("com.tencent.mobileqq", entry.packageName)
        assertEquals("fuse_redirect", entry.identifyMethod)
        assertEquals("/data/media/0/Download/第三方下载/QQ/storage.redirect.x.zip", entry.backendPath)
    }

    @Test
    fun parserCoalescesProviderOpenWithMappedInotifyAndKeepsRequestPathRecord() {
        val raw = listOf(
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
        ).joinToString("\n")

        val entry = parseMonitorLogEntries(raw).single()

        assertEquals("com.tencent.mm", entry.packageName)
        assertEquals("create", entry.operation)
        assertEquals("watch_package", entry.identifyMethod)
        assertEquals("path_mapping", entry.source)
        assertEquals(
            "/storage/emulated/0/Download/Weixin/storage.redirect.x-v1.2.55-local.zip",
            entry.fromPath,
        )
    }

    @Test
    fun parserDropsMediaStorePendingIntermediateRecords() {
        val raw = listOf(
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
        ).joinToString("\n")

        val entry = parseMonitorLogEntries(raw).single()

        assertEquals("/storage/emulated/0/Download/第三方下载/微信/storage.redirect.x-v1.2.55-local.zip", entry.path)
        assertEquals("/storage/emulated/0/Download/Weixin/storage.redirect.x-v1.2.55-local.zip", entry.fromPath)
        assertEquals("path_mapping", entry.source)
    }

    @Test
    fun parserKeepsMediaStorePendingCommitFinalRecord() {
        val raw = "2026-07-03 08:47:24|com.android.providers.media.module|com.tencent.mm|OPEN|" +
            "/storage/emulated/0/Download/WeiXin/storage-redirect-x-backup-20260619-130840.srxbak.zip|" +
            "ret=0|errno=0|identify_method=caller|identify_reliability=high|op=rename|" +
            "op_filter=open:create|from=/storage/emulated/0/Download/WeiXin/.pending-1783058689-storage-redirect-x-backup-20260619-130840.srxbak.zip|" +
            "source=media_store_pending_commit"

        val entry = parseMonitorLogEntries(raw).single()

        assertEquals("com.tencent.mm", entry.packageName)
        assertEquals("open", entry.operation)
        assertEquals("带创建意图的文件打开", entry.action)
        assertEquals("/storage/emulated/0/Download/WeiXin/storage-redirect-x-backup-20260619-130840.srxbak.zip", entry.path)
        assertEquals("/storage/emulated/0/Download/WeiXin/.pending-1783058689-storage-redirect-x-backup-20260619-130840.srxbak.zip", entry.fromPath)
        assertEquals("media_store_pending_commit", entry.source)
    }

    @Test
    fun parserSkipsMonitorWatchRecordsAndMalformedLines() {
        val raw = listOf(
            "malformed",
            "2026-06-10 10:20:30.000|com.example.app|com.example.app|inotify|/storage/emulated/0/Download/a.txt|op=monitor_watch|ret=0",
            "2026-06-10 10:21:30.000|com.example.app|com.example.app|inotify|/storage/emulated/0/Download/b.txt|op=inotify|ret=0",
        ).joinToString("\n")

        val entries = parseMonitorLogEntries(raw)

        assertEquals(1, entries.size)
        assertEquals("create", entries.single().operation)
        assertEquals("/storage/emulated/0/Download/b.txt", entries.single().path)
    }

    @Test
    fun parserCachesLabelsPerPackage() {
        var resolveCount = 0
        val raw = listOf(
            "2026-06-10 10:20:30.000|com.example.app|com.example.app|mkdir|/storage/emulated/0/Download/a|op=mkdir|ret=0",
            "2026-06-10 10:21:30.000|com.example.app|com.example.app|mkdir|/storage/emulated/0/Download/b|op=mkdir|ret=0",
        ).joinToString("\n")

        val entries = parseMonitorLogEntries(raw) {
            resolveCount += 1
            "Label:$it"
        }

        assertEquals(2, entries.size)
        assertEquals(1, resolveCount)
        assertTrue(entries.all { it.label == "Label:com.example.app" })
    }

    @Test
    fun parserMapsFailedErrnoToReadableText() {
        val raw = "2026-06-10 10:20:30.000|com.example.app|com.example.app|unlink|" +
            "/storage/emulated/0/Download/a.txt|op=unlink|ret=-1|errno=13"

        val entry = parseMonitorLogEntries(raw).single()

        assertFalse(entry.ok)
        assertEquals("失败：权限被拒绝", entry.errorText)
        assertEquals("文件操作：unlink", entry.action)
    }

    @Test
    fun parserShowsReadOnlyRuleDenyReason() {
        val raw = "2026-06-10 10:20:30.000|com.example.app|com.example.app|open|" +
            "/storage/emulated/0/DCIM/a.jpg|op=open|ret=-1|errno=30|deny_reason=read_only_rule"

        val entry = parseMonitorLogEntries(raw).single()

        assertFalse(entry.ok)
        assertEquals("失败：命中只读模式规则", entry.errorText)
    }

    @Test
    fun parserDoesNotCoalesceReadOnlyFailureIntoEarlierSuccess() {
        val raw = listOf(
            "2026-06-11 14:35:04|com.android.providers.media.module|xyz.nextalone.nnngram|OPEN|" +
                "/storage/emulated/0/Download/Nnngram/a.apk|ret=189|errno=0|op=open|op_filter=open:create|flags=0x88042",
            "2026-06-11 14:35:54|com.android.providers.media.module|xyz.nextalone.nnngram|OPEN|" +
                "/storage/emulated/0/Download/Nnngram/a.apk|ret=-1|errno=30|op=openAssetFile|op_filter=open:create|" +
                "flags=0x241|from=/storage/emulated/0/Download/第三方下载/Nnngram/a.apk|source=media_provider_open|" +
                "caller_uid=10312|deny_reason=read_only_rule",
        ).joinToString("\n")

        val entries = parseMonitorLogEntries(raw)

        assertEquals(2, entries.size)
        assertFalse(entries[0].ok)
        assertEquals("失败：命中只读模式规则", entries[0].errorText)
        assertTrue(entries[1].ok)
    }

    @Test
    fun parserDoesNotLabelDiagnosticArchiveFromFileNameOnly() {
        val raw = "2026-06-13 19:16:41|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
            "/storage/emulated/0/Download/storage-redirect-x-logs-20260613-111638.tar.gz|" +
            "ret=0|errno=0|identify_method=owner_uid|identify_reliability=high|op=inotify"

        val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

        assertFalse(entry.isModuleWebUiExport)
        assertEquals("com.android.providers.media.module", entry.packageName)
        assertEquals("Label:com.android.providers.media.module", entry.label)
        assertEquals("create", entry.operation)
        assertEquals("创建类文件操作", entry.action)
    }

    @Test
    fun parserDoesNotLabelDiagnosticArchiveFromMediaProviderFallback() {
        val raw = "2026-06-13 19:16:41|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
            "/storage/emulated/0/Download/storage-redirect-x-logs-20260613-111638.tar.gz|" +
            "ret=0|errno=0|identify_method=media_provider_fallback|identify_reliability=fallback|" +
            "op=inotify|source=public_root|watch_package=cn.wps.moffice_eng"

        val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

        assertFalse(entry.isModuleWebUiExport)
        assertEquals("com.android.providers.media.module", entry.packageName)
        assertEquals("Label:com.android.providers.media.module", entry.label)
        assertEquals("create", entry.operation)
        assertEquals("media_provider_fallback", entry.identifyMethod)
        assertEquals("cn.wps.moffice_eng", entry.watchPackage)
    }

    @Test
    fun parserLabelsExplicitModuleDiagnosticExportAsModule() {
        val raw = "2026-06-13 19:16:41|storage.redirect.x|storage.redirect.x|OPEN|" +
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
    fun parserLabelsExplicitModuleBackupExportAsModule() {
        val raw = listOf(
            "2026-06-24 12:06:52|com.android.providers.media.module|com.android.providers.media.module|OPEN|" +
                "/storage/emulated/0/Download/storage-redirect-x-backup-20260624-120650.srxbak.zip|" +
                "ret=186|errno=0|identify_method=unknown|identify_reliability=none|" +
                "op=open|op_filter=open:create|flags=0x200c2",
            "2026-06-24 12:06:52|storage.redirect.x|storage.redirect.x|OPEN|" +
                "/storage/emulated/0/Download/storage-redirect-x-backup-20260624-120650.srxbak.zip|" +
                "ret=0|errno=0|identify_method=module_export|identify_reliability=high|" +
                "op=provider_open|op_filter=provider_open:write|source=webui_backup|export_kind=backup",
        ).joinToString("\n")

        val entry = parseMonitorLogEntries(raw).single()

        assertTrue(entry.isModuleWebUiExport)
        assertEquals("storage.redirect.x", entry.packageName)
        assertEquals("存储重定向X", entry.label)
        assertEquals("export", entry.operation)
        assertEquals("备份导出", entry.action)
        assertEquals("module_export", entry.identifyMethod)
    }

    @Test
    fun parserShowsMediaProviderFallbackWhenRealSafSourceIsUnknown() {
        val raw = "2026-06-13 19:16:41|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
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
        val raw = "2026-06-13 19:16:41|org.srx.manager|org.srx.manager|CREATE|" +
            "/storage/emulated/0/Download/storage-redirect-x-logs-20260613-111638.tar.gz|" +
            "ret=0|errno=0|identify_method=caller|identify_reliability=high|op=open"

        val entry = parseMonitorLogEntries(raw) { "Label:$it" }.single()

        assertFalse(entry.isModuleWebUiExport)
        assertEquals("org.srx.manager", entry.packageName)
        assertEquals("Label:org.srx.manager", entry.label)
    }

    @Test
    fun parserCoalescesManagerDiagnosticArchiveAheadOfAnonymousWebUiRecord() {
        val raw = listOf(
            "2026-06-18 18:30:22|com.android.providers.media.module|-|OPEN|" +
                "/storage/emulated/0/Download/第三方下载/storage-redirect-x-logs-20260618-183022.tar.gz|" +
                "ret=258|errno=0|identify_method=unknown|identify_reliability=none|op=open|op_filter=open:create|flags=0x200c2",
            "2026-06-18 18:30:22|com.android.externalstorage|org.srx.manager|OPEN|" +
                "/storage/emulated/0/Download/第三方下载/storage-redirect-x-logs-20260618-183022.tar.gz|" +
                "ret=122|errno=0|identify_method=caller|identify_reliability=high|op=open|op_filter=open:create|flags=0xc2",
        ).joinToString("\n")

        val entries = parseMonitorLogEntries(raw) { "Label:$it" }

        assertEquals(1, entries.size)
        val entry = entries.single()
        assertFalse(entry.isModuleWebUiExport)
        assertEquals("org.srx.manager", entry.packageName)
        assertEquals("Label:org.srx.manager", entry.label)
    }

    @Test
    fun parserCoalescesManagerDiagnosticArchiveAcrossMappedDownloadPath() {
        val raw = listOf(
            "2026-06-18 18:30:22|com.android.providers.media.module|-|OPEN|" +
                "/storage/emulated/0/Download/storage-redirect-x-logs-20260618-183022.tar.gz|" +
                "ret=258|errno=0|identify_method=unknown|identify_reliability=none|op=open|op_filter=open:create|flags=0x200c2",
            "2026-06-18 18:30:22|com.android.externalstorage|org.srx.manager|OPEN|" +
                "/storage/emulated/0/Download/第三方下载/storage-redirect-x-logs-20260618-183022.tar.gz|" +
                "ret=122|errno=0|identify_method=caller|identify_reliability=high|op=open|op_filter=open:create|flags=0xc2",
        ).joinToString("\n")

        val entries = parseMonitorLogEntries(raw)

        assertEquals(1, entries.size)
        assertFalse(entries.single().isModuleWebUiExport)
        assertEquals("org.srx.manager", entries.single().packageName)
    }

    @Test
    fun parserCoalescesManagerDiagnosticArchiveAheadOfProviderRecordFromAppExportMarker() {
        val raw = listOf(
            "2026-06-24 18:30:22|com.android.providers.media.module|-|OPEN|" +
                "/storage/emulated/0/Download/备份/storage-redirect-x-logs-20260624-183022.tar.gz|" +
                "ret=258|errno=0|identify_method=unknown|identify_reliability=none|op=open|op_filter=open:create|flags=0x200c2",
            "2026-06-24 18:30:22|org.srx.manager|org.srx.manager|OPEN|" +
                "/storage/emulated/0/Download/备份/storage-redirect-x-logs-20260624-183022.tar.gz|" +
                "ret=0|errno=0|identify_method=caller|identify_reliability=high|" +
                "op=provider_open|op_filter=provider_open:write|source=app_export|export_kind=diagnostic",
        ).joinToString("\n")

        val entries = parseMonitorLogEntries(raw) { "Label:$it" }

        assertEquals(1, entries.size)
        val entry = entries.single()
        assertFalse(entry.isModuleWebUiExport)
        assertEquals("org.srx.manager", entry.packageName)
        assertEquals("Label:org.srx.manager", entry.label)
        assertEquals("caller", entry.identifyMethod)
    }

    @Test
    fun parserPrefersRecentPrivateOwnerExportOverDownloadProviderRecord() {
        val raw = listOf(
            "2026-06-18 18:30:02|com.android.providers.media.module|com.eg.android.AlipayGphone|OPEN|" +
                "/storage/emulated/0/Download/搞机备份/模块设置备份/XRadiant_backup_20260618_183000.json|" +
                "ret=256|errno=0|identify_method=recent_private_owner|identify_reliability=medium|" +
                "op=open|op_filter=open:create|flags=0x200c2",
            "2026-06-18 18:30:02|com.android.mtp,com.android.providers.downloads,com.android.providers.media,com.android.soundpicker|" +
                "com.android.providers.downloads|OPEN|" +
                "/storage/emulated/0/Download/搞机备份/模块设置备份/XRadiant_backup_20260618_183000.json|" +
                "ret=150|errno=0|identify_method=java_stack|identify_reliability=medium|op=open|op_filter=open:create|flags=0xc2",
        ).joinToString("\n")

        val entries = parseMonitorLogEntries(raw)

        assertEquals(1, entries.size)
        assertEquals("com.eg.android.AlipayGphone", entries.single().packageName)
        assertEquals("recent_private_owner", entries.single().identifyMethod)
    }

    @Test
    fun parserCoalescesSafOpenCreateAndAnonymousOpenWriteExport() {
        val raw = listOf(
            "2026-06-23 17:07:53|com.android.providers.media.module|com.eg.android.AlipayGphone|OPEN|" +
                "/storage/emulated/0/Download/XRadiant_backup_20260623_170751.json|" +
                "ret=212|errno=0|identify_method=recent_private_owner|identify_reliability=medium|" +
                "op=open|op_filter=open:create|flags=0x200c2",
            "2026-06-23 17:07:53|com.android.providers.media.module|-|OPEN|" +
                "/storage/emulated/0/Download/XRadiant_backup_20260623_170751.json|" +
                "ret=212|errno=0|identify_method=unknown|identify_reliability=none|" +
                "op=open|op_filter=open:write|flags=0x20202",
        ).joinToString("\n")

        val entries = parseMonitorLogEntries(raw)

        assertEquals(1, entries.size)
        assertEquals("com.eg.android.AlipayGphone", entries.single().packageName)
        assertEquals("recent_private_owner", entries.single().identifyMethod)
    }

    @Test
    fun parserPrefersProviderOpenCallerOverRecentPrivateOwnerExport() {
        val raw = listOf(
            "2026-06-23 17:07:53|com.android.providers.media.module|com.eg.android.AlipayGphone|OPEN|" +
                "/storage/emulated/0/Download/XRadiant_backup_20260623_170751.json|" +
                "ret=212|errno=0|identify_method=recent_private_owner|identify_reliability=medium|" +
                "op=open|op_filter=open:create|flags=0x200c2",
            "2026-06-23 17:07:53|com.android.providers.media.module|com.leo.xposed.xradiant|OPEN|" +
                "/storage/emulated/0/Download/XRadiant_backup_20260623_170751.json|" +
                "ret=212|errno=0|identify_method=provider_open|identify_reliability=high|" +
                "op=open|op_filter=open:write|flags=0x20202",
        ).joinToString("\n")

        val entry = parseMonitorLogEntries(raw).single()

        assertEquals("com.leo.xposed.xradiant", entry.packageName)
        assertEquals("provider_open", entry.identifyMethod)
    }

    @Test
    fun parserPrefersSafProviderOpenOverEarlierMediaProviderCreate() {
        val raw = listOf(
            "2026-06-24 12:06:52|com.android.providers.media.module|com.android.providers.media.module|OPEN|" +
                "/storage/emulated/0/Download/备份/storage-redirect-x-backup-20260624-120650.srxbak.zip|" +
                "ret=186|errno=0|identify_method=unknown|identify_reliability=none|" +
                "op=open|op_filter=open:create|flags=0x200c2",
            "2026-06-24 12:06:53|com.android.externalstorage|org.srx.manager|OPEN|" +
                "/storage/emulated/0/Download/备份/storage-redirect-x-backup-20260624-120650.srxbak.zip|" +
                "ret=0|errno=0|identify_method=provider_open|identify_reliability=high|" +
                "op=provider_open|op_filter=provider_open:write|source=saf_provider|caller_uid=10357",
        ).joinToString("\n")

        val entry = parseMonitorLogEntries(raw).single()

        assertEquals("org.srx.manager", entry.packageName)
        assertEquals("org.srx.manager", entry.callerPackage)
        assertEquals("provider_open", entry.identifyMethod)
        assertEquals("provider_open:write", entry.operation)
    }

    @Test
    fun parserKeepsProviderOpenReadAndCreateOperationSuffixes() {
        val readEntry = parseMonitorLogEntries(
            "2026-07-02 21:54:54|com.android.externalstorage|com.yxer.packageinstalles|OPEN|" +
                "/storage/emulated/0/Download/第三方下载/1DMP/app.apk|" +
                "ret=0|errno=0|identify_method=saf_provider|identify_reliability=high|" +
                "op=provider_open|op_filter=provider_open:read|source=saf_provider|caller_uid=10360",
        ).single()
        val createEntry = parseMonitorLogEntries(
            "2026-07-02 21:58:15|com.android.externalstorage|org.srx.manager|OPEN|" +
                "/storage/emulated/0/Download/第三方下载/storage-redirect-x-logs-20260702-215719.tar.gz|" +
                "ret=0|errno=0|identify_method=saf_provider|identify_reliability=high|" +
                "op=provider_open|op_filter=provider_open:create|source=saf_provider|caller_uid=10266",
        ).single()

        assertEquals("provider_open:read", readEntry.operation)
        assertEquals("provider_open:create", createEntry.operation)
    }

    @Test
    fun parserPrefersRecentPrivateCallerOverAnonymousMediaProviderExport() {
        val raw = listOf(
            "2026-06-23 18:33:31|com.android.providers.media.module|com.leo.xposed.xradiant|OPEN|" +
                "/storage/emulated/0/Download/XRadiant_backup_20260623_183330.json|" +
                "ret=206|errno=0|identify_method=recent_private_caller|identify_reliability=medium|" +
                "op=open|op_filter=open:create|flags=0x200c2",
            "2026-06-23 18:33:31|com.android.providers.media.module|-|OPEN|" +
                "/storage/emulated/0/Download/XRadiant_backup_20260623_183330.json|" +
                "ret=208|errno=0|identify_method=unknown|identify_reliability=none|" +
                "op=open|op_filter=open:write|flags=0x20202",
        ).joinToString("\n")

        val entry = parseMonitorLogEntries(raw).single()

        assertEquals("com.leo.xposed.xradiant", entry.packageName)
        assertEquals("recent_private_caller", entry.identifyMethod)
    }

    @Test
    fun parserPrefersRecentPrivateCallerOverRecentPrivateOwnerExport() {
        val raw = listOf(
            "2026-06-23 18:33:31|com.android.providers.media.module|com.eg.android.AlipayGphone|OPEN|" +
                "/storage/emulated/0/Download/XRadiant_backup_20260623_183330.json|" +
                "ret=206|errno=0|identify_method=recent_private_owner|identify_reliability=medium|" +
                "op=open|op_filter=open:create|flags=0x200c2",
            "2026-06-23 18:33:31|com.android.providers.media.module|com.leo.xposed.xradiant|OPEN|" +
                "/storage/emulated/0/Download/XRadiant_backup_20260623_183330.json|" +
                "ret=208|errno=0|identify_method=recent_private_caller|identify_reliability=medium|" +
                "op=open|op_filter=open:write|flags=0x20202",
        ).joinToString("\n")

        val entry = parseMonitorLogEntries(raw).single()

        assertEquals("com.leo.xposed.xradiant", entry.packageName)
        assertEquals("recent_private_caller", entry.identifyMethod)
    }

    @Test
    fun parserDoesNotAttributeAllowedRealPathWatchContextToWatcherPackage() {
        val raw = "2026-06-18 04:35:21|com.android.providers.media.module|com.android.providers.media.module|CREATE|" +
            "/storage/emulated/0/Download/第三方下载/DLManager/thumbs|" +
            "ret=0|errno=0|identify_method=owner_uid|identify_reliability=high|op=inotify|" +
            "source=allowed_real_path|mask=0x40000100|" +
            "backend=/data/media/0/Download/第三方下载/DLManager/thumbs|watch_package=cn.wps.moffice_eng"

        val entry = parseMonitorLogEntries(raw).single()

        assertEquals("com.android.providers.media.module", entry.packageName)
        assertEquals("cn.wps.moffice_eng", entry.watchPackage)
    }

    @Test
    fun parserCoalescesMappedDownloadProviderRecordAheadOfAllowedWatchContext() {
        val raw = listOf(
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
        ).joinToString("\n")

        val entry = parseMonitorLogEntries(raw).single()

        assertEquals("com.android.providers.downloads.ui", entry.packageName)
        assertEquals("/storage/emulated/0/Download/DLManager/thumbs", entry.fromPath)
        assertEquals("path_mapping", entry.source)
    }

    @Test
    fun parserAppliesMonitorPathFiltersToBackendPath() {
        val raw = "2026-07-03 12:30:41|com.android.mtp,com.android.providers.downloads,com.android.providers.media,com.android.soundpicker|" +
            "com.android.providers.downloads|MKDIR|/storage/emulated/0/.xlDownload|" +
            "ret=0|errno=0|identify_method=java_stack|identify_reliability=medium|op=mkdir|" +
            "backend=/data/media/0/Android/data/com.android.providers.downloads/sdcard/.xlDownload|" +
            "from=/storage/emulated/0/.xlDownload|source=sandbox_path"

        val entries = parseMonitorLogEntries(raw, filters = FileMonitorFilters(excludedPaths = listOf("Android/data")))

        assertTrue(entries.isEmpty())
    }

    @Test
    fun parserAppliesMonitorOperationFiltersWhenDisplayingExistingLogs() {
        val raw = "2026-07-03 12:30:41|com.example.app|com.example.app|OPEN|" +
            "/storage/emulated/0/Download/a.txt|ret=10|errno=0|op=open|op_filter=open:read"

        val entries = parseMonitorLogEntries(raw, filters = FileMonitorFilters(excludedPaths = emptyList()))

        assertTrue(entries.isEmpty())
    }
}
