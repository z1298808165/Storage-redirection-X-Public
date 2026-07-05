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
        Fail "Command failed with exit code ${LASTEXITCODE}: $FilePath $($Arguments -join ' ')"
    }
}

function Resolve-BuildPath {
    param([string]$Path)
    if ([System.IO.Path]::IsPathRooted($Path)) {
        return [System.IO.Path]::GetFullPath($Path)
    }
    return [System.IO.Path]::GetFullPath((Join-Path $RepoRoot $Path))
}

function Assert-UnderPath {
    param([string]$Path, [string]$Parent)

    $parentFull = [System.IO.Path]::GetFullPath($Parent).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    $pathFull = [System.IO.Path]::GetFullPath($Path).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    if (-not $pathFull.StartsWith($parentFull + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        Fail "Refusing to operate outside expected directory: $pathFull"
    }
}

function Remove-BuildPath {
    param([string]$Path, [string]$ExpectedParent)
    if (-not (Test-Path -LiteralPath $Path)) { return }
    Assert-UnderPath -Path $Path -Parent $ExpectedParent
    Remove-Item -LiteralPath $Path -Recurse -Force
}

function Write-Utf8LfFile {
    param([string]$Path, [string]$Content)
    $lf = $Content.Replace("`r`n", "`n").Replace("`r", "`n")
    [System.IO.File]::WriteAllText($Path, $lf, $Utf8NoBom)
}

function Normalize-TextFile {
    param([string]$Path)
    [byte[]]$bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
        if ($bytes.Length -eq 3) { $bytes = [byte[]]::new(0) } else { $bytes = $bytes[3..($bytes.Length - 1)] }
    }
    $content = [System.Text.Encoding]::UTF8.GetString($bytes).Replace("`r`n", "`n").Replace("`r", "`n")
    [System.IO.File]::WriteAllText($Path, $content, $Utf8NoBom)
}

function Get-PythonCommand {
    if (-not [string]::IsNullOrWhiteSpace($env:PYTHON)) { return @($env:PYTHON) }
    foreach ($candidate in @(@("python"), @("py", "-3"), @("python3"))) {
        try {
            & $candidate[0] @($candidate | Select-Object -Skip 1) -c "import sys; raise SystemExit(0 if sys.version_info >= (3, 8) else 1)" | Out-Null
            if ($LASTEXITCODE -eq 0) { return $candidate }
        } catch {
        }
    }
    Fail "Unable to find a usable Python 3 interpreter for test-flow verification."
}

function Get-ResolvedVersionData {
    $python = Get-PythonCommand
    $output = & $python[0] @($python | Select-Object -Skip 1) ".github/scripts/resolve_build_version.py" "--include-dirty" "--format" "github"
    if ($LASTEXITCODE -ne 0) { Fail "Unable to resolve test-flow version." }
    $data = @{}
    foreach ($line in $output) {
        if ($line -match '^([^=]+)=(.*)$') { $data[$Matches[1]] = $Matches[2] }
    }
    if ([string]::IsNullOrWhiteSpace($data.version) -or [string]::IsNullOrWhiteSpace($data.version_code)) {
        Fail "Unable to resolve test-flow version."
    }
    return $data
}

function New-ModulePackage {
    param(
        [string]$Version,
        [string]$VersionCode,
        [string]$SoFile,
        [string]$DaemonFile,
        [string]$OutputZip,
        [string]$Abi,
        [string]$WorkDir
    )

    if ($Abi -notin @("arm64-v8a", "x86_64")) { Fail "Unsupported module ABI: $Abi" }
    if (-not (Test-Path -LiteralPath $SoFile)) { Fail "Missing module library: $SoFile" }
    if (-not (Test-Path -LiteralPath $DaemonFile)) { Fail "Missing daemon binary: $DaemonFile" }

    $packageDir = Join-Path $WorkDir "module-package"
    Remove-BuildPath -Path $packageDir -ExpectedParent $WorkDir
    Remove-BuildPath -Path $OutputZip -ExpectedParent $WorkDir
    New-Item -ItemType Directory -Path $packageDir -Force | Out-Null

    Copy-Item -LiteralPath (Join-Path $RepoRoot "assets/zygisk_module") -Destination $packageDir -Recurse -Force
    $templateRoot = Join-Path $packageDir "zygisk_module"
    if (Test-Path -LiteralPath $templateRoot) {
        Get-ChildItem -LiteralPath $templateRoot -Force | ForEach-Object {
            Move-Item -LiteralPath $_.FullName -Destination $packageDir -Force
        }
        Remove-Item -LiteralPath $templateRoot -Force
    }

    Remove-Item -LiteralPath (Join-Path $packageDir "action.sh") -Force -ErrorAction SilentlyContinue
    Copy-Item -LiteralPath (Join-Path $RepoRoot "LICENSE") -Destination (Join-Path $packageDir "LICENSE") -Force
    Copy-Item -LiteralPath (Join-Path $RepoRoot "COPYING") -Destination (Join-Path $packageDir "COPYING") -Force

    New-Item -ItemType Directory -Path (Join-Path $packageDir "zygisk") -Force | Out-Null
    Copy-Item -LiteralPath $SoFile -Destination (Join-Path $packageDir "zygisk/$Abi.so") -Force
    New-Item -ItemType Directory -Path (Join-Path $packageDir "bin") -Force | Out-Null
    Copy-Item -LiteralPath $DaemonFile -Destination (Join-Path $packageDir "bin/srx_daemon") -Force

    $moduleProp = @(
        "id=storage.redirect.x",
        "name=Storage Redirect X",
        "version=v$Version",
        "versionCode=$VersionCode",
        "author=Kindness-Kismet",
        "description=Storage Redirect X Core Module",
        "webui=1"
    ) -join "`n"
    Write-Utf8LfFile -Path (Join-Path $packageDir "module.prop") -Content ($moduleProp + "`n")

    $srxctlPath = (Join-Path $packageDir "bin/srxctl")
    $separator = [System.IO.Path]::DirectorySeparatorChar
    Get-ChildItem -LiteralPath $packageDir -Recurse -File | Where-Object {
        $_.Extension -in @(".sh", ".prop", ".rule") -or
        $_.FullName.Contains("${separator}META-INF${separator}") -or
        $_.FullName -eq $srxctlPath
    } | ForEach-Object { Normalize-TextFile -Path $_.FullName }

    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [System.IO.Compression.ZipFile]::Open($OutputZip, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        $root = (Resolve-Path -LiteralPath $packageDir).Path
        Get-ChildItem -LiteralPath $packageDir -Recurse -File | ForEach-Object {
            $relativePath = $_.FullName.Substring($root.Length).TrimStart([char]92, [char]47).Replace([char]92, [char]47)
            $entry = $zip.CreateEntry($relativePath, [System.IO.Compression.CompressionLevel]::NoCompression)
            $entryStream = $entry.Open()
            $fileStream = [System.IO.File]::OpenRead($_.FullName)
            try { $fileStream.CopyTo($entryStream) } finally { $fileStream.Dispose(); $entryStream.Dispose() }
        }
    } finally {
        $zip.Dispose()
    }
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
    if ($LASTEXITCODE -ne 0) { Fail "adb su command failed: $Command" }
}

Push-Location $RepoRoot
try {
    $versionData = Get-ResolvedVersionData
    $version = [string]$versionData.version
    $versionCode = [string]$versionData.version_code

    $buildRoot = Resolve-BuildPath $BuildDir
    New-Item -ItemType Directory -Path (Join-Path $buildRoot "module-bin") -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $buildRoot "assets") -Force | Out-Null

    Write-Step "Build Rust test binaries for $TargetTriple"
    Invoke-Checked -FilePath "cargo" -Arguments @("test", "--target", $TargetTriple, "--no-run")

    Write-Step "Build SRX module binaries for $TargetTriple"
    Invoke-Checked -FilePath "cargo" -Arguments @("build", "--target", $TargetTriple, "--release")

    $moduleBinDir = Join-Path $buildRoot "module-bin"
    $libSource = Join-Path $RepoRoot "target/$TargetTriple/release/libsrx_core.so"
    $daemonSource = Join-Path $RepoRoot "target/$TargetTriple/release/srx_daemon"
    $libDest = Join-Path $moduleBinDir "libsrx_core.so"
    $daemonDest = Join-Path $moduleBinDir "srx_daemon"
    Copy-Item -LiteralPath $libSource -Destination $libDest -Force
    Copy-Item -LiteralPath $daemonSource -Destination $daemonDest -Force

    Write-Step "Package test-flow module zip"
    $moduleZip = Join-Path $buildRoot "assets/storage.redirect.x-v$version-$ModuleAbi.zip"
    New-ModulePackage -Version $version -VersionCode $versionCode -SoFile $libDest -DaemonFile $daemonDest -OutputZip $moduleZip -Abi $ModuleAbi -WorkDir $buildRoot

    Write-Step "Run Android unit tests and build test APK"
    $gradle = if (Test-Path -LiteralPath (Join-Path $RepoRoot "gradlew.bat")) { Join-Path $RepoRoot "gradlew.bat" } else { "gradle" }
    Invoke-Checked -FilePath $gradle -Arguments @(
        "--no-daemon", "--console=plain", "--stacktrace",
        ":app:testDebugUnitTest",
        ":storageRedirectTestApp:testDebugUnitTest",
        ":storageRedirectTestMediaFileApi:testDebugUnitTest",
        ":storageRedirectTestApp:assembleDebug"
    )

    $testAppApk = Get-TestAppApk
    if ([string]::IsNullOrWhiteSpace($testAppApk)) { Fail "Unable to find test app debug APK under $TestAppApkDir." }

    if ($RunDeviceScenarios -eq "0") {
        Write-Host "RUN_DEVICE_SCENARIOS=0: device scenario suite skipped."
        exit 0
    }

    Write-Step "Verify connected device and installed module state"
    Invoke-Checked -FilePath "adb" -Arguments @("wait-for-device")
    Invoke-Checked -FilePath "adb" -Arguments @("shell", "while [ `"`$(getprop sys.boot_completed)`" != `"1`" ]; do sleep 2; done")

    if ($InstallModule -ne "0") {
        Write-Step "Install freshly built module zip and reboot test device"
        $remoteZip = "/data/local/tmp/storage.redirect.x-test-flow.zip"
        Invoke-Checked -FilePath "adb" -Arguments @("push", $moduleZip, $remoteZip)
        Invoke-AdbSu "rm -rf /data/adb/modules_update/storage.redirect.x"
        $installCommand = "if [ -x /data/adb/ksu/bin/ksud ]; then /data/adb/ksu/bin/ksud module install '$remoteZip'; elif command -v ksud >/dev/null 2>&1; then ksud module install '$remoteZip'; else magisk --install-module '$remoteZip'; fi"
        Invoke-AdbSu $installCommand
        try { Invoke-AdbSu "rm -f '$remoteZip'" } catch { Write-Warning $_ }
        Invoke-Checked -FilePath "adb" -Arguments @("reboot")
        Invoke-Checked -FilePath "adb" -Arguments @("wait-for-device")
        Invoke-Checked -FilePath "adb" -Arguments @("shell", "while [ `"`$(getprop sys.boot_completed)`" != `"1`" ]; do sleep 2; done")
    }

    Invoke-AdbSu "id; test -d /data/adb/modules/storage.redirect.x; test ! -e /data/adb/modules/storage.redirect.x/disable; for file in module.prop post-fs-data.sh service.sh sepolicy.rule LICENSE COPYING bin/srx_daemon zygisk/$ModuleAbi.so; do test -s /data/adb/modules/storage.redirect.x/`$file || exit 1; done; cat /data/adb/modules/storage.redirect.x/module.prop" | Out-Null

    Write-Step "Install test APK"
    Invoke-Checked -FilePath "adb" -Arguments @("install", "-r", $testAppApk)

    Write-Step "Run device scenario suite"
    $env:MODULE_ZIP = $moduleZip
    $env:APP_APK = $testAppApk
    & pwsh -NoProfile -ExecutionPolicy Bypass -File ".github/tests/run-storage-redirect-scenarios.ps1"
    if ($LASTEXITCODE -ne 0) { Fail "Device scenario suite failed." }
} finally {
    Pop-Location
}
