param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot "..")).Path
$RunDeviceScenarios = if ([string]::IsNullOrWhiteSpace($env:RUN_DEVICE_SCENARIOS)) { "1" } else { $env:RUN_DEVICE_SCENARIOS }
$InstallModule = if ([string]::IsNullOrWhiteSpace($env:SRX_TEST_INSTALL_MODULE)) { "1" } else { $env:SRX_TEST_INSTALL_MODULE }
$TargetTriple = if ([string]::IsNullOrWhiteSpace($env:SRX_TEST_TARGET_TRIPLE)) { "aarch64-linux-android" } else { $env:SRX_TEST_TARGET_TRIPLE }
$ModuleAbi = if ([string]::IsNullOrWhiteSpace($env:SRX_TEST_MODULE_ABI)) { "arm64-v8a" } else { $env:SRX_TEST_MODULE_ABI }
$BuildDir = if ([string]::IsNullOrWhiteSpace($env:SRX_TEST_BUILD_DIR)) { "build/test-flow" } else { $env:SRX_TEST_BUILD_DIR }
$TestAppApkDir = Join-Path $RepoRoot "tests/storage-redirect-test/app/build/outputs/apk/debug"
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)

if ($TargetTriple -notin @("aarch64-linux-android", "x86_64-linux-android")) {
    throw "不支持的 Android target：$TargetTriple"
}
if ($ModuleAbi -notin @("arm64-v8a", "x86_64")) {
    throw "不支持的模块 ABI：$ModuleAbi"
}

function Write-Step {
    param([string]$Message)
    Write-Host "==> $Message"
}

function Fail {
    param([string]$Message)
    throw $Message
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)] [string]$FilePath,
        [string[]]$Arguments = @()
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        Fail "命令执行失败，退出码 ${LASTEXITCODE}：$FilePath $($Arguments -join ' ')"
    }
}

function Resolve-BuildPath {
    param([string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $RepoRoot $Path))
}

function Write-Utf8LfFile {
    param([string]$Path, [string]$Content)
    $lf = $Content.Replace("`r`n", "`n").Replace("`r", "`n")
    [System.IO.File]::WriteAllText($Path, $lf, $Utf8NoBom)
}

function Get-PythonCommand {
    if (-not [string]::IsNullOrWhiteSpace($env:PYTHON)) {
        return [pscustomobject]@{ FilePath = $env:PYTHON; Arguments = @() }
    }
    $candidates = @(
        [pscustomobject]@{ FilePath = "python"; Arguments = @() },
        [pscustomobject]@{ FilePath = "py"; Arguments = @("-3") },
        [pscustomobject]@{ FilePath = "python3"; Arguments = @() }
    )
    foreach ($candidate in $candidates) {
        try {
            & $candidate.FilePath @($candidate.Arguments) -c "import sys; raise SystemExit(0 if sys.version_info >= (3, 8) else 1)" | Out-Null
            if ($LASTEXITCODE -eq 0) { return $candidate }
        } catch {
        }
    }
    Fail "找不到可用于测试流验证的 Python 3 解释器。"
}

function Get-ResolvedVersionData {
    $python = Get-PythonCommand
    $output = & $python.FilePath @($python.Arguments) ".github/scripts/resolve_build_version.py" "--include-dirty" "--format" "github"
    if ($LASTEXITCODE -ne 0) { Fail "无法解析测试流版本。" }
    $data = @{}
    foreach ($line in $output) {
        if ($line -match '^([^=]+)=(.*)$') { $data[$Matches[1]] = $Matches[2] }
    }
    if ([string]::IsNullOrWhiteSpace($data.version) -or [string]::IsNullOrWhiteSpace($data.version_code)) {
        Fail "无法解析测试流版本。"
    }
    return $data
}

function Get-TestAppApk {
    if (-not (Test-Path -LiteralPath $TestAppApkDir)) { return $null }
    Get-ChildItem -LiteralPath $TestAppApkDir -Filter "*-debug.apk" | Select-Object -First 1 -ExpandProperty FullName
}

function Invoke-AdbSu {
    param([string]$Command)
    $escaped = $Command.Replace("'", "'\''")
    & adb shell "su 0 sh -c '$escaped'"
    if ($LASTEXITCODE -eq 0) { return }
    & adb shell "su -c '$escaped'"
    if ($LASTEXITCODE -ne 0) { Fail "adb su 命令执行失败：$Command" }
}

function Wait-DeviceBootCompleted {
    Invoke-Checked -FilePath "adb" -Arguments @("wait-for-device")
    for ($attempt = 1; $attempt -le 90; $attempt++) {
        $bootCompleted = (& adb shell getprop sys.boot_completed | Out-String).Trim()
        if ($LASTEXITCODE -eq 0 -and $bootCompleted -eq "1") { return }
        Start-Sleep -Seconds 2
    }
    Fail "设备在 180 秒内未完成启动。"
}

function Assert-ModuleRuntimeState {
    $command = @'
id; module_dir=/data/adb/modules/storage.redirect.x; logs_dir="$module_dir/logs"; boot_id=$(cat /proc/sys/kernel/random/boot_id 2>/dev/null || true); daemon_pid=$(cat "$logs_dir/.srx_daemon.pid" 2>/dev/null || true); test -d "$module_dir"; test ! -e "$module_dir/disable"; for file in module.prop post-fs-data.sh service.sh sepolicy.rule LICENSE COPYING bin/srx_daemon zygisk/__MODULE_ABI__.so; do test -s "$module_dir/$file" || exit 1; done; test -d "$module_dir/config/apps"; test -d "$logs_dir"; { [ -z "$boot_id" ] || [ "$(cat "$module_dir/.boot_ok" 2>/dev/null || true)" = "$boot_id" ] || test -f "$logs_dir/boot_${boot_id}.marker"; }; { [ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null || pidof srx_daemon >/dev/null 2>&1; }; cat "$module_dir/module.prop"
'@.Replace("__MODULE_ABI__", $ModuleAbi)
    Invoke-AdbSu $command | Out-Null
}

Push-Location $RepoRoot
try {
    Write-Step "在 WSL 中构建并测试 Rust/Android 产物"
    $wsl = Get-Command wsl.exe -ErrorAction Stop
    Invoke-Checked -FilePath $wsl.Source -Arguments @(
        "--cd", $RepoRoot, "--exec", "bash", "-lc",
        'build_dir=$(wslpath -a "$1") || exit; shift; exec env SRX_TEST_BUILD_DIR="$build_dir" "$@"',
        "_", (Resolve-BuildPath $BuildDir),
        "RUN_DEVICE_SCENARIOS=0",
        "SRX_TEST_TARGET_TRIPLE=$TargetTriple",
        "SRX_TEST_MODULE_ABI=$ModuleAbi",
        "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=aarch64-linux-android29-clang",
        "CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER=x86_64-linux-android29-clang",
        "bash", "scripts/verify-test-flow.sh"
    )

    $versionData = Get-ResolvedVersionData
    $version = [string]$versionData.version
    $buildRoot = Resolve-BuildPath $BuildDir
    $moduleZip = Join-Path $buildRoot "assets/storage.redirect.x-v$version-$ModuleAbi.zip"
    if (-not (Test-Path -LiteralPath $moduleZip -PathType Leaf)) {
        Fail "WSL 未生成测试模块 zip：$moduleZip"
    }
    $testAppApk = Get-TestAppApk
    if ([string]::IsNullOrWhiteSpace($testAppApk)) { Fail "在 $TestAppApkDir 下找不到测试应用 debug APK。" }

    if ($RunDeviceScenarios -eq "0") {
        Write-Host "RUN_DEVICE_SCENARIOS=0：已跳过设备场景套件。"
        exit 0
    }

    Write-Step "验证已连接设备和已安装模块状态"
    Wait-DeviceBootCompleted

    if ($InstallModule -ne "0") {
        Write-Step "安装刚构建的模块 zip 并重启测试设备"
        $remoteZip = "/data/local/tmp/storage.redirect.x-test-flow.zip"
        Invoke-Checked -FilePath "adb" -Arguments @("push", $moduleZip, $remoteZip)
        Invoke-AdbSu "rm -rf /data/adb/modules_update/storage.redirect.x"
        $installCommand = "if [ -x /data/adb/ksu/bin/ksud ]; then /data/adb/ksu/bin/ksud module install '$remoteZip'; elif command -v ksud >/dev/null 2>&1; then ksud module install '$remoteZip'; else magisk --install-module '$remoteZip'; fi"
        Invoke-AdbSu $installCommand
        try { Invoke-AdbSu "rm -f '$remoteZip'" } catch { Write-Warning $_ }
        Invoke-Checked -FilePath "adb" -Arguments @("reboot")
        Wait-DeviceBootCompleted
    }

    Assert-ModuleRuntimeState

    # 等待 FBE CE 解锁（/data/media/0 可写）。boot 后 15-25 秒内 CE 密钥尚未就绪
    # （真机实测 ENOTKEY=126）：共享宿主的策略登记与重定向根 mkdir 会连续失败、
    # 宿主反复自退，应用回退 scoped 会话后 fresh-app 首操作大面积 ENOENT——
    # 这是全量跑失败集（scenario 4/5/9/12/15/20/22/24/36）的根因链。
    # 场景必须等共享存储真正可写后再开跑；CI 模拟器解锁时序不同所以一直未暴露。
    Write-Step "等待共享存储 CE 解锁（/data/media/0 可写）"
    $unlockDeadline = (Get-Date).AddSeconds(180)
    $probe = ""
    do {
        $probe = (& adb shell "su 0 sh -c 'test -w /data/media/0 && echo unlocked'" 2>$null | Out-String).Trim()
        if ($probe -match "unlocked") { break }
        Start-Sleep -Seconds 3
    } while ((Get-Date) -lt $unlockDeadline)
    if ($probe -notmatch "unlocked") {
        Fail "等待 180 秒后 /data/media/0 仍不可写（FBE CE 未解锁），共享宿主无法稳定工作。"
    }

    Write-Step "安装测试 APK"
    Invoke-Checked -FilePath "adb" -Arguments @("install", "-r", $testAppApk)

    Write-Step "运行 native 单元与 ZN 回调测试"
    $nativeRemote = (& adb shell mktemp -d /data/local/tmp/srx-native-tests.XXXXXX | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $nativeRemote -notmatch '^/data/local/tmp/srx-native-tests\.[A-Za-z0-9]+$') {
        Fail "无法创建设备 native 测试目录。"
    }
    try {
        Invoke-Checked -FilePath "adb" -Arguments @("push", "$(Join-Path $buildRoot 'native')/.", "$nativeRemote/")
        Invoke-Checked -FilePath "adb" -Arguments @("push", ".github/tests/run-zn-runtime-fixture.sh", ".github/tests/grant-storage-test-permissions.sh", "$nativeRemote/")
        Invoke-AdbSu "sh '$nativeRemote/run-zn-runtime-fixture.sh' '$nativeRemote/zn-runtime-fixture' '/data/adb/modules/storage.redirect.x/zygisk/$ModuleAbi.so' '$nativeRemote/grant-storage-test-permissions.sh' '$nativeRemote/srx_core-tests' '$nativeRemote/srx_daemon-tests'"
    } finally {
        Invoke-Checked -FilePath "adb" -Arguments @("shell", "rm", "-rf", $nativeRemote)
    }

    Write-Step "运行设备场景套件"
    $env:MODULE_ZIP = $moduleZip
    $env:APP_APK = $testAppApk
    if ($env:SRX_DETACHED_SCENARIOS -match '^(1|true|TRUE|yes|YES)$') {
        # 脱离模式：交互式执行器约 120 秒回收其会话派生的全部进程——Start-Process 子进程
        # 与 adb server 都在同一 Job 对象里，中途被杀后设备锁与配置恢复不会执行。彻底
        # 方案：场景段打包成 wrapper 脚本，经 WMI Win32_Process.Create 由 WMI 服务代为
        # 创建进程，完全脱离工具会话的进程树与 Job，可一直运行到结束。
        # wrapper 内部用 Start-Process -Wait 隔离运行场景脚本：脚本以 `exit` 结束，
        # 直接 & 调用会连带终止 wrapper，退出码标记将无人写入。
        $runRoot = Join-Path $buildRoot "scenario-run"
        New-Item -ItemType Directory -Path $runRoot -Force | Out-Null
        $stamp = Get-Date -Format "yyyyMMdd-HHmmss"
        $stdout = Join-Path $runRoot "stdout-$stamp.log"
        $stderr = Join-Path $runRoot "stderr-$stamp.log"
        $exitFile = Join-Path $runRoot "exit-$stamp.txt"
        $wrapper = Join-Path $runRoot "wrapper-$stamp.ps1"
        Remove-Item -LiteralPath $exitFile -Force -ErrorAction SilentlyContinue
        $scenarioScript = Join-Path $RepoRoot ".github/tests/run-storage-redirect-scenarios.ps1"
        $adbPath = (Get-Command adb -ErrorAction Stop).Source
        $pwshPath = (Get-Command pwsh -ErrorAction Stop).Source
        # WMI 进程不继承工具会话环境，PATH 与关键变量必须显式烘焙进 wrapper。
        $escapedPath = ($env:Path -replace "'", "''")
        $wrapperLines = @(
            "`$ErrorActionPreference = 'Continue'",
            "Set-Location '$RepoRoot'",
            "`$env:Path = '$escapedPath'",
            "`$env:SRT_FAIL_FAST = '0'"
        )
        # 场景脚本每次 adb 都带 `-s $Serial`，而 `$Serial` 默认取自 `$env:ANDROID_SERIAL`；
        # 本机同时连着模拟器与真机（实测 `emulator-5556` + 真机 `ASUS_AI2501_A`）时，
        # 不显式透传会让子进程以「检测到多个设备」直接退出，表现为场景一条未跑就失败。
        # WMI 不继承工具会话环境，只能在 wrapper 里显式烘焙。
        if (-not [string]::IsNullOrWhiteSpace($env:ANDROID_SERIAL)) {
            $wrapperLines += "`$env:ANDROID_SERIAL = '$($env:ANDROID_SERIAL -replace "'", "''")'"
        }
        if (-not [string]::IsNullOrWhiteSpace($env:SRT_SCENARIOS)) {
            $wrapperLines += "`$env:SRT_SCENARIOS = '$($env:SRT_SCENARIOS -replace "'", "''")'"
        }
        # 版本级场景排除与 CI 矩阵同语义（如 A17 排除依赖外网的场景 39）：本地复现
        # 该类形态时必须一起透传，否则默认全量会跑出与 CI 不一致的范围。
        if (-not [string]::IsNullOrWhiteSpace($env:SRT_VERSION_SCENARIO_EXCLUDE)) {
            $wrapperLines += "`$env:SRT_VERSION_SCENARIO_EXCLUDE = '$($env:SRT_VERSION_SCENARIO_EXCLUDE -replace "'", "''")'"
        }
        $wrapperLines += @(
            "# adb server 若由工具会话派生会随 Job 回收中途死亡；先杀掉再由本进程树重建。",
            "& '$adbPath' kill-server 2>`$null",
            "& '$adbPath' start-server 2>`$null",
            "`$proc = Start-Process -FilePath '$pwshPath' -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','$scenarioScript') -WindowStyle Hidden -RedirectStandardOutput '$stdout' -RedirectStandardError '$stderr' -PassThru -Wait",
            "`$code = if (`$null -ne `$proc.ExitCode) { `$proc.ExitCode } else { 1 }",
            "Set-Content -LiteralPath '$exitFile' -Value `$code",
            "exit `$code"
        )
        Write-Utf8LfFile -Path $wrapper -Content (($wrapperLines -join "`n") + "`n")
        $commandLine = "`"$pwshPath`" -NoProfile -ExecutionPolicy Bypass -File `"$wrapper`""
        $creation = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{ CommandLine = $commandLine }
        if ($creation.ReturnValue -ne 0) { Fail "WMI 进程创建失败 ReturnValue=$($creation.ReturnValue)" }
        $childPid = $creation.ProcessId
        $meta = "pid=$childPid`nexit_file=$exitFile`nstdout=$stdout`nstderr=$stderr`nwrapper=$wrapper"
        Write-Utf8LfFile -Path (Join-Path $runRoot "run-$stamp.txt") -Content ($meta + "`n")
        Write-Host "场景子进程已脱离启动 pid=$childPid"
        Write-Host "stdout=$stdout"
        Write-Host "stderr=$stderr"
        Write-Host "exit_marker=$exitFile"
        exit 0
    }
    & pwsh -NoProfile -ExecutionPolicy Bypass -File ".github/tests/run-storage-redirect-scenarios.ps1"
    if ($LASTEXITCODE -ne 0) { Fail "设备场景套件执行失败。" }
} finally {
    Pop-Location
}
