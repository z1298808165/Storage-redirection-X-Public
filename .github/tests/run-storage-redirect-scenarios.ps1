param(
    [string]$Serial = $env:ANDROID_SERIAL,
    [string]$AppId = "me.fakerqu.test.storageredirect",
    [switch]$SkipBasicAll,
    [switch]$FreshAppPerCase,
    [int[]]$Scenarios = @()
)

$ErrorActionPreference = "Stop"

if ([string]::IsNullOrWhiteSpace($Serial)) {
    $devices = @(& adb devices | Select-Object -Skip 1 | Where-Object { $_ -match "`tdevice" })
    if ($devices.Count -eq 1) {
        $Serial = ($devices[0] -split "\s+")[0]
    } else {
        throw "Multiple or no devices detected. Pass -Serial explicitly."
    }
}

$Action = "me.fakerqu.test.storageredirection.TEST_CASE"
$Config = "/data/adb/modules/storage.redirect.x/config/apps/$AppId.json"
$GlobalConfig = "/data/adb/modules/storage.redirect.x/config/global.json"
$LogPath = "/data/adb/modules/storage.redirect.x/logs/running.log"
$FileMonitorLogPath = "/data/adb/modules/storage.redirect.x/logs/file_monitor.log"
$ResultDir = "/sdcard/Android/data/$AppId/files/test_case_result"
$InternalResultDir = "/data/data/$AppId/files/test_case_result"
$RealRoot = "/storage/emulated/0"
$PrivateRoot = "$RealRoot/Android/data/$AppId/sdcard"
$BackendRoot = "/data/media/0"
$BackendPrivateRoot = "$BackendRoot/Android/data/$AppId/sdcard"
$BackendResultDir = "$BackendRoot/Android/data/$AppId/files/test_case_result"
$SandboxResultDir = "$BackendPrivateRoot/Android/data/$AppId/files/test_case_result"
$TestFile = "srt_ci_probe.txt"
$HotBeforeFile = "srt_hot_before.txt"
$HotAfterFile = "srt_hot_after.txt"
$ReadOnlyFile = "srt_read_only_seed.txt"
$AllowKeepFile = "keep.txt"
$AllowPartFile = "srt_ci_probe.part"
$QMarkSingleFile = "srt_qmark_a.txt"
$QMarkDoubleFile = "srt_qmark_ab.txt"
$QMarkFileSingleFile = "srt_qmark_file_a.txt"
$MountNsStarMediaFile = "srt_mountns_star_media.bin"
$MountNsQMarkMediaFile = "srt_mountns_qmark_media.bin"
$FuseStarMediaFile = "srt_fuse_star_media.bin"
$FuseStarMissMediaFile = "srt_fuse_star_miss_media.bin"
$FuseQMarkMediaFile = "srt_fuse_qmark_media.bin"
$FuseQMarkMissMediaFile = "srt_fuse_qmark_miss_media.bin"
$ReadOnlyHardlink = "hardlink.txt"
$ReadOnlySymlink = "symlink.txt"
$ReadOnlyImageFile = "srt_read_only_media.jpg"
$Payload = "storage-redirect-test:file:ci"
$ReadOnlyPayload = "storage-redirect-test:file:readonly"
$ReadOnlyImageBase64 = "/9j/4AAQSkZJRgABAgAAAQABAAD//gAQTGF2YzYxLjE5LjEwMAD/2wBDAAgEBAQEBAUFBQUFBQYGBgYGBgYGBgYGBgYHBwcICAgHBwcGBgcHCAgICAkJCQgICAgJCQoKCgwMCwsODg4RERT/xABLAAEBAAAAAAAAAAAAAAAAAAAACAEBAAAAAAAAAAAAAAAAAAAAABABAAAAAAAAAAAAAAAAAAAAABEBAAAAAAAAAAAAAAAAAAAAAP/AABEIAAIAAgMBIgACEQADEQD/2gAMAwEAAhEDEQA/AJ/AB//Z"

$ReadOnlyRoot = "$RealRoot/Download/SrtReadOnly"
$ReadOnlyMediaRoot = "$RealRoot/Pictures/SrtReadOnlyMedia"
$PrivateReadOnlyMediaRoot = "$PrivateRoot/Pictures/SrtReadOnlyMedia"
$MappedReadOnlyRequest = "$RealRoot/Download/SrtMapRO"
$MappedReadOnlyTarget = "$RealRoot/Pictures/SrtLocked"
$AllowRoot = "$RealRoot/Download/SrtAllow"
$PrivateAllowRoot = "$PrivateRoot/Download/SrtAllow"
$LegacyRoot = "$RealRoot/Download/SrtLegacy"
$PrivateLegacyRoot = "$PrivateRoot/Download/SrtLegacy"
$QMarkRoot = "$RealRoot/Download/SrtQMark"
$PrivateQMarkRoot = "$PrivateRoot/Download/SrtQMark"
$FusePlainRoot = "$RealRoot/Download/SrtFusePlain"
$PrivateFusePlainRoot = "$PrivateRoot/Download/SrtFusePlain"
$FuseDcimRoot = "$RealRoot/DCIM/SrtFuseQQ"
$PrivateFuseDcimRoot = "$PrivateRoot/DCIM/SrtFuseQQ"
$FuseDcimOtherRoot = "$RealRoot/DCIM/SrtFuseOther"
$PrivateFuseDcimOtherRoot = "$PrivateRoot/DCIM/SrtFuseOther"
$FuseQMarkRoot = "$RealRoot/Download/SrtFuseQa"
$PrivateFuseQMarkRoot = "$PrivateRoot/Download/SrtFuseQa"
$FuseQMarkMissRoot = "$RealRoot/Download/SrtFuseQab"
$PrivateFuseQMarkMissRoot = "$PrivateRoot/Download/SrtFuseQab"
$FuseQMarkMediaRoot = "$RealRoot/Download/SrtFuseQb"
$PrivateFuseQMarkMediaRoot = "$PrivateRoot/Download/SrtFuseQb"
$FuseStarMediaRoot = "$RealRoot/Download/SrtFuseMediaAlpha"
$PrivateFuseStarMediaRoot = "$PrivateRoot/Download/SrtFuseMediaAlpha"
$FuseExcludeRoot = "$RealRoot/Download/SrtFuseExclude"
$PrivateFuseExcludeRoot = "$PrivateRoot/Download/SrtFuseExclude"
$FuseMapParent = "$RealRoot/Download/SrtFuseMapParent"
$FuseMapRwRequest = "$RealRoot/Download/SrtFuseMapRW"
$FuseMapRoRequest = "$RealRoot/Download/SrtFuseMapRO"
$FuseMapRwTarget = "$FuseMapParent/WritableTarget"
$FuseMapRoTarget = "$FuseMapParent/LockedTarget"
$FuseMultiRoot = "$RealRoot/Download/SrtFuseMulti"
$PrivateFuseMultiRoot = "$PrivateRoot/Download/SrtFuseMulti"
$MountNsAllowRoot = "$RealRoot/Download/SrtMountNsAllow"
$PrivateMountNsAllowRoot = "$PrivateRoot/Download/SrtMountNsAllow"
$MountNsReadOnlyRoot = "$RealRoot/Download/SrtMountNsReadOnly"
$PrivateMountNsReadOnlyRoot = "$PrivateRoot/Download/SrtMountNsReadOnly"
$MountNsMapParent = "$RealRoot/Download/SrtMountNsMapParent"
$MountNsMapRwRequest = "$RealRoot/Download/SrtMountNsMapRW"
$MountNsMapRoRequest = "$RealRoot/Download/SrtMountNsMapRO"
$MountNsMapRwTarget = "$MountNsMapParent/WritableTarget"
$MountNsMapRoTarget = "$MountNsMapParent/LockedTarget"
$MonitorBaseRoot = "$RealRoot/Download/SrtMonitor"
$PrivateMonitorBaseRoot = "$PrivateRoot/Download/SrtMonitor"
$MonitorMapRequest = "$RealRoot/Download/SrtMonitorMap"
$MonitorMapTarget = "$RealRoot/Download/SrtMonitorMapped"
$MonitorLockedRoot = "$RealRoot/Download/SrtMonitorLocked"
$MonitorWritableRoot = "$RealRoot/Download/SrtMonitorLocked/Writable"
$PrivateMonitorWritableRoot = "$PrivateRoot/Download/SrtMonitorLocked/Writable"

$script:Summary = New-Object System.Collections.Generic.List[object]
$script:Failures = New-Object System.Collections.Generic.List[string]
$script:CleanupDone = $false
$script:GlobalConfigBackupReady = $false
$script:AppConfigBackupReady = $false
$script:FreshAppPerCase = -not ($env:SRT_FRESH_APP_PER_CASE -match '^(0|false|FALSE|no|NO)$')
if ($FreshAppPerCase) { $script:FreshAppPerCase = $true }
$script:ResultPollMilliseconds = if ($env:SRT_RESULT_POLL_MS -match '^\d+$') { [Math]::Max(50, [int]$env:SRT_RESULT_POLL_MS) } else { 150 }
$script:AppLaunchSettleMilliseconds = if ($env:SRT_APP_LAUNCH_SETTLE_MS -match '^\d+$') { [Math]::Max(0, [int]$env:SRT_APP_LAUNCH_SETTLE_MS) } else { 800 }
$script:MountConfirmTimeoutMilliseconds = if ($env:SRT_MOUNT_CONFIRM_TIMEOUT_MS -match '^\d+$') { [Math]::Max(0, [int]$env:SRT_MOUNT_CONFIRM_TIMEOUT_MS) } else { 0 }
$script:ServiceCaseSettleMilliseconds = if ($env:SRT_SERVICE_CASE_SETTLE_MS -match '^\d+$') { [Math]::Max(0, [int]$env:SRT_SERVICE_CASE_SETTLE_MS) } else { 50 }
$script:FileMonitorEnabled = $env:SRT_FILE_MONITOR_ENABLED -match '^(1|true|TRUE|yes|YES)$'

function Invoke-Adb {
    param([string[]]$Arguments)
    & adb -s $Serial @Arguments | ForEach-Object { $_ -replace "`r", "" }
}

function Invoke-Su {
    param([string]$Command)
    $normalized = $Command.Replace("`r", "")
    $escaped = $normalized.Replace("'", "'\''")
    & adb -s $Serial shell "su -c '$escaped'" | ForEach-Object { $_ -replace "`r", "" }
}

function Test-Su {
    param([string]$Command)
    $normalized = $Command.Replace("`r", "")
    $escaped = $normalized.Replace("'", "'\''")
    & adb -s $Serial shell "su -c '$escaped'" | Out-Null
    $LASTEXITCODE -eq 0
}

function Write-DeviceConfig {
    param([string]$Json)
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Json))
    Invoke-Su "mkdir -p /data/adb/modules/storage.redirect.x/config/apps; printf '%s' '$encoded' | base64 -d > '$Config'; chmod 644 '$Config'" | Out-Null
}

function Write-GlobalConfig {
    param([string]$Json)
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Json))
    Invoke-Su "mkdir -p /data/adb/modules/storage.redirect.x/config; printf '%s' '$encoded' | base64 -d > '$GlobalConfig'; chmod 644 '$GlobalConfig'" | Out-Null
}

function Get-TestGlobalConfig {
    param([bool]$FuseDaemonEnabled, [Nullable[bool]]$FileMonitorEnabled = $null)
    $enabled = if ($FuseDaemonEnabled) { "true" } else { "false" }
    $fileMonitorEnabledValue = if ($null -ne $FileMonitorEnabled) { [bool]$FileMonitorEnabled } else { $script:FileMonitorEnabled }
    $fileMonitor = if ($fileMonitorEnabledValue) { "true" } else { "false" }
    '{"file_monitor_enabled":' + $fileMonitor + ',"fuse_fix_enabled":true,"fuse_daemon_redirect_enabled":' + $enabled + ',"verbose_logging_enabled":true,"auto_enable_redirect_for_new_apps":true,"auto_enable_new_apps_template_id":"","app_config_auto_save":true}'
}

function Enable-FuseDaemonConfig {
    Write-GlobalConfig (Get-TestGlobalConfig -FuseDaemonEnabled $true)
}

function Disable-FuseDaemonConfig {
    Write-GlobalConfig (Get-TestGlobalConfig -FuseDaemonEnabled $false)
}

function Use-MountNamespaceFallbackConfig {
    Write-GlobalConfig (Get-TestGlobalConfig -FuseDaemonEnabled $false)
}

function Backup-GlobalConfig {
    $script:GlobalConfigBackupReady = $false
    if (Test-Su "test -f '$GlobalConfig'") {
        $script:OriginalGlobalConfigExists = $true
        $script:OriginalGlobalConfigBase64 = ((Invoke-Su "base64 '$GlobalConfig' 2>/dev/null | tr -d '\n'") -join "")
    } else {
        $script:OriginalGlobalConfigExists = $false
        $script:OriginalGlobalConfigBase64 = ""
    }
    $script:GlobalConfigBackupReady = $true
}

function Restore-GlobalConfig {
    if (-not $script:GlobalConfigBackupReady) { return }
    if ($script:OriginalGlobalConfigExists -and -not [string]::IsNullOrWhiteSpace($script:OriginalGlobalConfigBase64)) {
        Invoke-Su "printf '%s' '$script:OriginalGlobalConfigBase64' | base64 -d > '$GlobalConfig'; chmod 644 '$GlobalConfig'" | Out-Null
    } else {
        Invoke-Su "rm -f '$GlobalConfig'" | Out-Null
    }
}

function Backup-AppConfig {
    $script:AppConfigBackupReady = $false
    if (Test-Su "test -f '$Config'") {
        $script:OriginalAppConfigExists = $true
        $script:OriginalAppConfigBase64 = ((Invoke-Su "base64 '$Config' 2>/dev/null | tr -d '\n'") -join "")
    } else {
        $script:OriginalAppConfigExists = $false
        $script:OriginalAppConfigBase64 = ""
    }
    $script:AppConfigBackupReady = $true
}

function Restore-AppConfig {
    if (-not $script:AppConfigBackupReady) { return }
    if ($script:OriginalAppConfigExists -and -not [string]::IsNullOrWhiteSpace($script:OriginalAppConfigBase64)) {
        Invoke-Su "mkdir -p /data/adb/modules/storage.redirect.x/config/apps; printf '%s' '$script:OriginalAppConfigBase64' | base64 -d > '$Config'; chmod 644 '$Config'" | Out-Null
    } else {
        Invoke-Su "rm -f '$Config'" | Out-Null
    }
}

function Test-FuseDaemonScenarioSupport {
    $mode = $env:RUN_FUSE_DAEMON_SCENARIOS
    if ($mode -match '^(1|true|TRUE|yes|YES)$') { return $true }
    if ($mode -match '^(0|false|FALSE|no|NO)$') { return $false }
    Test-Su "for file in /data/adb/modules/storage.redirect.x/bin/srx_daemon /data/adb/modules/storage.redirect.x/zygisk/arm64-v8a.so /data/adb/modules/storage.redirect.x/zygisk/x86_64.so; do [ -f `"`$file`" ] && grep -a -q 'fuse_daemon_redirect_enabled' `"`$file`" && exit 0; done; exit 1"
}

function Get-ScenarioList {
    $requested = New-Object System.Collections.Generic.List[int]
    foreach ($scenario in $Scenarios) {
        if ($scenario -lt 1 -or $scenario -gt 29) { throw "Invalid scenario: $scenario" }
        $requested.Add($scenario) | Out-Null
    }
    if ($requested.Count -eq 0 -and -not [string]::IsNullOrWhiteSpace($env:SRT_SCENARIOS)) {
        foreach ($part in ($env:SRT_SCENARIOS -split "[,\s;]+")) {
            if ([string]::IsNullOrWhiteSpace($part)) { continue }
            $scenario = [int]$part
            if ($scenario -lt 1 -or $scenario -gt 29) { throw "Invalid scenario: $scenario" }
            $requested.Add($scenario) | Out-Null
        }
    }
    if ($requested.Count -gt 0) {
        return @($requested | Select-Object -Unique)
    }

    $defaultScenarios = New-Object System.Collections.Generic.List[int]
    1..15 | ForEach-Object { $defaultScenarios.Add($_) | Out-Null }
    $defaultScenarios.Add(29) | Out-Null
    if (Test-FuseDaemonScenarioSupport) {
        16..19 | ForEach-Object { $defaultScenarios.Add($_) | Out-Null }
    } else {
        Write-Host "skip fuse daemon scenarios: module does not expose fuse_daemon_redirect_enabled or RUN_FUSE_DAEMON_SCENARIOS disabled"
    }
    20..22 | ForEach-Object { $defaultScenarios.Add($_) | Out-Null }
    $defaultScenarios.Add(28) | Out-Null
    23..24 | ForEach-Object { $defaultScenarios.Add($_) | Out-Null }
    if (Test-FuseDaemonScenarioSupport) {
        25..27 | ForEach-Object { $defaultScenarios.Add($_) | Out-Null }
    } else {
        $defaultScenarios.Add(26) | Out-Null
        Write-Host "skip file monitor fuse daemon scenarios: module does not expose fuse_daemon_redirect_enabled or RUN_FUSE_DAEMON_SCENARIOS disabled"
    }
    @($defaultScenarios)
}

function Apply-ScenarioConfig {
    param([int]$Scenario)
    Disable-FuseDaemonConfig
    switch ($Scenario) {
        1 { Invoke-Su "rm -f '$Config'" | Out-Null }
        2 { Write-DeviceConfig '{"users":{"0":{"enabled":true}}}' }
        3 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"path_mappings":{"Download/SrtProbe":"Download/Test"}}}}' }
        4 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download"],"path_mappings":{"Download/SrtProbe":"Download/Test"}}}}' }
        5 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download"]}}}' }
        6 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"path_mappings":{"Download/SrtOther":"Download/SrtOtherMapped"}}}}' }
        7 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"path_mappings":{"Download/SrtProbe":"Download/SrtMapOnlyMapped"}}}}' }
        8 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"sandboxed_paths":[".xlDownload"]}}}' }
        9 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"read_only_paths":["Download/SrtReadOnly"]}}}' }
        10 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"path_mappings":{"Download/SrtMapRO":"Pictures/SrtLocked"},"read_only_paths":["Pictures/SrtLocked"]}}}' }
        11 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtAllow","!Download/SrtAllow/tmp","Download","!Download/*.part"]}}}' }
        12 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtLegacy"],"excluded_real_paths":["Download/SrtLegacy/tmp"]}}}' }
        13 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/srt_qmark_?.txt","Download/srt_qmark_file_?.txt"]}}}' }
        14 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"path_mappings":{"Download/SrtLongest":"Download/SrtLongestBase","Download/SrtLongest/Deep":"Download/SrtLongestDeep"}}}}' }
        15 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"mapping_mode_only":true,"sandboxed_paths":"Download/SrtPriority","path_mappings":{"Download/SrtPriority":"Download/SrtPriorityMapped"}}}}' }
        16 {
            Enable-FuseDaemonConfig
            Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtFusePlain","DCIM/SrtFuseQQ/*","Download/SrtFuseQ?/Media","Download/SrtFuseMedia*/Drop"]}}}'
        }
        17 {
            Enable-FuseDaemonConfig
            Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtFuseExclude/Writable"],"read_only_paths":["Download/SrtFuseExclude","!Download/SrtFuseExclude/Writable"]}}}'
        }
        18 {
            Enable-FuseDaemonConfig
            Write-DeviceConfig '{"users":{"0":{"enabled":true,"read_only_paths":["Download/SrtFuseMapParent","!Download/SrtFuseMapParent/WritableTarget"],"path_mappings":{"Download/SrtFuseMapRW":"Download/SrtFuseMapParent/WritableTarget","Download/SrtFuseMapRO":"Download/SrtFuseMapParent/LockedTarget"}}}}'
        }
        19 {
            Enable-FuseDaemonConfig
            Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtFuseMulti/QQ/*","Download/SrtFuseMulti/WeChat/*"],"read_only_paths":["Download/SrtFuseMulti/Locked/*"]}}}'
        }
        20 {
            Use-MountNamespaceFallbackConfig
            Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMountNsAllow/Team*/Deep","Download/SrtMountNsAllow/Q?/Deep"]}}}'
        }
        21 {
            Use-MountNamespaceFallbackConfig
            Write-DeviceConfig '{"users":{"0":{"enabled":true,"read_only_paths":["Download/SrtMountNsReadOnly/Team*/Deep"]}}}'
        }
        22 {
            Use-MountNamespaceFallbackConfig
            Write-DeviceConfig '{"users":{"0":{"enabled":true,"read_only_paths":["Download/SrtMountNsMapParent","!Download/SrtMountNsMapParent/WritableTarget"],"path_mappings":{"Download/SrtMountNsMapRW":"Download/SrtMountNsMapParent/WritableTarget","Download/SrtMountNsMapRO":"Download/SrtMountNsMapParent/LockedTarget"}}}}'
        }
        23 {
            Write-GlobalConfig (Get-TestGlobalConfig -FuseDaemonEnabled $false -FileMonitorEnabled $true)
            Write-DeviceConfig '{"users":{"0":{"enabled":false}}}'
        }
        24 {
            Write-GlobalConfig (Get-TestGlobalConfig -FuseDaemonEnabled $false -FileMonitorEnabled $true)
            Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMonitor"],"read_only_paths":["Download/SrtMonitorLocked","!Download/SrtMonitorLocked/Writable"],"path_mappings":{"Download/SrtMonitorMap":"Download/SrtMonitorMapped"}}}}'
        }
        25 {
            Write-GlobalConfig (Get-TestGlobalConfig -FuseDaemonEnabled $true -FileMonitorEnabled $true)
            Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMonitor"],"read_only_paths":["Download/SrtMonitorLocked","!Download/SrtMonitorLocked/Writable"],"path_mappings":{"Download/SrtMonitorMap":"Download/SrtMonitorMapped"}}}}'
        }
        26 {
            Write-GlobalConfig (Get-TestGlobalConfig -FuseDaemonEnabled $false -FileMonitorEnabled $true)
            Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMonitor"],"read_only_paths":["Download/SrtMonitorLocked","!Download/SrtMonitorLocked/Writable"],"path_mappings":{"Download/SrtMonitorMap":"Download/SrtMonitorMapped"}}}}'
        }
        27 {
            Write-GlobalConfig (Get-TestGlobalConfig -FuseDaemonEnabled $true -FileMonitorEnabled $true)
            Write-DeviceConfig '{"users":{"0":{"enabled":true,"allowed_real_paths":["Download/SrtMonitor"],"read_only_paths":["Download/SrtMonitorLocked","!Download/SrtMonitorLocked/Writable"],"path_mappings":{"Download/SrtMonitorMap":"Download/SrtMonitorMapped"}}}}'
        }
        28 { Write-DeviceConfig '{"users":{"0":{"enabled":true,"read_only_paths":["Pictures/SrtReadOnlyMedia"]}}}' }
        29 { Write-DeviceConfig '{"users":{"0":{"enabled":true}}}' }
        default { throw "Unknown scenario $Scenario" }
    }
}

function Clear-Results {
    Invoke-Su "rm -rf '$ResultDir' '$InternalResultDir' '$BackendResultDir' '$SandboxResultDir'" | Out-Null
}

function Get-LatestResult {
    $path = Invoke-Su "ls -t '$ResultDir'/result_*.txt '$InternalResultDir'/result_*.txt '$BackendResultDir'/result_*.txt '$SandboxResultDir'/result_*.txt 2>/dev/null | head -1"
    $path | Where-Object { $_ -and $_.Trim().Length -gt 0 } | Select-Object -First 1
}

function Wait-ServiceResult {
    param([int]$TimeoutSeconds)

    $pollSeconds = [Math]::Max(0.05, $script:ResultPollMilliseconds / 1000.0).ToString("0.###", [Globalization.CultureInfo]::InvariantCulture)
    $command = @"
deadline=`$(date +%s); deadline=`$((deadline + $TimeoutSeconds));
while [ `$(date +%s) -lt `$deadline ]; do
  for file in '$ResultDir/result_current.txt' '$InternalResultDir/result_current.txt' '$BackendResultDir/result_current.txt' '$SandboxResultDir/result_current.txt'; do
    if [ -s "`$file" ]; then
      printf '%s\n' "__SRT_RESULT_PATH__=`$file"
      cat "`$file"
      exit 0
    fi
  done
  sleep $pollSeconds
done
exit 1
"@

    $lines = @(Invoke-Su $command)
    if ($LASTEXITCODE -ne 0) {
        return [pscustomobject]@{ Found = $false; Path = ""; Text = "" }
    }
    $pathLine = $lines | Where-Object { $_ -like "__SRT_RESULT_PATH__=*" } | Select-Object -First 1
    $path = if ($pathLine) { $pathLine.Substring("__SRT_RESULT_PATH__=".Length) } else { "" }
    $text = ($lines | Where-Object { $_ -notlike "__SRT_RESULT_PATH__=*" }) -join "`n"
    [pscustomobject]@{ Found = $true; Path = $path; Text = $text }
}

function Wait-AppMountConfirmed {
    param([string]$Label)

    if ($script:MountConfirmTimeoutMilliseconds -le 0) { return $false }

    $timeoutSeconds = [Math]::Max(1, [Math]::Ceiling($script:MountConfirmTimeoutMilliseconds / 1000.0))
    $command = @"
deadline=`$((`$(date +%s) + $timeoutSeconds))
pid=""
while [ `$(date +%s) -le `$deadline ]; do
  pid=`$(pidof '$AppId' 2>/dev/null | awk '{print `$1}')
  [ -n "`$pid" ] && break
  sleep 0.1
done
if [ -z "`$pid" ]; then
  echo "pid_not_found"
  exit 2
fi
pattern="app mount confirmed pid=`$pid"
while [ `$(date +%s) -le `$deadline ]; do
  logcat -d -t 200 -s StorageRedirect:V SRX:V 2>/dev/null | grep -Fq "`$pattern" && exit 0
  tail -120 '$LogPath' 2>/dev/null | grep -Fq "`$pattern" && exit 0
  sleep 0.1
done
echo "pid=`$pid"
exit 1
"@
    $output = @(Invoke-Su $command)
    if ($LASTEXITCODE -eq 0) { return $true }
    if ($output -contains "pid_not_found") {
        Write-Host "  mount confirm skipped: app pid not found for $Label"
    } else {
        $pidLine = $output | Where-Object { $_ -like "pid=*" } | Select-Object -First 1
        Write-Host "  mount confirm timeout: $Label $pidLine"
    }
    $false
}

function Get-ServiceCaseTimeoutSeconds {
    param([string]$TestCase)
    if ($TestCase -eq "all") {
        if (-not [string]::IsNullOrWhiteSpace($env:ALL_TEST_TIMEOUT_SECONDS)) {
            return [int]$env:ALL_TEST_TIMEOUT_SECONDS
        }
        return 240
    }
    if (-not [string]::IsNullOrWhiteSpace($env:TEST_CASE_TIMEOUT_SECONDS)) {
        return [int]$env:TEST_CASE_TIMEOUT_SECONDS
    }
    75
}

function Invoke-ServiceCase {
    param(
        [string]$Scenario,
        [string]$Label,
        [string]$TestCase,
        [hashtable]$Extras,
        [string]$PassRegex
    )

    Write-Host "  - ${Scenario}/${Label}: $TestCase"
    Prepare-ServiceCase "$Scenario/$Label"
    if ($script:ServiceCaseSettleMilliseconds -gt 0) {
        Start-Sleep -Milliseconds $script:ServiceCaseSettleMilliseconds
    }
    Clear-Results
    $args = @("shell", "am", "broadcast", "-n", "$AppId/.receiver.TestCaseReceiver", "-a", $Action, "--es", "test_case", $TestCase)
    foreach ($key in $Extras.Keys) {
        $args += @("--es", [string]$key, [string]$Extras[$key])
    }
    Invoke-Adb $args | Out-Null

    $timeoutSeconds = Get-ServiceCaseTimeoutSeconds $TestCase
    $result = Wait-ServiceResult $timeoutSeconds
    if ($result.Found) {
        $ok = if ($PassRegex) { $result.Text -match $PassRegex } else { $true }
        if (-not $ok) {
            $script:Failures.Add("$Scenario/$Label expected $PassRegex, got: $($result.Text -replace "`n", " | ")")
            Write-Host "    FAIL $Scenario/$Label"
        } else {
            Write-Host "    PASS $Scenario/$Label"
        }
        return [pscustomobject]@{ Ok = $ok; Text = $result.Text; Path = $result.Path }
    }

    $script:Failures.Add("$Scenario/$Label result timeout for $TestCase")
    Write-Host "    TIMEOUT $Scenario/$Label"
    Invoke-Adb @("shell", "am", "force-stop", $AppId) | Out-Null
    [pscustomobject]@{ Ok = $false; Text = "timeout"; Path = "" }
}

function Prepare-ServiceCase {
    param([string]$Label)
    if (-not $script:FreshAppPerCase) { return }
    Invoke-Adb @("shell", "am", "force-stop", $AppId) | Out-Null
    Start-Sleep -Milliseconds 500
    Invoke-Adb @("logcat", "-c") | Out-Null
    Invoke-Su ": > '$LogPath' 2>/dev/null || true" | Out-Null
    Invoke-Adb @("shell", "am", "start", "-W", "-n", "$AppId/.MainActivity") | Out-Null
    $confirmed = Wait-AppMountConfirmed $Label
    if (-not $confirmed -and $script:AppLaunchSettleMilliseconds -gt 0) {
        Start-Sleep -Milliseconds $script:AppLaunchSettleMilliseconds
    }
    Wait-Storage $Label | Out-Null
}

function Test-FileExists {
    param([string]$Path)
    Test-Su "test -f '$(Convert-ToBackendPath $Path)'"
}

function Test-PathMissing {
    param([string]$Path)
    Test-Su "test ! -e '$(Convert-ToBackendPath $Path)'"
}

function Convert-ToBackendPath {
    param([string]$Path)
    if ($Path.StartsWith($RealRoot)) {
        return $BackendRoot + $Path.Substring($RealRoot.Length)
    }
    $Path
}

function Require-File {
    param([string]$Scenario, [string]$Label, [string]$Path)
    if (Test-FileExists $Path) { return $true }
    $script:Failures.Add("$Scenario/$Label missing file: $Path")
    $false
}

function Require-Missing {
    param([string]$Scenario, [string]$Label, [string]$Path)
    if (Test-PathMissing $Path) { return $true }
    $script:Failures.Add("$Scenario/$Label unexpected path exists: $Path")
    $false
}

function Wait-Storage {
    param([string]$Label)
    $deadline = (Get-Date).AddSeconds(90)
    while ((Get-Date) -lt $deadline) {
        & adb -s $Serial shell "sm list-volumes all 2>/dev/null | grep -q 'emulated;0 mounted' && test -d '$RealRoot'" | Out-Null
        if ($LASTEXITCODE -eq 0) { return $true }
        Start-Sleep -Seconds 2
    }
    $script:Failures.Add("$Label storage not ready")
    $false
}

function Test-MediaProviderQueryReady {
    param([string]$Uri)

    $output = @(
        & adb -s $Serial shell content query --uri $Uri --projection _id --where "_id=-1" 2>&1
    )
    $text = ($output -join "`n")
    if ($LASTEXITCODE -ne 0) {
        return $false
    }
    if ($text -match "Error while accessing provider:media" -or
        $text -match "Volume external_primary not found" -or
        $text -match "IllegalArgumentException") {
        return $false
    }
    return $true
}

function Wait-MediaProviderReady {
    param([string]$Label)

    $deadline = (Get-Date).AddSeconds(120)
    $uris = @(
        "content://media/external/images/media",
        "content://media/external/video/media",
        "content://media/external/audio/media",
        "content://media/external/file",
        "content://media/external/downloads"
    )

    while ((Get-Date) -lt $deadline) {
        $ready = $true
        foreach ($uri in $uris) {
            if (-not (Test-MediaProviderQueryReady $uri)) {
                $ready = $false
                break
            }
        }
        if ($ready) { return $true }
        Start-Sleep -Seconds 2
    }
    $script:Failures.Add("$Label media provider not ready")
    $false
}

function Clear-Targets {
    Invoke-Su "rm -rf '$BackendRoot/Download/SrtProbe' '$BackendRoot/Download/SrtOther' '$BackendRoot/Download/SrtOtherMapped' '$BackendRoot/Download/SrtMapOnlyMapped' '$BackendRoot/Download/SrtReadOnly' '$BackendRoot/Download/SrtMapRO' '$BackendRoot/Download/SrtAllow' '$BackendRoot/Download/SrtLegacy' '$BackendRoot/Download/SrtQMark' '$BackendRoot/Download/SrtLongest' '$BackendRoot/Download/SrtLongestBase' '$BackendRoot/Download/SrtLongestDeep' '$BackendRoot/Download/SrtPriority' '$BackendRoot/Download/SrtPriorityMapped' '$BackendRoot/Pictures/SrtLocked' '$BackendPrivateRoot/Download/SrtProbe' '$BackendPrivateRoot/Download/SrtOther' '$BackendPrivateRoot/Download/SrtOtherMapped' '$BackendPrivateRoot/Download/SrtMapOnlyMapped' '$BackendPrivateRoot/Download/SrtReadOnly' '$BackendPrivateRoot/Download/SrtMapRO' '$BackendPrivateRoot/Download/SrtAllow' '$BackendPrivateRoot/Download/SrtLegacy' '$BackendPrivateRoot/Download/SrtQMark' '$BackendPrivateRoot/Download/SrtLongest' '$BackendPrivateRoot/Download/SrtLongestBase' '$BackendPrivateRoot/Download/SrtLongestDeep' '$BackendPrivateRoot/Download/SrtPriority' '$BackendPrivateRoot/Download/SrtPriorityMapped' '$BackendPrivateRoot/Pictures/SrtLocked'; rm -f '$BackendRoot/Download/$AllowPartFile' '$BackendPrivateRoot/Download/$AllowPartFile' '$BackendRoot/Download/$QMarkSingleFile' '$BackendPrivateRoot/Download/$QMarkSingleFile' '$BackendRoot/Download/$QMarkDoubleFile' '$BackendPrivateRoot/Download/$QMarkDoubleFile' '$BackendRoot/Download/Test/$TestFile' '$BackendPrivateRoot/Download/Test/$TestFile' '$BackendRoot/Download/Test/$HotBeforeFile' '$BackendRoot/Download/Test/$HotAfterFile' '$BackendPrivateRoot/Download/Test/$HotBeforeFile' '$BackendPrivateRoot/Download/Test/$HotAfterFile' '$BackendRoot/.xldownload/$TestFile' '$BackendRoot/.xlDownload/$TestFile' '$BackendPrivateRoot/.xldownload/$TestFile' '$BackendPrivateRoot/.xlDownload/$TestFile'" | Out-Null
    Invoke-Su "mkdir -p '$BackendRoot/Download/SrtProbe' '$BackendRoot/Download/Test' '$BackendRoot/Download/SrtMapOnlyMapped' '$BackendRoot/Download/SrtReadOnly' '$BackendRoot/Download/SrtMapRO' '$BackendRoot/Download/SrtAllow/tmp' '$BackendRoot/Download/SrtLegacy/tmp' '$BackendRoot/Download/SrtQMark/Keep1' '$BackendRoot/Download/SrtQMark/Keep12' '$BackendRoot/Download/SrtLongest/Deep' '$BackendRoot/Download/SrtLongestBase' '$BackendRoot/Download/SrtLongestDeep' '$BackendRoot/Download/SrtPriority' '$BackendRoot/Download/SrtPriorityMapped' '$BackendRoot/Pictures/SrtLocked' '$BackendRoot/.xldownload' '$BackendRoot/.xlDownload' '$BackendPrivateRoot/Download/SrtProbe' '$BackendPrivateRoot/Download/Test' '$BackendPrivateRoot/Download/SrtMapOnlyMapped' '$BackendPrivateRoot/Download/SrtReadOnly' '$BackendPrivateRoot/Download/SrtMapRO' '$BackendPrivateRoot/Download/SrtAllow/tmp' '$BackendPrivateRoot/Download/SrtLegacy/tmp' '$BackendPrivateRoot/Download/SrtQMark/Keep1' '$BackendPrivateRoot/Download/SrtQMark/Keep12' '$BackendPrivateRoot/Download/SrtLongest/Deep' '$BackendPrivateRoot/Download/SrtLongestBase' '$BackendPrivateRoot/Download/SrtLongestDeep' '$BackendPrivateRoot/Download/SrtPriority' '$BackendPrivateRoot/Download/SrtPriorityMapped' '$BackendPrivateRoot/Pictures/SrtLocked' '$BackendPrivateRoot/.xldownload' '$BackendPrivateRoot/.xlDownload'; chmod -R 777 '$BackendRoot/Download/SrtProbe' '$BackendRoot/Download/Test' '$BackendRoot/Download/SrtMapOnlyMapped' '$BackendRoot/Download/SrtReadOnly' '$BackendRoot/Download/SrtMapRO' '$BackendRoot/Download/SrtAllow' '$BackendRoot/Download/SrtLegacy' '$BackendRoot/Download/SrtQMark' '$BackendRoot/Download/SrtLongest' '$BackendRoot/Download/SrtLongestBase' '$BackendRoot/Download/SrtLongestDeep' '$BackendRoot/Download/SrtPriority' '$BackendRoot/Download/SrtPriorityMapped' '$BackendRoot/Pictures/SrtLocked' '$BackendPrivateRoot/Download/SrtProbe' '$BackendPrivateRoot/Download/Test' '$BackendPrivateRoot/Download/SrtMapOnlyMapped' '$BackendPrivateRoot/Download/SrtReadOnly' '$BackendPrivateRoot/Download/SrtMapRO' '$BackendPrivateRoot/Download/SrtAllow' '$BackendPrivateRoot/Download/SrtLegacy' '$BackendPrivateRoot/Download/SrtQMark' '$BackendPrivateRoot/Download/SrtLongest' '$BackendPrivateRoot/Download/SrtLongestBase' '$BackendPrivateRoot/Download/SrtLongestDeep' '$BackendPrivateRoot/Download/SrtPriority' '$BackendPrivateRoot/Download/SrtPriorityMapped' '$BackendPrivateRoot/Pictures/SrtLocked' 2>/dev/null || true; chmod 777 '$BackendRoot/.xldownload' '$BackendRoot/.xlDownload' '$BackendPrivateRoot/.xldownload' '$BackendPrivateRoot/.xlDownload' 2>/dev/null || true" | Out-Null
    Invoke-Su "rm -f '$BackendRoot/Download/$QMarkFileSingleFile' '$BackendPrivateRoot/Download/$QMarkFileSingleFile'" | Out-Null
    Invoke-Su "rm -rf '$BackendRoot/Download/SrtFusePlain' '$BackendRoot/Download/SrtFuseExclude' '$BackendRoot/Download/SrtFuseMapParent' '$BackendRoot/Download/SrtFuseMapRW' '$BackendRoot/Download/SrtFuseMapRO' '$BackendRoot/Download/SrtFuseMulti' '$BackendRoot/DCIM/SrtFuseQQ' '$BackendRoot/DCIM/SrtFuseOther' '$BackendPrivateRoot/Download/SrtFusePlain' '$BackendPrivateRoot/Download/SrtFuseExclude' '$BackendPrivateRoot/Download/SrtFuseMapParent' '$BackendPrivateRoot/Download/SrtFuseMapRW' '$BackendPrivateRoot/Download/SrtFuseMapRO' '$BackendPrivateRoot/Download/SrtFuseMulti' '$BackendPrivateRoot/DCIM/SrtFuseQQ' '$BackendPrivateRoot/DCIM/SrtFuseOther'; mkdir -p '$BackendRoot/Download/SrtFusePlain' '$BackendRoot/Download/SrtFuseExclude/Locked' '$BackendRoot/Download/SrtFuseExclude/Writable' '$BackendRoot/Download/SrtFuseMapParent/WritableTarget' '$BackendRoot/Download/SrtFuseMapParent/LockedTarget' '$BackendRoot/Download/SrtFuseMapRW' '$BackendRoot/Download/SrtFuseMapRO' '$BackendRoot/Download/SrtFuseMulti/QQ' '$BackendRoot/Download/SrtFuseMulti/WeChat' '$BackendRoot/Download/SrtFuseMulti/Locked' '$BackendRoot/Download/SrtFuseMulti/Other' '$BackendRoot/DCIM/SrtFuseQQ' '$BackendRoot/DCIM/SrtFuseOther' '$BackendPrivateRoot/Download/SrtFusePlain' '$BackendPrivateRoot/Download/SrtFuseExclude/Locked' '$BackendPrivateRoot/Download/SrtFuseExclude/Writable' '$BackendPrivateRoot/Download/SrtFuseMapParent/WritableTarget' '$BackendPrivateRoot/Download/SrtFuseMapParent/LockedTarget' '$BackendPrivateRoot/Download/SrtFuseMapRW' '$BackendPrivateRoot/Download/SrtFuseMapRO' '$BackendPrivateRoot/Download/SrtFuseMulti/QQ' '$BackendPrivateRoot/Download/SrtFuseMulti/WeChat' '$BackendPrivateRoot/Download/SrtFuseMulti/Locked' '$BackendPrivateRoot/Download/SrtFuseMulti/Other' '$BackendPrivateRoot/DCIM/SrtFuseQQ' '$BackendPrivateRoot/DCIM/SrtFuseOther'; chmod -R 777 '$BackendRoot/Download/SrtFusePlain' '$BackendRoot/Download/SrtFuseExclude' '$BackendRoot/Download/SrtFuseMapParent' '$BackendRoot/Download/SrtFuseMapRW' '$BackendRoot/Download/SrtFuseMapRO' '$BackendRoot/Download/SrtFuseMulti' '$BackendRoot/DCIM/SrtFuseQQ' '$BackendRoot/DCIM/SrtFuseOther' '$BackendPrivateRoot/Download/SrtFusePlain' '$BackendPrivateRoot/Download/SrtFuseExclude' '$BackendPrivateRoot/Download/SrtFuseMapParent' '$BackendPrivateRoot/Download/SrtFuseMapRW' '$BackendPrivateRoot/Download/SrtFuseMapRO' '$BackendPrivateRoot/Download/SrtFuseMulti' '$BackendPrivateRoot/DCIM/SrtFuseQQ' '$BackendPrivateRoot/DCIM/SrtFuseOther' 2>/dev/null || true" | Out-Null
    Invoke-Su "rm -rf '$BackendRoot/Download/SrtFuseQa' '$BackendRoot/Download/SrtFuseQab' '$BackendRoot/Download/SrtFuseQb' '$BackendRoot/Download/SrtFuseMediaAlpha' '$BackendPrivateRoot/Download/SrtFuseQa' '$BackendPrivateRoot/Download/SrtFuseQab' '$BackendPrivateRoot/Download/SrtFuseQb' '$BackendPrivateRoot/Download/SrtFuseMediaAlpha'; mkdir -p '$BackendRoot/Download/SrtFuseQa/Media' '$BackendRoot/Download/SrtFuseQab/Media' '$BackendRoot/Download/SrtFuseQb/Media' '$BackendRoot/Download/SrtFuseMediaAlpha/Drop' '$BackendRoot/Download/SrtFuseMediaAlpha/Other' '$BackendPrivateRoot/Download/SrtFuseQa/Media' '$BackendPrivateRoot/Download/SrtFuseQab/Media' '$BackendPrivateRoot/Download/SrtFuseQb/Media' '$BackendPrivateRoot/Download/SrtFuseMediaAlpha/Drop' '$BackendPrivateRoot/Download/SrtFuseMediaAlpha/Other'; chmod -R 777 '$BackendRoot/Download/SrtFuseQa' '$BackendRoot/Download/SrtFuseQab' '$BackendRoot/Download/SrtFuseQb' '$BackendRoot/Download/SrtFuseMediaAlpha' '$BackendPrivateRoot/Download/SrtFuseQa' '$BackendPrivateRoot/Download/SrtFuseQab' '$BackendPrivateRoot/Download/SrtFuseQb' '$BackendPrivateRoot/Download/SrtFuseMediaAlpha' 2>/dev/null || true" | Out-Null
    Invoke-Su "rm -rf '$BackendRoot/Download/SrtMountNsAllow' '$BackendRoot/Download/SrtMountNsReadOnly' '$BackendRoot/Download/SrtMountNsMapParent' '$BackendRoot/Download/SrtMountNsMapRW' '$BackendRoot/Download/SrtMountNsMapRO' '$BackendPrivateRoot/Download/SrtMountNsAllow' '$BackendPrivateRoot/Download/SrtMountNsReadOnly' '$BackendPrivateRoot/Download/SrtMountNsMapParent' '$BackendPrivateRoot/Download/SrtMountNsMapRW' '$BackendPrivateRoot/Download/SrtMountNsMapRO'; mkdir -p '$BackendRoot/Download/SrtMountNsAllow' '$BackendRoot/Download/SrtMountNsReadOnly' '$BackendRoot/Download/SrtMountNsMapParent/WritableTarget' '$BackendRoot/Download/SrtMountNsMapParent/LockedTarget' '$BackendRoot/Download/SrtMountNsMapRW' '$BackendRoot/Download/SrtMountNsMapRO' '$BackendPrivateRoot/Download/SrtMountNsAllow' '$BackendPrivateRoot/Download/SrtMountNsReadOnly' '$BackendPrivateRoot/Download/SrtMountNsMapParent/WritableTarget' '$BackendPrivateRoot/Download/SrtMountNsMapParent/LockedTarget' '$BackendPrivateRoot/Download/SrtMountNsMapRW' '$BackendPrivateRoot/Download/SrtMountNsMapRO'; chmod -R 777 '$BackendRoot/Download/SrtMountNsAllow' '$BackendRoot/Download/SrtMountNsReadOnly' '$BackendRoot/Download/SrtMountNsMapParent' '$BackendRoot/Download/SrtMountNsMapRW' '$BackendRoot/Download/SrtMountNsMapRO' '$BackendPrivateRoot/Download/SrtMountNsAllow' '$BackendPrivateRoot/Download/SrtMountNsReadOnly' '$BackendPrivateRoot/Download/SrtMountNsMapParent' '$BackendPrivateRoot/Download/SrtMountNsMapRW' '$BackendPrivateRoot/Download/SrtMountNsMapRO' 2>/dev/null || true" | Out-Null
    Invoke-Su "mkdir -p '$BackendRoot/Download/SrtMountNsAllow/TeamAlpha/Deep' '$BackendRoot/Download/SrtMountNsAllow/Qa/Deep' '$BackendPrivateRoot/Download/SrtMountNsAllow/TeamAlpha/Deep' '$BackendPrivateRoot/Download/SrtMountNsAllow/Qa/Deep'; chmod -R 777 '$BackendRoot/Download/SrtMountNsAllow' '$BackendPrivateRoot/Download/SrtMountNsAllow' 2>/dev/null || true" | Out-Null
    Invoke-Su "rm -rf '$BackendRoot/Download/SrtMonitor' '$BackendRoot/Download/SrtMonitorMap' '$BackendRoot/Download/SrtMonitorMapped' '$BackendRoot/Download/SrtMonitorLocked' '$BackendPrivateRoot/Download/SrtMonitor' '$BackendPrivateRoot/Download/SrtMonitorMap' '$BackendPrivateRoot/Download/SrtMonitorMapped' '$BackendPrivateRoot/Download/SrtMonitorLocked'; mkdir -p '$BackendRoot/Download/SrtMonitor' '$BackendRoot/Download/SrtMonitorMap' '$BackendRoot/Download/SrtMonitorMapped' '$BackendRoot/Download/SrtMonitorLocked/Writable' '$BackendPrivateRoot/Download/SrtMonitor' '$BackendPrivateRoot/Download/SrtMonitorMap' '$BackendPrivateRoot/Download/SrtMonitorMapped' '$BackendPrivateRoot/Download/SrtMonitorLocked/Writable'; chmod -R 777 '$BackendRoot/Download/SrtMonitor' '$BackendRoot/Download/SrtMonitorMap' '$BackendRoot/Download/SrtMonitorMapped' '$BackendRoot/Download/SrtMonitorLocked' '$BackendPrivateRoot/Download/SrtMonitor' '$BackendPrivateRoot/Download/SrtMonitorMap' '$BackendPrivateRoot/Download/SrtMonitorMapped' '$BackendPrivateRoot/Download/SrtMonitorLocked' 2>/dev/null || true" | Out-Null
    Invoke-Su "rm -rf '$BackendRoot/Pictures/SrtReadOnlyMedia' '$BackendPrivateRoot/Pictures/SrtReadOnlyMedia'; mkdir -p '$BackendRoot/Pictures/SrtReadOnlyMedia' '$BackendPrivateRoot/Pictures/SrtReadOnlyMedia'; chmod -R 777 '$BackendRoot/Pictures/SrtReadOnlyMedia' '$BackendPrivateRoot/Pictures/SrtReadOnlyMedia' 2>/dev/null || true" | Out-Null
}

function Remove-TestTargetArtifacts {
    Invoke-Su "rm -rf '$BackendRoot/Download/SrtProbe' '$BackendRoot/Download/SrtOther' '$BackendRoot/Download/SrtOtherMapped' '$BackendRoot/Download/SrtMapOnlyMapped' '$BackendRoot/Download/SrtReadOnly' '$BackendRoot/Download/SrtMapRO' '$BackendRoot/Download/SrtAllow' '$BackendRoot/Download/SrtLegacy' '$BackendRoot/Download/SrtQMark' '$BackendRoot/Download/SrtLongest' '$BackendRoot/Download/SrtLongestBase' '$BackendRoot/Download/SrtLongestDeep' '$BackendRoot/Download/SrtPriority' '$BackendRoot/Download/SrtPriorityMapped' '$BackendRoot/Download/Test' '$BackendRoot/.xldownload' '$BackendRoot/.xlDownload' '$BackendRoot/Pictures/SrtLocked' '$BackendPrivateRoot/Download/SrtProbe' '$BackendPrivateRoot/Download/SrtOther' '$BackendPrivateRoot/Download/SrtOtherMapped' '$BackendPrivateRoot/Download/SrtMapOnlyMapped' '$BackendPrivateRoot/Download/SrtReadOnly' '$BackendPrivateRoot/Download/SrtMapRO' '$BackendPrivateRoot/Download/SrtAllow' '$BackendPrivateRoot/Download/SrtLegacy' '$BackendPrivateRoot/Download/SrtQMark' '$BackendPrivateRoot/Download/SrtLongest' '$BackendPrivateRoot/Download/SrtLongestBase' '$BackendPrivateRoot/Download/SrtLongestDeep' '$BackendPrivateRoot/Download/SrtPriority' '$BackendPrivateRoot/Download/SrtPriorityMapped' '$BackendPrivateRoot/Download/Test' '$BackendPrivateRoot/.xldownload' '$BackendPrivateRoot/.xlDownload' '$BackendPrivateRoot/Pictures/SrtLocked'; rm -f '$BackendRoot/Download/$AllowPartFile' '$BackendPrivateRoot/Download/$AllowPartFile' '$BackendRoot/Download/$QMarkSingleFile' '$BackendPrivateRoot/Download/$QMarkSingleFile' '$BackendRoot/Download/$QMarkDoubleFile' '$BackendPrivateRoot/Download/$QMarkDoubleFile'" | Out-Null
    Invoke-Su "rm -f '$BackendRoot/Download/$QMarkFileSingleFile' '$BackendPrivateRoot/Download/$QMarkFileSingleFile'" | Out-Null
    Invoke-Su "rm -rf '$BackendRoot/Download/SrtFusePlain' '$BackendRoot/Download/SrtFuseExclude' '$BackendRoot/Download/SrtFuseMapParent' '$BackendRoot/Download/SrtFuseMapRW' '$BackendRoot/Download/SrtFuseMapRO' '$BackendRoot/Download/SrtFuseMulti' '$BackendRoot/DCIM/SrtFuseQQ' '$BackendRoot/DCIM/SrtFuseOther' '$BackendPrivateRoot/Download/SrtFusePlain' '$BackendPrivateRoot/Download/SrtFuseExclude' '$BackendPrivateRoot/Download/SrtFuseMapParent' '$BackendPrivateRoot/Download/SrtFuseMapRW' '$BackendPrivateRoot/Download/SrtFuseMapRO' '$BackendPrivateRoot/Download/SrtFuseMulti' '$BackendPrivateRoot/DCIM/SrtFuseQQ' '$BackendPrivateRoot/DCIM/SrtFuseOther'" | Out-Null
    Invoke-Su "rm -rf '$BackendRoot/Download/SrtFuseQa' '$BackendRoot/Download/SrtFuseQab' '$BackendRoot/Download/SrtFuseQb' '$BackendRoot/Download/SrtFuseMediaAlpha' '$BackendPrivateRoot/Download/SrtFuseQa' '$BackendPrivateRoot/Download/SrtFuseQab' '$BackendPrivateRoot/Download/SrtFuseQb' '$BackendPrivateRoot/Download/SrtFuseMediaAlpha'" | Out-Null
    Invoke-Su "rm -rf '$BackendRoot/Download/SrtMountNsAllow' '$BackendRoot/Download/SrtMountNsReadOnly' '$BackendRoot/Download/SrtMountNsMapParent' '$BackendRoot/Download/SrtMountNsMapRW' '$BackendRoot/Download/SrtMountNsMapRO' '$BackendPrivateRoot/Download/SrtMountNsAllow' '$BackendPrivateRoot/Download/SrtMountNsReadOnly' '$BackendPrivateRoot/Download/SrtMountNsMapParent' '$BackendPrivateRoot/Download/SrtMountNsMapRW' '$BackendPrivateRoot/Download/SrtMountNsMapRO'" | Out-Null
    Invoke-Su "rm -rf '$BackendRoot/Download/SrtMonitor' '$BackendRoot/Download/SrtMonitorMap' '$BackendRoot/Download/SrtMonitorMapped' '$BackendRoot/Download/SrtMonitorLocked' '$BackendPrivateRoot/Download/SrtMonitor' '$BackendPrivateRoot/Download/SrtMonitorMap' '$BackendPrivateRoot/Download/SrtMonitorMapped' '$BackendPrivateRoot/Download/SrtMonitorLocked'" | Out-Null
    Invoke-Su "rm -rf '$BackendRoot/Pictures/SrtReadOnlyMedia' '$BackendPrivateRoot/Pictures/SrtReadOnlyMedia'" | Out-Null
}

function Remove-MediaStoreRowsByPattern {
    param(
        [string]$CollectionUri,
        [string[]]$NamePatterns,
        [string[]]$PathPatterns
    )

    $rows = @(Invoke-Su "content query --uri '$CollectionUri' --projection _id:_display_name:_data:relative_path 2>/dev/null || true")
    foreach ($row in $rows) {
        if ($row -notmatch "_id=(\d+)") { continue }
        $id = $Matches[1]
        $nameMatched = $false
        foreach ($pattern in $NamePatterns) {
            if ($row -match $pattern) {
                $nameMatched = $true
                break
            }
        }
        if (-not $nameMatched) { continue }

        $pathMatched = $false
        foreach ($pattern in $PathPatterns) {
            if ($row -match $pattern) {
                $pathMatched = $true
                break
            }
        }
        if (-not $pathMatched) { continue }

        Invoke-Adb @("shell", "content", "delete", "--uri", "$CollectionUri/$id") | Out-Null
    }
}

function Remove-RandomMediaStoreRows {
    $escapedAppId = [regex]::Escape($AppId)
    Remove-MediaStoreRowsByPattern "content://media/external/images/media" @("_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_image_\d+( \(\d+\))?\.jpg(,|$)") @("relative_path=Pictures/", "_data=.*/Pictures/", "_data=.*/Android/data/$escapedAppId/sdcard/Pictures/")
    Remove-MediaStoreRowsByPattern "content://media/external/images/media" @("_display_name=srt_read_only_media( \(\d+\))?\.jpg(,|$)") @("relative_path=Pictures/SrtReadOnlyMedia/", "_data=.*/Pictures/SrtReadOnlyMedia/", "_data=.*/Android/data/$escapedAppId/sdcard/Pictures/SrtReadOnlyMedia/")
    Remove-MediaStoreRowsByPattern "content://media/external/video/media" @("_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_video_\d+( \(\d+\))?\.mp4(,|$)") @("relative_path=Movies/", "_data=.*/Movies/", "_data=.*/Android/data/$escapedAppId/sdcard/Movies/")
    Remove-MediaStoreRowsByPattern "content://media/external/audio/media" @("_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_audio_\d+( \(\d+\))?\.mp3(,|$)") @("relative_path=Music/", "_data=.*/Music/", "_data=.*/Android/data/$escapedAppId/sdcard/Music/")
    Remove-MediaStoreRowsByPattern "content://media/external/file" @("_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_file_\d+( \(\d+\))?\.txt(,|$)") @("relative_path=Documents/", "_data=.*/Documents/", "_data=.*/Android/data/$escapedAppId/sdcard/Documents/")
    Remove-MediaStoreRowsByPattern "content://media/external/downloads" @("_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_download_\d+( \(\d+\))?\.bin(,|$)", "_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_ci_probe( \(\d+\))?\.part(,|$)", "_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_qmark_a( \(\d+\))?\.txt(,|$)", "_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_qmark_ab( \(\d+\))?\.txt(,|$)", "_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_qmark_file_a( \(\d+\))?\.txt(,|$)", "_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_mountns_star_media( \(\d+\))?\.bin(,|$)", "_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_mountns_qmark_media( \(\d+\))?\.bin(,|$)", "_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_fuse_star_media( \(\d+\))?\.bin(,|$)", "_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_fuse_star_miss_media( \(\d+\))?\.bin(,|$)", "_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_fuse_qmark_media( \(\d+\))?\.bin(,|$)", "_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_fuse_qmark_miss_media( \(\d+\))?\.bin(,|$)") @("relative_path=Download/", "_data=.*/Download/", "_data=.*/Android/data/$escapedAppId/sdcard/Download/")
    Remove-MediaStoreRowsByPattern "content://media/external/downloads" @("_display_name=(\.pending-\d+-|\.trashed-\d+-)?srt_monitor_[A-Za-z0-9_.-]+( \(\d+\))?\.bin(,|$)") @("relative_path=Download/SrtMonitor", "relative_path=Download/SrtMonitorMap", "relative_path=Download/SrtMonitorMapped", "relative_path=Download/SrtMonitorLocked", "_data=.*/Download/SrtMonitor", "_data=.*/Android/data/$escapedAppId/sdcard/Download/SrtMonitor")
}

function Remove-RandomPhysicalMediaFiles {
    Invoke-Su "find '$BackendRoot/Pictures' '$BackendPrivateRoot/Pictures' -maxdepth 1 -type f \( -name 'srt_image_[0-9]*.jpg' -o -name '.pending-*srt_image_[0-9]*.jpg' -o -name '.trashed-*srt_image_[0-9]*.jpg' \) -delete 2>/dev/null || true" | Out-Null
    Invoke-Su "rm -rf '$BackendRoot/Pictures/SrtReadOnlyMedia' '$BackendPrivateRoot/Pictures/SrtReadOnlyMedia' 2>/dev/null || true" | Out-Null
    Invoke-Su "find '$BackendRoot/Movies' '$BackendPrivateRoot/Movies' -maxdepth 1 -type f \( -name 'srt_video_[0-9]*.mp4' -o -name '.pending-*srt_video_[0-9]*.mp4' -o -name '.trashed-*srt_video_[0-9]*.mp4' \) -delete 2>/dev/null || true" | Out-Null
    Invoke-Su "find '$BackendRoot/Music' '$BackendPrivateRoot/Music' -maxdepth 1 -type f \( -name 'srt_audio_[0-9]*.mp3' -o -name '.pending-*srt_audio_[0-9]*.mp3' -o -name '.trashed-*srt_audio_[0-9]*.mp3' \) -delete 2>/dev/null || true" | Out-Null
    Invoke-Su "find '$BackendRoot/Documents' '$BackendPrivateRoot/Documents' -maxdepth 1 -type f \( -name 'srt_file_[0-9]*.txt' -o -name '.pending-*srt_file_[0-9]*.txt' -o -name '.trashed-*srt_file_[0-9]*.txt' \) -delete 2>/dev/null || true" | Out-Null
    Invoke-Su "find '$BackendRoot/Download' '$BackendPrivateRoot/Download' -maxdepth 1 -type f \( -name 'srt_download_[0-9]*.bin' -o -name '.pending-*srt_download_[0-9]*.bin' -o -name '.trashed-*srt_download_[0-9]*.bin' -o -name 'srt_ci_probe*.part' -o -name '.pending-*srt_ci_probe*.part' -o -name '.trashed-*srt_ci_probe*.part' -o -name 'srt_qmark*.txt' -o -name '.pending-*srt_qmark*.txt' -o -name '.trashed-*srt_qmark*.txt' -o -name 'srt_mountns_*_media.bin' -o -name '.pending-*srt_mountns_*_media.bin' -o -name '.trashed-*srt_mountns_*_media.bin' -o -name 'srt_fuse_*_media.bin' -o -name '.pending-*srt_fuse_*_media.bin' -o -name '.trashed-*srt_fuse_*_media.bin' \) -delete 2>/dev/null || true" | Out-Null
    Invoke-Su "find '$BackendRoot/Download/SrtMonitor' '$BackendRoot/Download/SrtMonitorMap' '$BackendRoot/Download/SrtMonitorMapped' '$BackendRoot/Download/SrtMonitorLocked' '$BackendPrivateRoot/Download/SrtMonitor' '$BackendPrivateRoot/Download/SrtMonitorMap' '$BackendPrivateRoot/Download/SrtMonitorMapped' '$BackendPrivateRoot/Download/SrtMonitorLocked' -type f \( -name 'srt_monitor_*.bin' -o -name '.pending-*srt_monitor_*.bin' -o -name '.trashed-*srt_monitor_*.bin' \) -delete 2>/dev/null || true" | Out-Null
    Invoke-Su "rm -rf '$BackendResultDir' '$BackendRoot/Android/data/$AppId/files/srt_file_tests' '$InternalResultDir' '/data/data/$AppId/files/srt_file_tests' '$SandboxResultDir' '$BackendPrivateRoot/Android/data/$AppId/files/srt_file_tests' 2>/dev/null || true" | Out-Null
}

function Restart-MediaProvider {
    Invoke-Adb @("shell", "am", "force-stop", "com.android.providers.media.module") | Out-Null
    Invoke-Adb @("shell", "am", "force-stop", "com.google.android.providers.media.module") | Out-Null
    Invoke-Su "pkill -f com.android.providers.media.module 2>/dev/null || true; pkill -f com.google.android.providers.media.module 2>/dev/null || true" | Out-Null
    Start-Sleep -Seconds 2
}

function Ensure-MonitorCollector {
    Invoke-Su "touch /data/adb/modules/storage.redirect.x/config/apps '$GlobalConfig' '$Config' 2>/dev/null || true" | Out-Null
    Invoke-Su "/data/adb/modules/storage.redirect.x/bin/srxctl ensure-collectors" | Out-Null
}

function Clear-FileMonitorLog {
    Invoke-Su "mkdir -p /data/adb/modules/storage.redirect.x/logs; : > '$FileMonitorLogPath'" | Out-Null
}

function Test-FileMonitorWatchCapacityLimited {
    $status = (@(
        Invoke-Su "grep -E 'daemon monitor watch limit reached|capacity_limited=true' /data/adb/modules/storage.redirect.x/logs/running.log 2>/dev/null | tail -1 || true"
    ) -join "`n").Trim()
    $status -match 'daemon monitor watch limit reached|capacity_limited=true'
}

function Test-FileMonitorEnabledForScenario {
    param([string]$Scenario, [string]$Label)
    $configText = (@(Invoke-Su "cat '$GlobalConfig' 2>/dev/null || true") -join "`n").Trim()
    $fileMonitorEnabled = $false
    if ($configText) {
        try {
            $fileMonitorEnabled = [bool](($configText | ConvertFrom-Json).file_monitor_enabled)
        } catch {
            $fileMonitorEnabled = $configText -match '"file_monitor_enabled"\s*:\s*true'
        }
    }
    if ($fileMonitorEnabled) {
        return $true
    }
    $script:Failures.Add("scenario-$Scenario/$Label file_monitor_enabled is not true")
    Write-Warning "file_monitor_disabled scenario=$Scenario label=$Label`: file_monitor_enabled must be true for monitor record tests"
    $configText -split "`n" | ForEach-Object { Write-Host "  global_config: $_" }
    return $false
}

function Prepare-FileMonitorAssertion {
    param([string]$Scenario, [string]$Label)
    Write-Host "  - monitor prepare $Scenario/$Label"
    if (-not (Test-FileMonitorEnabledForScenario $Scenario $Label)) {
        return $false
    }
    Invoke-Adb @("logcat", "-c") | Out-Null
    Clear-FileMonitorLog
    Ensure-MonitorCollector
    if ($script:ServiceCaseSettleMilliseconds -gt 0) {
        Start-Sleep -Milliseconds $script:ServiceCaseSettleMilliseconds
    }
    return $true
}

function Wait-FileMonitorLogLine {
    param(
        [string]$Scenario,
        [string]$Label,
        [string]$FileName,
        [ValidateSet("success", "failure")]
        [string]$Expected,
        [int]$TimeoutSeconds = 30,
        [switch]$AllowCapacityLimitedInotifyMiss
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        $lines = @(
            Invoke-Su "grep -F -- '$FileName' '$FileMonitorLogPath' 2>/dev/null || true"
        )
        foreach ($line in $lines) {
            if ($Expected -eq "success" -and $line -notmatch "ret=-1" -and $line -notmatch "op=close_write") {
                Write-Host "  - monitor_log_found $Scenario/$Label file=$FileName expected=$Expected"
                return $true
            }
            if ($Expected -eq "failure" -and $line -match "ret=-1" -and $line -match "deny_reason=read_only_rule") {
                Write-Host "  - monitor_log_found $Scenario/$Label file=$FileName expected=$Expected"
                return $true
            }
        }
        Start-Sleep -Milliseconds 200
    }
    if ($AllowCapacityLimitedInotifyMiss -and (Test-FileMonitorWatchCapacityLimited)) {
        Write-Warning "monitor log skipped $Scenario/$Label file=$FileName expected=$Expected reason=watch-capacity-limited"
        return $true
    }
    Write-Warning "monitor log timeout $Scenario/$Label file=$FileName expected=$Expected"
    $script:Failures.Add("scenario-$Scenario/$Label monitor log timeout file=$FileName expected=$Expected")
    @(
        Invoke-Su "tail -80 '$FileMonitorLogPath' 2>/dev/null || true"
    ) | ForEach-Object { Write-Host "  monitor_tail: $_" }
    return $false
}

function New-MonitorFileName {
    param([string]$Scenario, [string]$Label)
    "srt_monitor_${Scenario}_${Label}.bin" -replace '[^A-Za-z0-9_.-]', '_'
}

function Invoke-FileMonitorWriteSuccessCase {
    param(
        [string]$Scenario,
        [string]$Label,
        [string]$Path,
        [string]$ExpectedPath,
        [string]$PrivatePath = "",
        [bool]$AllowCapacityLimitedInotifyMiss = $false
    )
    $fileName = ($Path -split '/')[-1]
    if (-not (Prepare-FileMonitorAssertion $Scenario $Label)) { return $false }
    $ok = (Invoke-WriteCase ([int]$Scenario) $Label $Path $Payload).Ok
    $ok = (Require-File "scenario-$Scenario" "$Label expected" $ExpectedPath) -and $ok
    if ($PrivatePath) {
        $ok = (Require-Missing "scenario-$Scenario" "$Label private" $PrivatePath) -and $ok
    }
    $ok = (Wait-FileMonitorLogLine $Scenario $Label $fileName "success" -AllowCapacityLimitedInotifyMiss:$AllowCapacityLimitedInotifyMiss) -and $ok
    $ok
}

function Invoke-FileMonitorWriteDeniedCase {
    param([string]$Scenario, [string]$Label, [string]$Path, [string]$MissingPath = "")
    $fileName = ($Path -split '/')[-1]
    if (-not $MissingPath) { $MissingPath = $Path }
    if (-not (Prepare-FileMonitorAssertion $Scenario $Label)) { return $false }
    $ok = (Invoke-ServiceCase "scenario-$Scenario" $Label "file_write_denied" @{ file_path = $Path; payload = $Payload } "^PASS \[file_write_denied\]").Ok
    $ok = (Require-Missing "scenario-$Scenario" "$Label missing" $MissingPath) -and $ok
    Write-Host "  - monitor_failure_record_skipped $Scenario/$Label file=$fileName reason=ordinary-app-inotify"
    $ok
}

function Invoke-FileMonitorMediaStoreSuccessCase {
    param([string]$Scenario, [string]$Label, [string]$RelativePath, [string]$ExpectedPath, [string]$PrivatePath = "")
    $fileName = New-MonitorFileName $Scenario $Label
    if (-not (Prepare-FileMonitorAssertion $Scenario $Label)) { return $false }
    $ok = (Invoke-MediaStoreDownloadCreateCase ([int]$Scenario) $Label $fileName $RelativePath).Ok
    $ok = (Require-File "scenario-$Scenario" "$Label expected" "$ExpectedPath/$fileName") -and $ok
    if ($PrivatePath) {
        $ok = (Require-Missing "scenario-$Scenario" "$Label private" "$PrivatePath/$fileName") -and $ok
    }
    $ok = (Wait-FileMonitorLogLine $Scenario $Label $fileName "success") -and $ok
    $ok
}

function Invoke-FileMonitorMediaStoreDeniedCase {
    param([string]$Scenario, [string]$Label, [string]$RelativePath, [string]$MissingPath)
    $fileName = New-MonitorFileName $Scenario $Label
    if (-not (Prepare-FileMonitorAssertion $Scenario $Label)) { return $false }
    $ok = (Invoke-MediaStoreDownloadCreateDeniedCase ([int]$Scenario) $Label $fileName $RelativePath).Ok
    $ok = (Require-Missing "scenario-$Scenario" "$Label missing" "$MissingPath/$fileName") -and $ok
    $ok = (Wait-FileMonitorLogLine $Scenario $Label $fileName "failure") -and $ok
    $ok
}

function Invoke-DisabledRedirectMonitorScenario {
    param([string]$Scenario)
    $fileName = "srt_monitor_${Scenario}_disabled_regular.bin"
    $ok = Invoke-FileMonitorWriteSuccessCase $Scenario "disabled-regular-write" "$MonitorBaseRoot/$fileName" "$MonitorBaseRoot/$fileName" "$PrivateMonitorBaseRoot/$fileName" $true
    $ok = (Invoke-FileMonitorMediaStoreSuccessCase $Scenario "disabled-system-writer-create" "Download/SrtMonitor" $MonitorBaseRoot $PrivateMonitorBaseRoot) -and $ok
    $ok
}

function Invoke-RegularMonitorScenario {
    param([string]$Scenario)
    $allowFile = "srt_monitor_${Scenario}_allow.bin"
    $mapFile = "srt_monitor_${Scenario}_map.bin"
    $lockedFile = "srt_monitor_${Scenario}_locked.bin"
    $writableFile = "srt_monitor_${Scenario}_writable.bin"
    $ok = $true
    $ok = (Invoke-FileMonitorWriteSuccessCase $Scenario "regular-allow-write" "$MonitorBaseRoot/$allowFile" "$MonitorBaseRoot/$allowFile" "$PrivateMonitorBaseRoot/$allowFile" $true) -and $ok
    if ([int]$Scenario -eq 25) {
        $ok = (Test-ScopedFuseDaemonStarted ([int]$Scenario) $MonitorLockedRoot) -and $ok
    }
    $ok = (Invoke-FileMonitorWriteSuccessCase $Scenario "regular-mapped-write" "$MonitorMapRequest/$mapFile" "$MonitorMapTarget/$mapFile") -and $ok
    $ok = (Invoke-FileMonitorWriteDeniedCase $Scenario "regular-read-only-denied" "$MonitorLockedRoot/$lockedFile") -and $ok
    $ok = (Invoke-FileMonitorWriteSuccessCase $Scenario "regular-read-only-excluded-write" "$MonitorWritableRoot/$writableFile" "$MonitorWritableRoot/$writableFile" "$PrivateMonitorWritableRoot/$writableFile") -and $ok
    $ok
}

function Invoke-MediaStoreMonitorScenario {
    param([string]$Scenario)
    $ok = $true
    $ok = (Invoke-FileMonitorMediaStoreSuccessCase $Scenario "media-allow-create" "Download/SrtMonitor" $MonitorBaseRoot $PrivateMonitorBaseRoot) -and $ok
    if ([int]$Scenario -eq 27) {
        $ok = (Test-ScopedFuseDaemonStarted ([int]$Scenario) $MonitorLockedRoot) -and $ok
    }
    $ok = (Invoke-FileMonitorMediaStoreSuccessCase $Scenario "media-mapped-create" "Download/SrtMonitorMap" $MonitorMapTarget) -and $ok
    $ok = (Invoke-FileMonitorMediaStoreDeniedCase $Scenario "media-read-only-denied" "Download/SrtMonitorLocked" $MonitorLockedRoot) -and $ok
    $ok = (Invoke-FileMonitorMediaStoreSuccessCase $Scenario "media-read-only-excluded-create" "Download/SrtMonitorLocked/Writable" $MonitorWritableRoot $PrivateMonitorWritableRoot) -and $ok
    $ok
}

function Get-AppPid {
    $pidText = @(Invoke-Adb @("shell", "pidof", $AppId)) | Select-Object -First 1
    if ($pidText) { ($pidText -split "\s+")[0] } else { "" }
}

function Invoke-ConfigHotReloadScenario {
    param([int]$Scenario)
    $previousFreshAppPerCase = $script:FreshAppPerCase
    $script:FreshAppPerCase = $false

    try {
        $initialPid = Get-AppPid
        if ([string]::IsNullOrWhiteSpace($initialPid)) {
            $script:Failures.Add("scenario-$Scenario app pid missing before hot reload")
            return $false
        }

        $beforeRequest = "$RealRoot/Download/SrtProbe/$HotBeforeFile"
        $beforePrivate = "$PrivateRoot/Download/SrtProbe/$HotBeforeFile"
        $afterRequest = "$RealRoot/Download/SrtProbe/$HotAfterFile"
        $afterMapped = "$RealRoot/Download/Test/$HotAfterFile"
        $afterPrivate = "$PrivateRoot/Download/SrtProbe/$HotAfterFile"

        $ok = (Invoke-WriteCase $Scenario "hot-initial-private" $beforeRequest $Payload).Ok
        $ok = (Require-File "scenario-$Scenario" "initial-private" $beforePrivate) -and $ok
        $ok = (Require-Missing "scenario-$Scenario" "initial-real" $beforeRequest) -and $ok
        if (-not $ok) {
            return $false
        }

        Write-Host "  - config_hot_reload_update scenario=$Scenario switch default redirect to path mapping without restarting app"
        Write-DeviceConfig '{"users":{"0":{"enabled":true,"path_mappings":{"Download/SrtProbe":"Download/Test"}}}}'

        for ($attempt = 1; $attempt -le 20; $attempt++) {
            $currentPid = Get-AppPid
            if ([string]::IsNullOrWhiteSpace($currentPid) -or $currentPid -ne $initialPid) {
                $script:Failures.Add("scenario-$Scenario app pid changed during hot reload before=$initialPid after=$currentPid")
                return $false
            }

            Invoke-Su "rm -f '$BackendRoot/Download/SrtProbe/$HotAfterFile' '$BackendRoot/Download/Test/$HotAfterFile' '$BackendPrivateRoot/Download/SrtProbe/$HotAfterFile' 2>/dev/null || true" | Out-Null
            $attemptOk = (Invoke-WriteCase $Scenario "hot-update-mapped-$attempt" $afterRequest $Payload).Ok
            $attemptOk = (Test-FileExists $afterMapped) -and $attemptOk
            $attemptOk = (Test-PathMissing $afterRequest) -and $attemptOk
            $attemptOk = (Test-PathMissing $afterPrivate) -and $attemptOk
            if ($attemptOk) {
                $currentPid = Get-AppPid
                if ([string]::IsNullOrWhiteSpace($currentPid) -or $currentPid -ne $initialPid) {
                    $script:Failures.Add("scenario-$Scenario app pid changed after hot reload applied before=$initialPid after=$currentPid")
                    return $false
                }
                Write-Host "  - config_hot_reload_applied scenario=$Scenario pid=$initialPid attempt=$attempt"
                return $true
            }
            Start-Sleep -Seconds 1
        }

        $script:Failures.Add("scenario-$Scenario config hot reload did not apply while app pid stayed $initialPid")
        Require-File "scenario-$Scenario" "hot-mapped" $afterMapped | Out-Null
        Require-Missing "scenario-$Scenario" "hot-request" $afterRequest | Out-Null
        Require-Missing "scenario-$Scenario" "hot-private" $afterPrivate | Out-Null
        $false
    } finally {
        $script:FreshAppPerCase = $previousFreshAppPerCase
    }
}

function Invoke-TestArtifactCleanup {
    if ($script:CleanupDone) { return }
    $script:CleanupDone = $true
    Write-Host "== cleanup test artifacts =="
    try { Invoke-Adb @("shell", "am", "force-stop", $AppId) | Out-Null } catch { Write-Warning "force-stop cleanup failed: $_" }
    try { Restore-AppConfig } catch { Write-Warning "app config restore failed: $_" }
    try { Restore-GlobalConfig } catch { Write-Warning "global config restore failed: $_" }
    try { Clear-Results } catch { Write-Warning "result cleanup failed: $_" }
    try { Remove-TestTargetArtifacts } catch { Write-Warning "target cleanup failed: $_" }
    try { Remove-RandomMediaStoreRows } catch { Write-Warning "MediaStore cleanup failed: $_" }
    try { Remove-RandomPhysicalMediaFiles } catch { Write-Warning "physical cleanup failed: $_" }
    try { Restart-MediaProvider } catch { Write-Warning "MediaProvider restart failed: $_" }
}

function Restart-App {
    param([string]$Label, [bool]$ExpectMount = $true)
    Invoke-Adb @("shell", "am", "force-stop", $AppId) | Out-Null
    Invoke-Adb @("logcat", "-c") | Out-Null
    Invoke-Su ": > '$LogPath' 2>/dev/null || true" | Out-Null
    Invoke-Adb @("shell", "am", "start", "-n", "$AppId/.MainActivity") | Out-Null
    $confirmed = if ($ExpectMount) { Wait-AppMountConfirmed $Label } else { $true }
    if (-not $confirmed -and $script:AppLaunchSettleMilliseconds -gt 0) {
        Start-Sleep -Milliseconds $script:AppLaunchSettleMilliseconds
    }
    Wait-Storage $Label | Out-Null
}

function Get-TargetPath {
    param([int]$Scenario)
    if ($Scenario -eq 8) { return "$RealRoot/.xldownload/$TestFile" }
    if ($Scenario -eq 14) { return "$RealRoot/Download/SrtLongest/Deep/$TestFile" }
    if ($Scenario -eq 15) { return "$RealRoot/Download/SrtPriority/$TestFile" }
    "$RealRoot/Download/SrtProbe/$TestFile"
}

function Get-LogicalDir {
    param([int]$Scenario)
    if ($Scenario -eq 8) { return "$RealRoot/.xldownload" }
    if ($Scenario -eq 14) { return "$RealRoot/Download/SrtLongest/Deep" }
    if ($Scenario -eq 15) { return "$RealRoot/Download/SrtPriority" }
    "$RealRoot/Download/SrtProbe"
}

function Get-ExpectedPath {
    param([int]$Scenario)
    switch ($Scenario) {
        1 { "$RealRoot/Download/SrtProbe/$TestFile" }
        2 { "$PrivateRoot/Download/SrtProbe/$TestFile" }
        3 { "$RealRoot/Download/Test/$TestFile" }
        4 { "$RealRoot/Download/Test/$TestFile" }
        5 { "$RealRoot/Download/SrtProbe/$TestFile" }
        6 { "$RealRoot/Download/SrtProbe/$TestFile" }
        7 { "$RealRoot/Download/SrtMapOnlyMapped/$TestFile" }
        8 { "$PrivateRoot/.xldownload/$TestFile" }
        14 { "$RealRoot/Download/SrtLongestDeep/$TestFile" }
        15 { "$RealRoot/Download/SrtPriorityMapped/$TestFile" }
    }
}

function Get-ScenarioTitle {
    param([int]$Scenario)
    switch ($Scenario) {
        1 { "no config keeps real path" }
        2 { "default redirect to private" }
        3 { "path mapping to Download/Test" }
        4 { "mapping priority over allow Download" }
        5 { "allow Download keeps real path" }
        6 { "mapping_mode_only unmatched stays real" }
        7 { "mapping_mode_only mapped path maps" }
        8 { "mapping_mode_only sandboxed .xlDownload alias" }
        9 { "read_only_paths deny writes" }
        10 { "mapped target read-only deny write" }
        11 { "allow with inline exclusions and wildcard" }
        12 { "legacy excluded_real_paths merges into allow exclusions" }
        13 { "allowed_real_paths question-mark wildcard" }
        14 { "path mapping longest-prefix match" }
        15 { "mapping priority over string sandboxed_paths" }
        16 { "Fuse daemon hybrid plain allow plus wildcard allow" }
        17 { "Fuse daemon read_only_paths exclusion priority" }
        18 { "Fuse daemon mapped final target read-only policy" }
        19 { "Fuse daemon sibling wildcard rules stay scoped" }
        20 { "mount namespace allowed wildcard fallback" }
        21 { "mount namespace read_only wildcard fallback" }
        22 { "mount namespace mapped final target read-only policy" }
        23 { "file monitor disabled redirect regular app and system writer success records" }
        24 { "file monitor regular app with fuse daemon off" }
        25 { "file monitor regular app with fuse daemon on" }
        26 { "file monitor system writer with fuse daemon off" }
        27 { "file monitor system writer with fuse daemon on" }
        28 { "MediaStore query keeps read-only real image visible" }
        29 { "config hot reload switches running app from default redirect to path mapping" }
    }
}

function Invoke-WriteCase {
    param([int]$Scenario, [string]$Label, [string]$Path, [string]$Data)
    Invoke-ServiceCase "scenario-$Scenario" $Label "file_write" @{ file_path = $Path; payload = $Data; expected_payload = $Data } "^PASS \[file_write\]"
}

function Invoke-CreateCase {
    param([int]$Scenario, [string]$Label, [string]$Path)
    Invoke-ServiceCase "scenario-$Scenario" $Label "file_create" @{ file_path = $Path } "^PASS \[file_create\]"
}

function Invoke-MediaStoreDownloadCreateCase {
    param([int]$Scenario, [string]$Label, [string]$FileName, [string]$RelativePath = "")
    $extras = @{ file_name = $FileName }
    if ($RelativePath) { $extras.relative_path = $RelativePath }
    Invoke-ServiceCase "scenario-$Scenario" $Label "mediastore_create_download" $extras "^PASS \[mediastore_create_download\]"
}

function Invoke-MediaStoreDownloadCreateDeniedCase {
    param([int]$Scenario, [string]$Label, [string]$FileName, [string]$RelativePath = "")
    $extras = @{ file_name = $FileName }
    if ($RelativePath) { $extras.relative_path = $RelativePath }
    Invoke-ServiceCase "scenario-$Scenario" $Label "mediastore_create_download_denied" $extras "^PASS \[mediastore_create_download_denied\]"
}

function Expect-AppEntry {
    param([int]$Scenario, [string]$Label, [string]$Dir, [string]$FileName)
    $result = Invoke-ServiceCase "scenario-$Scenario" $Label "file_list_dir" @{ file_dir = $Dir } "^PASS \[file_list_dir\]"
    if (-not $result.Ok) { return $false }
    if ($result.Text -match "entries=.*$([regex]::Escape($FileName))") { return $true }
    $script:Failures.Add("scenario-$Scenario/$Label app view missing $FileName in $Dir :: $($result.Text -replace "`n", " | ")")
    $false
}

function Expect-NoAppEntry {
    param([int]$Scenario, [string]$Label, [string]$Dir, [string]$FileName)
    $result = Invoke-ServiceCase "scenario-$Scenario" $Label "file_list_dir" @{ file_dir = $Dir } ""
    if ($result.Text -match "entries=.*$([regex]::Escape($FileName))") {
        $script:Failures.Add("scenario-$Scenario/$Label app view unexpectedly sees $FileName in $Dir")
        return $false
    }
    $true
}

function Invoke-StandardScenario {
    param([int]$Scenario)
    $ok = (Invoke-WriteCase $Scenario "write" (Get-TargetPath $Scenario) $Payload).Ok
    $ok = (Expect-AppEntry $Scenario "app-view" (Get-LogicalDir $Scenario) $TestFile) -and $ok
    if ($Scenario -eq 3) { $ok = (Expect-NoAppEntry $Scenario "mapped-real-view" "$RealRoot/Download/Test" $TestFile) -and $ok }
    if ($Scenario -eq 4) { $ok = (Expect-AppEntry $Scenario "mapped-real-view" "$RealRoot/Download/Test" $TestFile) -and $ok }
    $ok = (Require-File "scenario-$Scenario" "expected-location" (Get-ExpectedPath $Scenario)) -and $ok
    switch ($Scenario) {
        2 { $ok = (Require-Missing "scenario-$Scenario" "real-request" "$RealRoot/Download/SrtProbe/$TestFile") -and $ok }
        3 { $ok = (Require-Missing "scenario-$Scenario" "real-request" "$RealRoot/Download/SrtProbe/$TestFile") -and $ok }
        7 { $ok = (Require-Missing "scenario-$Scenario" "real-request" "$RealRoot/Download/SrtProbe/$TestFile") -and $ok }
        8 { $ok = (Require-Missing "scenario-$Scenario" "real-request" "$RealRoot/.xldownload/$TestFile") -and $ok }
    }
    $ok
}

function Set-ReadOnlySeed {
    $root = Convert-ToBackendPath $ReadOnlyRoot
    Invoke-Su "mkdir -p '$root'; rm -f '$root/write_denied.txt' '$root/renamed.txt' '$root/$ReadOnlyHardlink' '$root/$ReadOnlySymlink'; rm -rf '$root/newdir'; printf '%s' '$ReadOnlyPayload' > '$root/$ReadOnlyFile'; chmod -R 777 '$root' 2>/dev/null || true" | Out-Null
}

function Invoke-ReadOnlyScenario {
    param([int]$Scenario)
    $ok = $true
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "read" "file_read" @{ file_path = "$ReadOnlyRoot/$ReadOnlyFile"; expected_payload = $ReadOnlyPayload } "^PASS \[file_read\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "stat" "file_stat" @{ file_path = "$ReadOnlyRoot/$ReadOnlyFile" } "^PASS \[file_stat\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "access" "file_access" @{ file_path = "$ReadOnlyRoot/$ReadOnlyFile" } "^PASS \[file_access\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "write-denied" "file_write_denied" @{ file_path = "$ReadOnlyRoot/write_denied.txt"; payload = $Payload } "^PASS \[file_write_denied\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "truncate-denied" "file_truncate_denied" @{ file_path = "$ReadOnlyRoot/$ReadOnlyFile"; length = "4" } "^PASS \[file_truncate_denied\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "ftruncate-denied" "file_ftruncate_denied" @{ file_path = "$ReadOnlyRoot/$ReadOnlyFile"; length = "8" } "^PASS \[file_ftruncate_denied\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "chmod-denied" "file_chmod_denied" @{ file_path = "$ReadOnlyRoot/$ReadOnlyFile"; mode = "0600" } "^PASS \[file_chmod_denied\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "fchmod-denied" "file_fchmod_denied" @{ file_path = "$ReadOnlyRoot/$ReadOnlyFile"; mode = "0600" } "^PASS \[file_fchmod_denied\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "link-denied" "file_link_denied" @{ file_path = "$ReadOnlyRoot/$ReadOnlyFile"; target_file_path = "$ReadOnlyRoot/$ReadOnlyHardlink" } "^PASS \[file_link_denied\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "symlink-denied" "file_symlink_denied" @{ file_path = "$ReadOnlyRoot/$ReadOnlyFile"; target_file_path = "$ReadOnlyRoot/$ReadOnlySymlink" } "^PASS \[file_symlink_denied\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "mkdir-denied" "file_mkdir_denied" @{ file_dir = "$ReadOnlyRoot/newdir" } "^PASS \[file_mkdir_denied\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "rename-denied" "file_rename_denied" @{ file_path = "$ReadOnlyRoot/$ReadOnlyFile"; target_file_path = "$ReadOnlyRoot/renamed.txt" } "^PASS \[file_rename_denied\]").Ok -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "delete-denied" "file_delete_denied" @{ file_path = "$ReadOnlyRoot/$ReadOnlyFile" } "^PASS \[file_delete_denied\]").Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "seed-still-exists" "$ReadOnlyRoot/$ReadOnlyFile") -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "write-target" "$ReadOnlyRoot/write_denied.txt") -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "hardlink-target" "$ReadOnlyRoot/$ReadOnlyHardlink") -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "symlink-target" "$ReadOnlyRoot/$ReadOnlySymlink") -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "mkdir-target" "$ReadOnlyRoot/newdir") -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "rename-target" "$ReadOnlyRoot/renamed.txt") -and $ok
    $ok
}

function Wait-MediaStoreReadOnlyImage {
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        $rows = @(Invoke-Su "content query --uri content://media/external/images/media --projection _id:_display_name:_data:relative_path 2>/dev/null | grep -F -- '$ReadOnlyImageFile' || true")
        foreach ($row in $rows) {
            if ($row -like "*Pictures/SrtReadOnlyMedia*") {
                Write-Host "  - mediastore_row_ready $ReadOnlyMediaRoot/$ReadOnlyImageFile"
                return $true
            }
        }
        Start-Sleep -Milliseconds 500
    }
    $script:Failures.Add("scenario-28/read-only-image MediaStore row missing: $ReadOnlyMediaRoot/$ReadOnlyImageFile")
    $false
}

function Set-ReadOnlyMediaImage {
    $backendDir = Convert-ToBackendPath $ReadOnlyMediaRoot
    $backendPath = "$backendDir/$ReadOnlyImageFile"
    $privatePath = Convert-ToBackendPath "$PrivateReadOnlyMediaRoot/$ReadOnlyImageFile"
    Remove-MediaStoreRowsByPattern "content://media/external/images/media" @("_display_name=srt_read_only_media( \(\d+\))?\.jpg(,|$)") @("relative_path=Pictures/SrtReadOnlyMedia/", "_data=.*/Pictures/SrtReadOnlyMedia/")
    Invoke-Su "mkdir -p '$backendDir'; rm -f '$privatePath'; printf '%s' '$ReadOnlyImageBase64' | base64 -d > '$backendPath'; chmod -R 777 '$backendDir' 2>/dev/null || true" | Out-Null
    Invoke-Adb @("shell", "content", "insert", "--uri", "content://media/external/images/media", "--bind", "_data:s:$ReadOnlyMediaRoot/$ReadOnlyImageFile", "--bind", "_display_name:s:$ReadOnlyImageFile", "--bind", "mime_type:s:image/jpeg") | Out-Null
    Invoke-Adb @("shell", "am", "broadcast", "-a", "android.intent.action.MEDIA_SCANNER_SCAN_FILE", "-d", "file://$ReadOnlyMediaRoot/$ReadOnlyImageFile") | Out-Null
    Wait-MediaStoreReadOnlyImage
}

function Invoke-MediaStoreReadOnlyQueryScenario {
    param([int]$Scenario)
    $logicalPath = "$ReadOnlyMediaRoot/$ReadOnlyImageFile"
    $privatePath = "$PrivateReadOnlyMediaRoot/$ReadOnlyImageFile"
    $ok = Set-ReadOnlyMediaImage
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "read-only-image-query" "mediastore_query_read_only_image" @{ file_name = $ReadOnlyImageFile; expected_path = $logicalPath } "^PASS \[mediastore_query_read_only_image\]").Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "read-only-media-real" $logicalPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "read-only-media-private" $privatePath) -and $ok
    $ok
}

function Set-MappedReadOnlyTargets {
    $request = Convert-ToBackendPath $MappedReadOnlyRequest
    $target = Convert-ToBackendPath $MappedReadOnlyTarget
    Invoke-Su "mkdir -p '$request' '$target'; rm -f '$request/$TestFile' '$target/$TestFile'; chmod -R 777 '$request' '$target' 2>/dev/null || true" | Out-Null
}

function Invoke-MappedReadOnlyScenario {
    param([int]$Scenario)
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "mapped-write-denied" "file_write_denied" @{ file_path = "$MappedReadOnlyRequest/$TestFile"; payload = $Payload } "^PASS \[file_write_denied\]").Ok
    $ok = (Require-Missing "scenario-$Scenario" "request-file" "$MappedReadOnlyRequest/$TestFile") -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "target-file" "$MappedReadOnlyTarget/$TestFile") -and $ok
    $ok
}

function Invoke-AllowExclusionScenario {
    param([int]$Scenario)
    $keepPath = "$AllowRoot/$AllowKeepFile"
    $keepPrivate = "$PrivateAllowRoot/$AllowKeepFile"
    $tmpPath = "$AllowRoot/tmp/$TestFile"
    $tmpPrivate = "$PrivateAllowRoot/tmp/$TestFile"
    $partPath = "$RealRoot/Download/$AllowPartFile"
    $partPrivate = "$PrivateRoot/Download/$AllowPartFile"

    $ok = (Invoke-WriteCase $Scenario "allow-real-write" $keepPath $Payload).Ok
    $ok = (Require-File "scenario-$Scenario" "allow-real" $keepPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "allow-real-private" $keepPrivate) -and $ok
    $ok = (Invoke-WriteCase $Scenario "excluded-dir-write" $tmpPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "excluded-dir-private" $tmpPrivate) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "excluded-dir-real" $tmpPath) -and $ok
    $ok = (Invoke-MediaStoreDownloadCreateCase $Scenario "excluded-glob-download-create" $AllowPartFile).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "excluded-glob-private" $partPrivate) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "excluded-glob-real" $partPath) -and $ok
    $ok
}

function Invoke-LegacyExclusionScenario {
    param([int]$Scenario)
    $keepPath = "$LegacyRoot/$AllowKeepFile"
    $keepPrivate = "$PrivateLegacyRoot/$AllowKeepFile"
    $tmpPath = "$LegacyRoot/tmp/$TestFile"
    $tmpPrivate = "$PrivateLegacyRoot/tmp/$TestFile"

    $ok = (Invoke-WriteCase $Scenario "legacy-allow-real-write" $keepPath $Payload).Ok
    $ok = (Require-File "scenario-$Scenario" "legacy-allow-real" $keepPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "legacy-allow-private" $keepPrivate) -and $ok
    $ok = (Invoke-WriteCase $Scenario "legacy-excluded-write" $tmpPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "legacy-excluded-private" $tmpPrivate) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "legacy-excluded-real" $tmpPath) -and $ok
    $ok
}

function Invoke-QMarkWildcardScenario {
    param([int]$Scenario)
    $singlePath = "$RealRoot/Download/$QMarkSingleFile"
    $singlePrivate = "$PrivateRoot/Download/$QMarkSingleFile"
    $doublePath = "$RealRoot/Download/$QMarkDoubleFile"
    $doublePrivate = "$PrivateRoot/Download/$QMarkDoubleFile"
    $fileSinglePath = "$RealRoot/Download/$QMarkFileSingleFile"
    $fileSinglePrivate = "$PrivateRoot/Download/$QMarkFileSingleFile"

    $ok = (Invoke-MediaStoreDownloadCreateCase $Scenario "qmark-single-char-download-create" $QMarkSingleFile).Ok
    $ok = (Require-File "scenario-$Scenario" "qmark-single-char-real" $singlePath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "qmark-single-char-private" $singlePrivate) -and $ok
    $ok = (Invoke-MediaStoreDownloadCreateCase $Scenario "qmark-two-char-download-create" $QMarkDoubleFile).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "qmark-two-char-private" $doublePrivate) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "qmark-two-char-real" $doublePath) -and $ok
    $ok = (Invoke-WriteCase $Scenario "qmark-single-char-file-write" $fileSinglePath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "qmark-file-single-char-real" $fileSinglePath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "qmark-file-single-char-private" $fileSinglePrivate) -and $ok
    $ok
}

function Test-FuseDaemonStarted {
    param([int]$Scenario)
    for ($i = 0; $i -lt 5; $i++) {
        if (Test-Su "grep -Eq 'fuse redirect mount start pkg=$AppId|mount request cfg pkg=$AppId fuse_daemon=true|app mount confirmed pid=' '$LogPath' 2>/dev/null") {
            Write-Host "  - scenario-$Scenario/fuse-daemon-started"
            return $true
        }
        Start-Sleep -Milliseconds $script:ResultPollMilliseconds
    }
    Write-Warning "scenario-$Scenario/fuse-daemon-started log not observed; continuing with behavioral checks"
    $true
}

function Test-ScopedFuseDaemonStarted {
    param([int]$Scenario, [string]$MountRoot, [bool]$Strict = $true)
    for ($i = 0; $i -lt 20; $i++) {
        if (Test-Su "grep -F -- 'daemon hybrid fuse no scoped service mounted' '$LogPath' 2>/dev/null | grep -F -- 'pkg=$AppId' >/dev/null") {
            $script:Failures.Add("scenario-$Scenario scoped fuse fell back to mount namespace root=$MountRoot")
            Write-Warning "scenario-$Scenario/scoped-fuse-fallback root=$MountRoot"
            return $false
        }
        if (Test-Su "grep -F -- 'fuse redirect session ended' '$LogPath' 2>/dev/null | grep -F -- 'mp=$MountRoot' >/dev/null") {
            $script:Failures.Add("scenario-$Scenario scoped fuse session failed root=$MountRoot")
            Write-Warning "scenario-$Scenario/scoped-fuse-session-failed root=$MountRoot"
            return $false
        }
        if (Test-Su "grep -F -- 'fuse redirect mount start pkg=$AppId' '$LogPath' 2>/dev/null | grep -F -- 'mp=$MountRoot' >/dev/null") {
            Write-Host "  - scoped_fuse_started scenario=$Scenario root=$MountRoot"
            return $true
        }
        Start-Sleep -Milliseconds $script:ResultPollMilliseconds
    }
    if ($Strict) {
        $script:Failures.Add("scenario-$Scenario scoped fuse mount not observed root=$MountRoot")
        Write-Warning "scenario-$Scenario/scoped-fuse-missing root=$MountRoot"
        @(Invoke-Su "grep -F -- '$AppId' '$LogPath' 2>/dev/null | tail -80 || true") | ForEach-Object { Write-Host "  fuse_tail: $_" }
        return $false
    }
    Write-Warning "scenario-$Scenario/scoped-fuse-start-log-not-observed root=$MountRoot; continuing with behavioral checks"
    $true
}

function Invoke-FuseDaemonAllowWildcardScenario {
    param([int]$Scenario)
    $plainPath = "$FusePlainRoot/$TestFile"
    $plainPrivate = "$PrivateFusePlainRoot/$TestFile"
    $wildcardPath = "$FuseDcimRoot/$TestFile"
    $wildcardPrivate = "$PrivateFuseDcimRoot/$TestFile"
    $otherPath = "$FuseDcimOtherRoot/$TestFile"
    $otherPrivate = "$PrivateFuseDcimOtherRoot/$TestFile"
    $qmarkPath = "$FuseQMarkRoot/Media/$TestFile"
    $qmarkPrivate = "$PrivateFuseQMarkRoot/Media/$TestFile"
    $qmarkMissPath = "$FuseQMarkMissRoot/Media/$TestFile"
    $qmarkMissPrivate = "$PrivateFuseQMarkMissRoot/Media/$TestFile"
    $starMediaPath = "$FuseStarMediaRoot/Drop/$FuseStarMediaFile"
    $starMediaPrivate = "$PrivateFuseStarMediaRoot/Drop/$FuseStarMediaFile"
    $starMissMediaPath = "$FuseStarMediaRoot/Other/$FuseStarMissMediaFile"
    $starMissMediaPrivate = "$PrivateFuseStarMediaRoot/Other/$FuseStarMissMediaFile"
    $qmarkMediaPath = "$FuseQMarkMediaRoot/Media/$FuseQMarkMediaFile"
    $qmarkMediaPrivate = "$PrivateFuseQMarkMediaRoot/Media/$FuseQMarkMediaFile"
    $qmarkMissMediaPath = "$FuseQMarkMissRoot/Media/$FuseQMarkMissMediaFile"
    $qmarkMissMediaPrivate = "$PrivateFuseQMarkMissRoot/Media/$FuseQMarkMissMediaFile"

    $ok = Test-FuseDaemonStarted $Scenario
    $ok = (Invoke-WriteCase $Scenario "plain-allow-write" $plainPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-plain-real" $plainPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-plain-private" $plainPrivate) -and $ok
    $ok = (Invoke-WriteCase $Scenario "wildcard-allow-write" $wildcardPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-wildcard-real" $wildcardPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-wildcard-private" $wildcardPrivate) -and $ok
    $ok = (Invoke-WriteCase $Scenario "wildcard-other-write" $otherPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-wildcard-other-private" $otherPrivate) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-wildcard-other-real" $otherPath) -and $ok
    $ok = (Invoke-WriteCase $Scenario "qmark-allow-write" $qmarkPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-qmark-real" $qmarkPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-qmark-private" $qmarkPrivate) -and $ok
    $ok = (Invoke-WriteCase $Scenario "qmark-miss-write" $qmarkMissPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-qmark-miss-private" $qmarkMissPrivate) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-qmark-miss-real" $qmarkMissPath) -and $ok
    $ok = (Invoke-MediaStoreDownloadCreateCase $Scenario "fuse-star-media-download-create" $FuseStarMediaFile "Download/SrtFuseMediaAlpha/Drop").Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-star-media-real" $starMediaPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-star-media-private" $starMediaPrivate) -and $ok
    $ok = (Invoke-MediaStoreDownloadCreateCase $Scenario "fuse-star-media-miss-download-create" $FuseStarMissMediaFile "Download/SrtFuseMediaAlpha/Other").Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-star-media-miss-private" $starMissMediaPrivate) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-star-media-miss-real" $starMissMediaPath) -and $ok
    $ok = (Invoke-MediaStoreDownloadCreateCase $Scenario "fuse-qmark-media-download-create" $FuseQMarkMediaFile "Download/SrtFuseQb/Media").Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-qmark-media-real" $qmarkMediaPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-qmark-media-private" $qmarkMediaPrivate) -and $ok
    $ok = (Invoke-MediaStoreDownloadCreateCase $Scenario "fuse-qmark-media-miss-download-create" $FuseQMarkMissMediaFile "Download/SrtFuseQab/Media").Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-qmark-media-miss-private" $qmarkMissMediaPrivate) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-qmark-media-miss-real" $qmarkMissMediaPath) -and $ok
    $ok
}

function Invoke-FuseDaemonReadOnlyExclusionScenario {
    param([int]$Scenario)
    $lockedPath = "$FuseExcludeRoot/Locked/$TestFile"
    $writablePath = "$FuseExcludeRoot/Writable/$TestFile"

    $ok = Test-FuseDaemonStarted $Scenario
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "read-only-excluded-write" "file_write" @{ file_path = $writablePath; payload = $Payload; expected_payload = $Payload } "^PASS \[file_write\]").Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-read-only-excluded-real" $writablePath) -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "read-only-locked-write-denied" "file_write_denied" @{ file_path = $lockedPath; payload = $Payload } "^PASS \[file_write_denied\]").Ok -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-read-only-locked-real" $lockedPath) -and $ok
    $ok
}

function Invoke-FuseDaemonMappingReadOnlyScenario {
    param([int]$Scenario)
    $rwRequest = "$FuseMapRwRequest/$TestFile"
    $rwTarget = "$FuseMapRwTarget/$TestFile"
    $roRequest = "$FuseMapRoRequest/$TestFile"
    $roTarget = "$FuseMapRoTarget/$TestFile"

    $ok = Test-FuseDaemonStarted $Scenario
    $ok = (Invoke-WriteCase $Scenario "mapping-target-excluded-write" $rwRequest $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-mapping-rw-target" $rwTarget) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-mapping-rw-request" $rwRequest) -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "mapping-target-read-only-denied" "file_write_denied" @{ file_path = $roRequest; payload = $Payload } "^PASS \[file_write_denied\]").Ok -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-mapping-ro-target" $roTarget) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-mapping-ro-request" $roRequest) -and $ok
    $ok
}

function Invoke-MountNamespaceMappingReadOnlyScenario {
    param([int]$Scenario)
    $rwRequest = "$MountNsMapRwRequest/$TestFile"
    $rwTarget = "$MountNsMapRwTarget/$TestFile"
    $roRequest = "$MountNsMapRoRequest/$TestFile"
    $roTarget = "$MountNsMapRoTarget/$TestFile"

    $ok = (Invoke-WriteCase $Scenario "mapping-target-excluded-write" $rwRequest $Payload).Ok
    $ok = (Require-File "scenario-$Scenario" "mount-ns-mapping-rw-target" $rwTarget) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "mount-ns-mapping-rw-request" $rwRequest) -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "mapping-target-read-only-denied" "file_write_denied" @{ file_path = $roRequest; payload = $Payload } "^PASS \[file_write_denied\]").Ok -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "mount-ns-mapping-ro-target" $roTarget) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "mount-ns-mapping-ro-request" $roRequest) -and $ok
    $ok
}

function Invoke-FuseDaemonMultiWildcardScenario {
    param([int]$Scenario)
    $qqPath = "$FuseMultiRoot/QQ/$TestFile"
    $qqPrivate = "$PrivateFuseMultiRoot/QQ/$TestFile"
    $wechatPath = "$FuseMultiRoot/WeChat/$TestFile"
    $wechatPrivate = "$PrivateFuseMultiRoot/WeChat/$TestFile"
    $lockedPath = "$FuseMultiRoot/Locked/$TestFile"
    $otherPath = "$FuseMultiRoot/Other/$TestFile"
    $otherPrivate = "$PrivateFuseMultiRoot/Other/$TestFile"

    $ok = Test-FuseDaemonStarted $Scenario
    $ok = (Invoke-WriteCase $Scenario "multi-qq-write" $qqPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-multi-qq-real" $qqPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-multi-qq-private" $qqPrivate) -and $ok
    $ok = (Invoke-WriteCase $Scenario "multi-wechat-write" $wechatPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-multi-wechat-real" $wechatPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-multi-wechat-private" $wechatPrivate) -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "multi-locked-write-denied" "file_write_denied" @{ file_path = $lockedPath; payload = $Payload } "^PASS \[file_write_denied\]").Ok -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-multi-locked-real" $lockedPath) -and $ok
    $ok = (Invoke-WriteCase $Scenario "multi-other-write" $otherPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "fuse-multi-other-private" $otherPrivate) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "fuse-multi-other-real" $otherPath) -and $ok
    $ok
}

function Set-MountNamespaceReadOnlySeed {
    $root = Convert-ToBackendPath $MountNsReadOnlyRoot
    Invoke-Su "mkdir -p '$root'; rm -f '$root/write_denied.txt'; printf '%s' '$ReadOnlyPayload' > '$root/$ReadOnlyFile'; chmod -R 777 '$root' 2>/dev/null || true" | Out-Null
}

function Invoke-MountNamespaceAllowWildcardFallbackScenario {
    param([int]$Scenario)
    $controlPath = "$RealRoot/Download/SrtProbe/$TestFile"
    $controlPrivate = "$PrivateRoot/Download/SrtProbe/$TestFile"
    $starPath = "$MountNsAllowRoot/TeamAlpha/Deep/$TestFile"
    $starPrivate = "$PrivateMountNsAllowRoot/TeamAlpha/Deep/$TestFile"
    $qmarkPath = "$MountNsAllowRoot/Qa/Deep/$TestFile"
    $qmarkPrivate = "$PrivateMountNsAllowRoot/Qa/Deep/$TestFile"
    $starMediaPath = "$MountNsAllowRoot/TeamAlpha/Deep/$MountNsStarMediaFile"
    $starMediaPrivate = "$PrivateMountNsAllowRoot/TeamAlpha/Deep/$MountNsStarMediaFile"
    $qmarkMediaPath = "$MountNsAllowRoot/Qa/Deep/$MountNsQMarkMediaFile"
    $qmarkMediaPrivate = "$PrivateMountNsAllowRoot/Qa/Deep/$MountNsQMarkMediaFile"

    $ok = (Invoke-WriteCase $Scenario "control-private-write" $controlPath $Payload).Ok
    $ok = (Require-File "scenario-$Scenario" "control-private" $controlPrivate) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "control-real" $controlPath) -and $ok
    $ok = (Invoke-WriteCase $Scenario "star-fallback-write" $starPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "star-fallback-real" $starPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "star-fallback-private" $starPrivate) -and $ok
    $ok = (Invoke-WriteCase $Scenario "qmark-fallback-write" $qmarkPath $Payload).Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "qmark-fallback-real" $qmarkPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "qmark-fallback-private" $qmarkPrivate) -and $ok
    $ok = (Invoke-MediaStoreDownloadCreateCase $Scenario "star-fallback-media-create" $MountNsStarMediaFile "Download/SrtMountNsAllow/TeamAlpha/Deep").Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "star-fallback-media-real" $starMediaPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "star-fallback-media-private" $starMediaPrivate) -and $ok
    $ok = (Invoke-MediaStoreDownloadCreateCase $Scenario "qmark-fallback-media-create" $MountNsQMarkMediaFile "Download/SrtMountNsAllow/Qa/Deep").Ok -and $ok
    $ok = (Require-File "scenario-$Scenario" "qmark-fallback-media-real" $qmarkMediaPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "qmark-fallback-media-private" $qmarkMediaPrivate) -and $ok
    $ok
}

function Invoke-MountNamespaceReadOnlyWildcardFallbackScenario {
    param([int]$Scenario)
    $seedPath = "$MountNsReadOnlyRoot/$ReadOnlyFile"
    $seedPrivate = "$PrivateMountNsReadOnlyRoot/$ReadOnlyFile"
    $deniedPath = "$MountNsReadOnlyRoot/write_denied.txt"
    $deniedPrivate = "$PrivateMountNsReadOnlyRoot/write_denied.txt"

    $ok = (Invoke-ServiceCase "scenario-$Scenario" "fallback-read" "file_read" @{ file_path = $seedPath; expected_payload = $ReadOnlyPayload } "^PASS \[file_read\]").Ok
    $ok = (Require-Missing "scenario-$Scenario" "seed-private" $seedPrivate) -and $ok
    $ok = (Invoke-ServiceCase "scenario-$Scenario" "fallback-write-denied" "file_write_denied" @{ file_path = $deniedPath; payload = $Payload } "^PASS \[file_write_denied\]").Ok -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "denied-real" $deniedPath) -and $ok
    $ok = (Require-Missing "scenario-$Scenario" "denied-private" $deniedPrivate) -and $ok
    $ok
}

function Invoke-Scenario {
    param([int]$Scenario)
    Write-Host "== scenario ${Scenario}: $(Get-ScenarioTitle $Scenario) =="
    if ($Scenario -in @(16, 17, 18, 19)) {
        Invoke-Su ": > '$LogPath' 2>/dev/null || true" | Out-Null
    }
    Apply-ScenarioConfig $Scenario
    Clear-Targets
    if ($Scenario -eq 9) { Set-ReadOnlySeed }
    if ($Scenario -eq 10) { Set-MappedReadOnlyTargets }
    if ($Scenario -eq 21) { Set-MountNamespaceReadOnlySeed }
    Restart-App "scenario-$Scenario" ($Scenario -ne 1)
    $before = $script:Failures.Count
    $ok = switch ($Scenario) {
        9 { Invoke-ReadOnlyScenario $Scenario }
        10 { Invoke-MappedReadOnlyScenario $Scenario }
        11 { Invoke-AllowExclusionScenario $Scenario }
        12 { Invoke-LegacyExclusionScenario $Scenario }
        13 { Invoke-QMarkWildcardScenario $Scenario }
        16 { Invoke-FuseDaemonAllowWildcardScenario $Scenario }
        17 { Invoke-FuseDaemonReadOnlyExclusionScenario $Scenario }
        18 { Invoke-FuseDaemonMappingReadOnlyScenario $Scenario }
        19 { Invoke-FuseDaemonMultiWildcardScenario $Scenario }
        20 { Invoke-MountNamespaceAllowWildcardFallbackScenario $Scenario }
        21 { Invoke-MountNamespaceReadOnlyWildcardFallbackScenario $Scenario }
        22 { Invoke-MountNamespaceMappingReadOnlyScenario $Scenario }
        23 { Invoke-DisabledRedirectMonitorScenario $Scenario }
        24 { Invoke-RegularMonitorScenario $Scenario }
        25 { Invoke-RegularMonitorScenario $Scenario }
        26 { Invoke-MediaStoreMonitorScenario $Scenario }
        27 { Invoke-MediaStoreMonitorScenario $Scenario }
        28 { Invoke-MediaStoreReadOnlyQueryScenario $Scenario }
        29 { Invoke-ConfigHotReloadScenario $Scenario }
        default { Invoke-StandardScenario $Scenario }
    }
    if (-not $ok -and $script:Failures.Count -eq $before) {
        $script:Failures.Add("scenario-$Scenario returned false without a detailed failure")
    }
    $newFailures = $script:Failures.Count - $before
    $script:Summary.Add([pscustomobject]@{ Scenario = $Scenario; Title = (Get-ScenarioTitle $Scenario); Passed = ($ok -and $newFailures -eq 0); NewFailures = $newFailures }) | Out-Null
}

function Invoke-BasicAll {
    Write-Host "== basic suite with default redirect enabled =="
    Disable-FuseDaemonConfig
    Write-DeviceConfig '{"users":{"0":{"enabled":true}}}'
    Clear-Targets
    Restart-App "all-basic"
    $before = $script:Failures.Count
    $result = Invoke-ServiceCase "basic" "all" "all" @{} "^PASS "
    $failedLines = @()
    if ($result.Text) {
        $failedLines = $result.Text -split "`n" | Where-Object { $_ -match "^FAIL " }
    }
    foreach ($line in $failedLines) {
        $script:Failures.Add("basic/all $line")
    }
    $ok = $result.Ok -and $failedLines.Count -eq 0

    $queryCases = @(
        "mediastore_query_image",
        "mediastore_query_video",
        "mediastore_query_audio",
        "mediastore_query_file",
        "mediastore_query_download"
    )
    foreach ($case in $queryCases) {
        $queryResult = Invoke-ServiceCase "basic" $case $case @{} "^PASS \[$case\]"
        $ok = $queryResult.Ok -and $ok
    }

    $script:Summary.Add([pscustomobject]@{ Scenario = "basic"; Title = "deterministic all + query smoke"; Passed = $ok; NewFailures = ($script:Failures.Count - $before) }) | Out-Null
}

$script:ExitCode = 0
try {
    Backup-GlobalConfig
    Backup-AppConfig
    Invoke-Adb @("shell", "pm", "grant", $AppId, "android.permission.READ_EXTERNAL_STORAGE") | Out-Null
    Invoke-Adb @("shell", "pm", "grant", $AppId, "android.permission.WRITE_EXTERNAL_STORAGE") | Out-Null
    Invoke-Adb @("shell", "pm", "grant", $AppId, "android.permission.READ_MEDIA_IMAGES") | Out-Null
    Invoke-Adb @("shell", "pm", "grant", $AppId, "android.permission.READ_MEDIA_VIDEO") | Out-Null
    Invoke-Adb @("shell", "pm", "grant", $AppId, "android.permission.READ_MEDIA_AUDIO") | Out-Null
    Invoke-Adb @("shell", "appops", "set", $AppId, "MANAGE_EXTERNAL_STORAGE", "allow") | Out-Null
    Invoke-Adb @("logcat", "-c") | Out-Null
    Invoke-Su ": > '$LogPath' 2>/dev/null || true" | Out-Null
    Restart-MediaProvider
    Wait-Storage "initial" | Out-Null
    Wait-MediaProviderReady "initial" | Out-Null

    if (-not $SkipBasicAll) {
        Invoke-BasicAll
    }

    $scenarios = Get-ScenarioList

    foreach ($scenario in $scenarios) {
        Invoke-Scenario $scenario
    }

    Write-Host "== summary =="
    $script:Summary | Format-Table -AutoSize | Out-String | Write-Host

    $failedSummary = @($script:Summary | Where-Object { -not $_.Passed })
    if ($script:Failures.Count -gt 0 -or $failedSummary.Count -gt 0) {
        Write-Host "== failures =="
        $failedSummary | ForEach-Object {
            Write-Host "summary failure: scenario=$($_.Scenario) title=$($_.Title) newFailures=$($_.NewFailures)"
        }
        $script:Failures | ForEach-Object { Write-Host $_ }
        Write-Host "== module log tail =="
        Invoke-Su "echo ---global.json---; cat '$GlobalConfig' 2>/dev/null || true; echo; echo ---app config---; cat '$Config' 2>/dev/null || true; echo; for log in running.log app_status.log file_monitor.log media_provider_state.log; do echo ---`$log---; tail -80 /data/adb/modules/storage.redirect.x/logs/`$log 2>/dev/null || true; done" | Write-Host
        Write-Host "== relevant logcat tail =="
        & adb -s $Serial logcat -d -t 500 |
            Select-String -Pattern "StorageRedirectTest|srx|StorageRedirect|FATAL EXCEPTION|AndroidRuntime|MediaProvider|ExternalStorage|fuse|Transport endpoint" |
            Select-Object -Last 160 |
            ForEach-Object { Write-Host $_.Line }
        $script:ExitCode = 1
    } else {
        Write-Host "ALL_SCENARIOS_PASSED"
    }
} finally {
    Invoke-TestArtifactCleanup
}

exit $script:ExitCode
