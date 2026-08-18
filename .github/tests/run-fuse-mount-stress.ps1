[CmdletBinding()]
param(
    [string]$Serial = $env:ANDROID_SERIAL,
    [string]$AppId = "me.fakerqu.test.storageredirect",
    [int]$Iterations = 5,
    [int]$AbnormalIterations = 2,
    [int]$TimeoutSeconds = 180,
    [int]$DirectoryPressureCount = 0,
    [switch]$DirectoryPressureOnly
)

$ErrorActionPreference = "Stop"

if ($Iterations -lt 1 -or $AbnormalIterations -lt 0 -or $AbnormalIterations -gt $Iterations) {
    throw "Iterations 与 AbnormalIterations 参数范围无效。"
}
if ($DirectoryPressureCount -lt 0 -or $DirectoryPressureCount -gt 256) {
    throw "DirectoryPressureCount 参数范围无效。"
}

if ([string]::IsNullOrWhiteSpace($Serial)) {
    $devices = @(& adb devices | Select-Object -Skip 1 | Where-Object { $_ -match "`tdevice" })
    if ($devices.Count -ne 1) {
        throw "检测到多个设备或未检测到设备，请显式传入 -Serial。"
    }
    $Serial = ($devices[0] -split "\s+")[0]
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "../..")).Path
$runner = Join-Path $repoRoot ".github/tests/run-storage-redirect-scenarios.ps1"
$logRoot = Join-Path $repoRoot "temp/fuse-mount-stress"
New-Item -ItemType Directory -Path $logRoot -Force | Out-Null

$stateRoot = "/data/adb/modules/storage.redirect.x/tmp/mount_state"
$statePattern = "$stateRoot/${AppId}_*.state"
$pressureRoot = "/storage/emulated/0/Download/SrtCachePressure"
$pressureBackendRoot = "/data/media/0/Download/SrtCachePressure"

function Invoke-Adb {
    param([string[]]$Arguments)
    $output = & adb -s $Serial @Arguments | ForEach-Object { $_ -replace "`r", "" }
    if ($LASTEXITCODE -ne 0) {
        throw "ADB 命令失败：adb -s $Serial $($Arguments -join ' ')"
    }
    @($output)
}

function Invoke-Su {
    param([string]$Command)
    $normalized = $Command.Replace("`r", "")
    $escaped = $normalized.Replace("'", "'\''")
    $output = & adb -s $Serial shell "su -c '$escaped'" | ForEach-Object { $_ -replace "`r", "" }
    if ($LASTEXITCODE -ne 0) {
        throw "root shell 命令失败：$Command"
    }
    @($output)
}

function Get-FuseChildren {
    $command = @"
for state in $statePattern; do
  [ -f "`$state" ] || continue
  sed -n 's/^fuse_child=//p' "`$state"
done
true
"@
    $lines = @(Invoke-Su $command)
    @(
        $lines |
            Where-Object { $_ -match '^\d+:\d+$' } |
            ForEach-Object { [int]($_ -split ':', 2)[0] } |
            Sort-Object -Unique
    )
}

function Get-MountStateFiles {
    $command = @"
for state in $statePattern; do
  [ -f "`$state" ] && echo "`$state"
done
true
"@
    @(Invoke-Su $command | Where-Object { $_ -like "*.state" })
}

function Wait-TestCleanup {
    param([int]$Timeout = 30)
    $deadline = (Get-Date).AddSeconds($Timeout)
    do {
        $states = @(Get-MountStateFiles)
        $children = @(Get-FuseChildren)
        if ($states.Count -eq 0 -and $children.Count -eq 0) {
            return $true
        }
        Start-Sleep -Milliseconds 250
    } while ((Get-Date) -lt $deadline)
    Write-Warning "测试 APP 清理超时：states=$($states -join ',') children=$($children -join ',')"
    $false
}

function Stop-TestApp {
    try { Invoke-Adb @("shell", "am", "force-stop", $AppId) | Out-Null } catch { Write-Warning "测试 APP 强停失败：$_" }
    try { Wait-TestCleanup -Timeout 30 | Out-Null } catch { Write-Warning "测试 APP 清理检查失败：$_" }
}

function Invoke-DirectoryCachePressure {
    param([int]$Count)
    if ($Count -le 0) { return }
    $command = "rm -rf '$pressureBackendRoot'; mkdir -p '$pressureBackendRoot'; " +
        "i=0; while [ `$i -lt $Count ]; do mkdir -p '$pressureBackendRoot/dir_'`$i; i=`$((i + 1)); done; true"
    Invoke-Su $command | Out-Null
    for ($index = 0; $index -lt $Count; $index++) {
        Invoke-Adb @(
            "shell", "am", "broadcast", "-n", "$AppId/.receiver.TestCaseReceiver",
            "-a", "me.fakerqu.test.storageredirection.TEST_CASE",
            "--es", "test_case", "file_list_dir",
            "--es", "file_dir", "$pressureRoot/dir_$index"
        ) | Out-Null
        Start-Sleep -Milliseconds 100
    }
    Write-Host "  directory_pressure count=$Count root=$pressureRoot"
}

function Start-ScenarioJob {
    param([string]$LogPath)
    Start-Job -ScriptBlock {
        param($Root, $RunnerPath, $DeviceSerial, $OutputPath)
        Set-Location $Root
        $env:ANDROID_SERIAL = $DeviceSerial
        $env:RUN_FUSE_DAEMON_SCENARIOS = "1"
        $env:SRT_FAIL_FAST = "0"
        $env:SRT_SCENARIOS = "8"
        & powershell -NoProfile -ExecutionPolicy Bypass -File $RunnerPath -SkipBasicAll *> $OutputPath
        [int]$LASTEXITCODE
    } -ArgumentList $repoRoot, $runner, $Serial, $LogPath
}

function Wait-ScenarioJob {
    param(
        [System.Management.Automation.Job]$Job,
        [bool]$KillChild,
        [int]$Timeout
    )
    $deadline = (Get-Date).AddSeconds($Timeout)
    $killed = $false
    $pressureDone = $false
    while ($Job.State -eq "Running" -and (Get-Date) -lt $deadline) {
        if (-not $pressureDone -and $DirectoryPressureCount -gt 0) {
            $pressureChildren = @(Get-FuseChildren)
            $pressureReady = (Test-Path -LiteralPath $LogPath -PathType Leaf) -and
                [bool](Select-String -LiteralPath $LogPath -Pattern "PASS scenario-8/sandbox-rule-hit" -Quiet)
            if ($pressureChildren.Count -gt 0 -and $pressureReady) {
                Invoke-DirectoryCachePressure -Count $DirectoryPressureCount
                $pressureDone = $true
                if ($DirectoryPressureOnly) {
                    Stop-Job -Job $Job -ErrorAction SilentlyContinue
                    break
                }
            }
        }
        if ($KillChild -and -not $killed) {
            $children = @(Get-FuseChildren)
            if ($children.Count -gt 0) {
                $child = $children[0]
                Invoke-Su "kill -9 $child"
                Write-Host "  - abnormal_child_killed pid=$child"
                $killed = $true
            }
        }
        Start-Sleep -Milliseconds 200
    }
    if ($Job.State -eq "Running") {
        Stop-Job -Job $Job -ErrorAction SilentlyContinue
        Write-Warning "场景脚本超时，已停止后台任务。"
    }
    $result = @(Receive-Job -Job $Job -ErrorAction SilentlyContinue)
    Remove-Job -Job $Job -Force -ErrorAction SilentlyContinue
    $exitCode = if ($result.Count -gt 0) { [int]$result[-1] } else { -1 }
    if ($DirectoryPressureOnly -and $pressureDone) { $exitCode = 0 }
    [pscustomobject]@{
        Killed = $killed
        PressureDone = $pressureDone
        ExitCode = $exitCode
    }
}

$failures = New-Object System.Collections.Generic.List[string]
try {
    for ($iteration = 1; $iteration -le $Iterations; $iteration++) {
        $abnormal = $iteration -gt ($Iterations - $AbnormalIterations)
        $mode = if ($abnormal) { "abnormal" } else { "normal" }
        $logPath = Join-Path $logRoot ("iteration-{0:D3}-{1}.log" -f $iteration, $mode)
        Write-Host "== stress iteration $iteration/$Iterations mode=$mode =="
        $job = Start-ScenarioJob -LogPath $logPath
        $result = Wait-ScenarioJob -Job $job -KillChild $abnormal -Timeout $TimeoutSeconds
        Stop-TestApp
        $clean = Wait-TestCleanup -Timeout 30

        if ($DirectoryPressureOnly) {
            if (-not $result.PressureDone) {
                $failures.Add("iteration-$iteration 未完成目录压力访问")
            }
            Write-Host "  pressure_result count=$DirectoryPressureCount exit=$($result.ExitCode)"
        } elseif ($abnormal) {
            if (-not $result.Killed) {
                $failures.Add("iteration-$iteration 未观察到可终止的 FUSE child")
            }
            if (-not $clean) {
                $failures.Add("iteration-$iteration 异常 child 退出后资源未清理")
            }
            Write-Host "  abnormal_result killed=$($result.Killed) exit=$($result.ExitCode) cleanup=$clean"
        } elseif ($result.ExitCode -ne 0 -or -not $clean) {
            $failures.Add("iteration-$iteration 正常生命周期失败 exit=$($result.ExitCode) cleanup=$clean")
        } else {
            Write-Host "  normal_result exit=$($result.ExitCode) cleanup=$clean"
        }
    }
}
finally {
    try { Invoke-Su "rm -rf '$pressureBackendRoot'" | Out-Null } catch { Write-Warning "目录压力产物清理失败：$_" }
    Stop-TestApp
}

if ($failures.Count -gt 0) {
    Write-Host "== stress failures =="
    $failures | ForEach-Object { Write-Host $_ }
    exit 1
}

Write-Host "FUSE_MOUNT_STRESS_PASSED iterations=$Iterations abnormal=$AbnormalIterations"
exit 0
