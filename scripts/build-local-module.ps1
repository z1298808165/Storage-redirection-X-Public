param(
    [string]$Version = "",
    [int]$VersionCode = 0,
    [string]$OutputDir = "build",

    [switch]$SkipBuild,
    [switch]$NoAdb
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$BuildVersionBaselinePath = Join-Path $RepoRoot ".github\build-version-baseline.json"
$Utf8NoBom = New-Object System.Text.UTF8Encoding($false)

function Write-Step {
    param([string]$Message)
    Write-Host ""
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Fail {
    param([string]$Message)
    throw $Message
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$Arguments = @()
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        Fail "命令执行失败，退出代码 ${LASTEXITCODE}: $FilePath $($Arguments -join ' ')"
    }
}

function Get-CargoTargetDirectory {
    $metadataJson = & cargo metadata --format-version 1 --no-deps
    if ($LASTEXITCODE -ne 0) {
        Fail "无法解析 Cargo 目标目录"
    }

    $metadata = $metadataJson | ConvertFrom-Json
    if ([string]::IsNullOrWhiteSpace([string]$metadata.target_directory)) {
        Fail "Cargo metadata 未返回目标目录"
    }
    return [System.IO.Path]::GetFullPath([string]$metadata.target_directory)
}

function Get-FirstExistingPath {
    param([string[]]$Paths)

    foreach ($path in $Paths) {
        if ([string]::IsNullOrWhiteSpace($path)) {
            continue
        }
        if (Test-Path -LiteralPath $path) {
            return (Resolve-Path -LiteralPath $path).Path
        }
    }

    return $null
}

function Get-LatestChildDirectory {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return $null
    }

    $dirs = Get-ChildItem -LiteralPath $Path -Directory | Sort-Object {
        try {
            [version]$_.Name
        } catch {
            [version]"0.0.0"
        }
    } -Descending

    if ($dirs.Count -eq 0) {
        return $null
    }

    return $dirs[0].FullName
}

function Add-PathPrefix {
    param([string]$Path)

    if ([string]::IsNullOrWhiteSpace($Path) -or -not (Test-Path -LiteralPath $Path)) {
        return
    }

    $resolved = (Resolve-Path -LiteralPath $Path).Path
    $parts = $env:Path -split [System.IO.Path]::PathSeparator
    if ($parts -notcontains $resolved) {
        $env:Path = "$resolved$([System.IO.Path]::PathSeparator)$env:Path"
    }
}

function Initialize-AndroidEnvironment {
    $sdk = Get-FirstExistingPath @(
        $env:ANDROID_HOME,
        $env:ANDROID_SDK_ROOT,
        (Join-Path $env:LOCALAPPDATA "Android\Sdk")
    )

    if ($sdk) {
        $env:ANDROID_HOME = $sdk
        $env:ANDROID_SDK_ROOT = $sdk
        Add-PathPrefix (Join-Path $sdk "platform-tools")

        $cmakeRoot = Join-Path $sdk "cmake"
        $cmakeDir = Get-LatestChildDirectory $cmakeRoot
        if ($cmakeDir) {
            Add-PathPrefix (Join-Path $cmakeDir "bin")
        }
    }

    $ndk = Get-FirstExistingPath @(
        $env:ANDROID_NDK_HOME,
        $env:ANDROID_NDK_ROOT,
        $(if ($sdk) { Get-LatestChildDirectory (Join-Path $sdk "ndk") } else { $null })
    )

    if ($ndk) {
        $env:ANDROID_NDK_HOME = $ndk
        $env:ANDROID_NDK_ROOT = $ndk
        Add-PathPrefix (Join-Path $ndk "toolchains\llvm\prebuilt\windows-x86_64\bin")
    }
}

function Get-CargoPackageVersion {
    $cargoToml = Join-Path $RepoRoot "Cargo.toml"
    $line = Get-Content -LiteralPath $cargoToml | Where-Object { $_ -match '^\s*version\s*=\s*"([^"]+)"' } | Select-Object -First 1
    if (-not $line -or $line -notmatch '^\s*version\s*=\s*"([^"]+)"') {
        Fail "无法从 Cargo.toml 读取包版本"
    }

    return $Matches[1]
}

function Invoke-GitText {
    param(
        [string[]]$Arguments,
        [switch]$AllowFailure
    )

    $output = & git @Arguments 2>$null
    if ($LASTEXITCODE -ne 0) {
        if ($AllowFailure) {
            return $null
        }
        Fail "git 命令执行失败: git $($Arguments -join ' ')"
    }

    return ($output -join "`n").Trim()
}

function Get-CargoVersionFromText {
    param([string]$Text)

    if ($Text -match '(?m)^version\s*=\s*"([^"]+)"') {
        return $Matches[1]
    }

    return $null
}

function Get-CargoVersionAtCommit {
    param([string]$Commit)

    $text = Invoke-GitText -Arguments @("show", "${Commit}:Cargo.toml") -AllowFailure
    if ($null -eq $text) {
        return $null
    }

    return Get-CargoVersionFromText -Text $text
}

function Get-HeadCargoVersion {
    return Get-CargoVersionAtCommit -Commit "HEAD"
}

function Get-VersionStartCommit {
    param([string]$Version)

    $commitsText = Invoke-GitText -Arguments @("rev-list", "--first-parent", "--reverse", "HEAD", "--", "Cargo.toml") -AllowFailure
    if ([string]::IsNullOrWhiteSpace($commitsText)) {
        return $null
    }

    $previousVersion = $null
    $start = $null
    foreach ($commit in ($commitsText -split "`n" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })) {
        $commitVersion = Get-CargoVersionAtCommit -Commit $commit.Trim()
        if ($commitVersion -eq $Version -and $previousVersion -ne $Version) {
            $start = $commit.Trim()
        }
        $previousVersion = $commitVersion
    }

    return $start
}

function Test-AutoManifestCommit {
    param([string]$Commit)

    $subject = Invoke-GitText -Arguments @("log", "-1", "--pretty=%s", $Commit) -AllowFailure
    if ($null -eq $subject) {
        return $false
    }

    return $subject.StartsWith("CI：更新更新清单") -or $subject.StartsWith("发布：更新更新清单")
}

function Test-WorktreeDirty {
    $status = Invoke-GitText -Arguments @("status", "--porcelain") -AllowFailure
    return -not [string]::IsNullOrWhiteSpace($status)
}

function Get-BuildCountOffset {
    param([string]$BaseVersion)

    if ($BaseVersion -eq "1.2.57") {
        return 285
    }

    return 0
}

function Get-BuildCountBaseline {
    param([string]$BaseVersion)

    if (-not (Test-Path -LiteralPath $BuildVersionBaselinePath)) {
        return $null
    }

    try {
        $baseline = Get-Content -LiteralPath $BuildVersionBaselinePath -Raw -Encoding UTF8 | ConvertFrom-Json
        if ($null -eq $baseline.buildCounts) {
            return $null
        }
        $property = $baseline.buildCounts.PSObject.Properties[$BaseVersion]
        if ($null -eq $property) {
            return $null
        }
        return [int]$property.Value
    } catch {
        return $null
    }
}

function Update-BuildCountBaseline {
    param(
        [string]$BaseVersion,
        [int]$BuildCount
    )

    if ($BuildCount -lt 1) {
        Fail "构建次数必须为正数，当前为: $BuildCount"
    }

    if (Test-Path -LiteralPath $BuildVersionBaselinePath) {
        try {
            $baseline = Get-Content -LiteralPath $BuildVersionBaselinePath -Raw -Encoding UTF8 | ConvertFrom-Json
        } catch {
            $baseline = [pscustomobject]@{}
        }
    } else {
        $baseline = [pscustomobject]@{}
    }

    if ($null -eq $baseline.PSObject.Properties["buildCounts"]) {
        $baseline | Add-Member -NotePropertyName "buildCounts" -NotePropertyValue ([pscustomobject]@{})
    }

    $previous = 0
    $property = $baseline.buildCounts.PSObject.Properties[$BaseVersion]
    if ($null -ne $property) {
        $previous = [int]$property.Value
    }
    $next = [Math]::Max($previous, $BuildCount)
    if ($null -eq $property) {
        $baseline.buildCounts | Add-Member -NotePropertyName $BaseVersion -NotePropertyValue $next
    } else {
        $property.Value = $next
    }

    $orderedCounts = [ordered]@{}
    $baseline.buildCounts.PSObject.Properties.Name | Sort-Object { [version]$_ } | ForEach-Object {
        $orderedCounts[$_] = [int]$baseline.buildCounts.PSObject.Properties[$_].Value
    }
    $orderedBaseline = [ordered]@{
        schema = 1
        buildCounts = $orderedCounts
    }
    $json = $orderedBaseline | ConvertTo-Json -Depth 5 -Compress
    Write-Utf8LfFile -Path $BuildVersionBaselinePath -Content ($json + "`n")
}

function Resolve-LocalVersion {
    param([string]$BaseVersion)

    $parts = $BaseVersion.Split(".")
    if ($parts.Count -ne 3) {
        Fail "Cargo.toml 的版本格式必须为 MAJOR.MINOR.PATCH，当前为: $BaseVersion"
    }

    $major = [int]$parts[0]
    $minor = [int]$parts[1]
    $patch = [int]$parts[2]
    $baseCode = ($major * 1000000) + ($minor * 10000) + ($patch * 100)
    $headVersion = Get-HeadCargoVersion
    $start = $null
    if ($headVersion -eq $BaseVersion) {
        $start = Get-VersionStartCommit -Version $BaseVersion
    }
    $count = 0
    if (-not [string]::IsNullOrWhiteSpace($start)) {
        $commitsText = Invoke-GitText -Arguments @("rev-list", "--first-parent", "--reverse", "$start..HEAD") -AllowFailure
        foreach ($commit in ($commitsText -split "`n" | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })) {
            if (-not (Test-AutoManifestCommit -Commit $commit.Trim())) {
                $count++
            }
        }
    }

    if (Test-WorktreeDirty) {
        if ($headVersion -ne $BaseVersion) {
            $count = 0
        }
        $count++
    }

    $buildCount = [Math]::Max($count, 1) + (Get-BuildCountOffset -BaseVersion $BaseVersion)
    $baselineCount = Get-BuildCountBaseline -BaseVersion $BaseVersion
    if ($null -ne $baselineCount) {
        $buildCount = [Math]::Max($buildCount, $baselineCount + 1)
    }
    $resolvedVersionCode = $baseCode - 100 + [Math]::Min($buildCount, 99)
    return @{
        Version = "$BaseVersion-ci.$buildCount"
        VersionCode = $resolvedVersionCode
    }
}

function Assert-UnderPath {
    param(
        [string]$Path,
        [string]$Parent
    )

    $parentFull = [System.IO.Path]::GetFullPath($Parent).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    $pathFull = [System.IO.Path]::GetFullPath($Path).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)

    if (-not $pathFull.StartsWith($parentFull + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        Fail "拒绝删除预期父目录之外的路径: $pathFull"
    }
}

function Remove-LocalPath {
    param(
        [string]$Path,
        [string]$ExpectedParent
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }

    Assert-UnderPath -Path $Path -Parent $ExpectedParent
    Remove-Item -LiteralPath $Path -Recurse -Force
}

function Write-Utf8LfFile {
    param(
        [string]$Path,
        [string]$Content
    )

    $lf = $Content.Replace("`r`n", "`n").Replace("`r", "`n")
    [System.IO.File]::WriteAllText($Path, $lf, $Utf8NoBom)
}

function Normalize-TextFile {
    param([string]$Path)

    [byte[]]$bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
        if ($bytes.Length -eq 3) {
            $bytes = [byte[]]::new(0)
        } else {
            $bytes = $bytes[3..($bytes.Length - 1)]
        }
    }

    $content = [System.Text.Encoding]::UTF8.GetString($bytes).Replace("`r`n", "`n").Replace("`r", "`n")
    [System.IO.File]::WriteAllText($Path, $content, $Utf8NoBom)
}

function Build-ModuleZip {
    param(
        [string]$PackageDir,
        [string]$ZipPath,
        [string]$ModuleVersion,
        [int]$ModuleVersionCode
    )

    $templateDir = Join-Path $RepoRoot "assets\zygisk_module"
    $releaseDir = Join-Path (Join-Path (Get-CargoTargetDirectory) "aarch64-linux-android") "release"
    $soFile = Join-Path $releaseDir "libsrx_core.so"
    $daemonFile = Join-Path $releaseDir "srx_daemon"

    if (-not (Test-Path -LiteralPath $soFile)) {
        Fail "缺失构建输出文件: $soFile"
    }
    if (-not (Test-Path -LiteralPath $daemonFile)) {
        Fail "缺失构建输出文件: $daemonFile"
    }

    $outputRoot = Split-Path -Parent $PackageDir
    Remove-LocalPath -Path $PackageDir -ExpectedParent $outputRoot
    Remove-LocalPath -Path $ZipPath -ExpectedParent $outputRoot
    New-Item -ItemType Directory -Path $PackageDir -Force | Out-Null

    Copy-Item -Path (Join-Path $templateDir "*") -Destination $PackageDir -Recurse -Force
    Remove-Item -LiteralPath (Join-Path $PackageDir "action.sh") -Force -ErrorAction SilentlyContinue
    Copy-Item -LiteralPath (Join-Path $RepoRoot "LICENSE") -Destination (Join-Path $PackageDir "LICENSE") -Force
    Copy-Item -LiteralPath (Join-Path $RepoRoot "COPYING") -Destination (Join-Path $PackageDir "COPYING") -Force

    New-Item -ItemType Directory -Path (Join-Path $PackageDir "zygisk") -Force | Out-Null
    Copy-Item -LiteralPath $soFile -Destination (Join-Path $PackageDir "zygisk\arm64-v8a.so") -Force

    New-Item -ItemType Directory -Path (Join-Path $PackageDir "bin") -Force | Out-Null
    Copy-Item -LiteralPath $daemonFile -Destination (Join-Path $PackageDir "bin\srx_daemon") -Force

    $moduleProp = @(
        "id=storage.redirect.x",
        "name=Storage Redirect X",
        "version=v$ModuleVersion",
        "versionCode=$ModuleVersionCode",
        "author=Kindness-Kismet",
        "description=Storage Redirect X Core Module",
        "webui=1"
    ) -join "`n"
    Write-Utf8LfFile -Path (Join-Path $PackageDir "module.prop") -Content ($moduleProp + "`n")

    $srxctlPath = Join-Path $PackageDir "bin\srxctl"
    Get-ChildItem -LiteralPath $PackageDir -Recurse -File | Where-Object {
        $_.Extension -in ".sh", ".prop", ".rule" -or
        $_.FullName.Contains("\META-INF\") -or
        $_.FullName -eq $srxctlPath
    } | ForEach-Object {
        Normalize-TextFile -Path $_.FullName
    }

    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem

    $zip = [System.IO.Compression.ZipFile]::Open($ZipPath, [System.IO.Compression.ZipArchiveMode]::Create)
    try {
        $pkgRoot = (Resolve-Path -LiteralPath $PackageDir).Path
        Get-ChildItem -LiteralPath $PackageDir -Recurse -File | ForEach-Object {
            $relativePath = $_.FullName.Substring($pkgRoot.Length).TrimStart([char]92, [char]47).Replace([char]92, [char]47)
            $keepUncompressed =
                $_.Extension -in @(".sh", ".prop", ".rule") -or
                $relativePath.StartsWith("META-INF/", [System.StringComparison]::Ordinal) -or
                $relativePath -eq "bin/srxctl"
            $compression = if ($keepUncompressed) {
                [System.IO.Compression.CompressionLevel]::NoCompression
            } else {
                [System.IO.Compression.CompressionLevel]::Optimal
            }
            $entry = $zip.CreateEntry($relativePath, $compression)
            $entryStream = $entry.Open()
            $fileStream = [System.IO.File]::OpenRead($_.FullName)
            try {
                $fileStream.CopyTo($entryStream)
            } finally {
                $fileStream.Dispose()
                $entryStream.Dispose()
            }
        }
    } finally {
        $zip.Dispose()
    }
}

function Test-ModuleZip {
    param([string]$ZipPath)

    Add-Type -AssemblyName System.IO.Compression
    Add-Type -AssemblyName System.IO.Compression.FileSystem

    $zip = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
    try {
        $names = @($zip.Entries | ForEach-Object { $_.FullName })
        foreach ($required in @("LICENSE", "COPYING", "module.prop", "customize.sh", "zygisk/arm64-v8a.so", "bin/srx_daemon", "webroot/index.html")) {
            if ($names -notcontains $required) {
                Fail "Zip 文件缺失必需的条目: $required"
            }
        }
        foreach ($entryName in $names) {
            if ($entryName.IndexOf([char]92) -ge 0) {
                Fail "Zip 包含带反斜杠的条目名称"
            }
        }
        if ($names -contains "CHANGELOG.md") {
            Fail "Zip 文件不能包含 CHANGELOG.md"
        }
        $licenseEntry = $zip.GetEntry("LICENSE")
        $licenseStream = $licenseEntry.Open()
        $licenseReader = New-Object -TypeName System.IO.StreamReader -ArgumentList @($licenseStream, [System.Text.Encoding]::UTF8)
        try {
            $licenseText = $licenseReader.ReadToEnd()
        } finally {
            $licenseReader.Dispose()
            $licenseStream.Dispose()
        }
        if ($licenseText -notmatch "GPL-3\.0-or-later") {
            Fail "LICENSE 未声明 GPL-3.0-or-later"
        }
        $copyingEntry = $zip.GetEntry("COPYING")
        $copyingStream = $copyingEntry.Open()
        $copyingReader = New-Object -TypeName System.IO.StreamReader -ArgumentList @($copyingStream, [System.Text.Encoding]::UTF8)
        try {
            $copyingText = $copyingReader.ReadToEnd()
        } finally {
            $copyingReader.Dispose()
            $copyingStream.Dispose()
        }
        if ($copyingText -notmatch "GNU GENERAL PUBLIC LICENSE") {
            Fail "COPYING 未包含 GPL 正文"
        }
        foreach ($diagnosticEntry in @("service.d/debug_collectors.sh", "service.d/media_state.sh", "service.d/app_status.sh", "service.d/diagnostic_archive.sh")) {
            if ($names -notcontains $diagnosticEntry) {
                Fail "Zip 缺失详细日志采集条目: $diagnosticEntry"
            }
        }

        foreach ($entryName in $names) {
            $isShellEntry = (
                $entryName -like "*.sh" -or
                $entryName -like "*.prop" -or
                $entryName -like "*.rule" -or
                $entryName -eq "bin/srxctl" -or
                $entryName -like "META-INF/*"
            )

            if (-not $isShellEntry) {
                continue
            }

            $entry = $zip.GetEntry($entryName)
            $entryStream = $entry.Open()
            $reader = New-Object -TypeName System.IO.StreamReader -ArgumentList @($entryStream, [System.Text.Encoding]::UTF8)
            try {
                $text = $reader.ReadToEnd()
            } finally {
                $reader.Dispose()
                $entryStream.Dispose()
            }

            if ($text.IndexOf([char]13) -ge 0) {
                Fail "Zip 条目包含 CR 换行符: $entryName"
            }
            if ($entryName -eq "module.prop") {
                $hasWebUi = $false
                foreach ($propLine in $text.Split([char]10)) {
                    if ($propLine.Trim() -eq "webui=1") {
                        $hasWebUi = $true
                        break
                    }
                }
                if (-not $hasWebUi) {
                    Fail "module.prop 缺失 webui=1"
                }
            }
        }
    } finally {
        $zip.Dispose()
    }
}

function Get-AdbDevices {
    param([string]$AdbPath)

    $output = & $AdbPath devices
    if ($LASTEXITCODE -ne 0) {
        return @()
    }

    $devices = @()
    foreach ($line in $output | Select-Object -Skip 1) {
        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }
        $parts = $line.Trim() -split "\s+"
        if ($parts.Count -ge 2) {
            $devices += [pscustomobject]@{
                Serial = $parts[0]
                State = $parts[1]
            }
        }
    }

    return $devices
}

function Confirm-YesNo {
    param([string]$Prompt)

    $answer = Read-Host "$Prompt [y/N]"
    return $answer -match '^(y|yes)$'
}

function Invoke-AdbChecked {
    param(
        [string]$AdbPath,
        [string]$Serial,
        [string[]]$Arguments
    )

    & $AdbPath -s $Serial @Arguments
    if ($LASTEXITCODE -ne 0) {
        Fail "adb 命令执行失败，退出代码 ${LASTEXITCODE}: adb -s $Serial $($Arguments -join ' ')"
    }
}

function Get-DeviceKsudPath {
    param(
        [string]$AdbPath,
        [string]$Serial
    )

    $probe = "if [ -x /data/adb/ksu/bin/ksud ]; then echo /data/adb/ksu/bin/ksud; elif command -v ksud >/dev/null 2>&1; then command -v ksud; else echo missing; fi"
    $output = & $AdbPath -s $Serial shell "su -c '$probe'"
    if ($LASTEXITCODE -ne 0) {
        Fail "无法在设备上运行 su。请先授予 adb shell root 权限。"
    }

    $path = ($output | Where-Object { $_ } | Select-Object -Last 1).Trim()
    if ($path -eq "missing" -or [string]::IsNullOrWhiteSpace($path)) {
        Fail "未在设备上找到 ksud。"
    }

    return $path
}

Push-Location $RepoRoot
try {
    Initialize-AndroidEnvironment

    $baseVersion = Get-CargoPackageVersion
    $resolved = Resolve-LocalVersion -BaseVersion $baseVersion
    if ([string]::IsNullOrWhiteSpace($Version)) {
        $Version = $resolved.Version
    }
    if ($VersionCode -le 0) {
        $VersionCode = $resolved.VersionCode
    }

    $outputRoot = [System.IO.Path]::GetFullPath((Join-Path $RepoRoot $OutputDir))
    New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null

    Write-Step "构建设置"
    Write-Host "版本:        v$Version"
    Write-Host "版本号:      $VersionCode"
    Write-Host "输出目录:    $outputRoot"

    if (-not $SkipBuild) {
        Write-Step "编译 Android 模块二进制文件"
        if ([string]::IsNullOrWhiteSpace($env:CXXFLAGS)) {
            $env:CXXFLAGS = "-Wno-error=unused-but-set-variable"
        }
        Remove-Item Env:\SRX_BUILD_DEBUG -ErrorAction SilentlyContinue
        Invoke-Checked -FilePath "cargo" -Arguments @("build", "--target", "aarch64-linux-android", "--release")
    } else {
        Write-Step "跳过构建，使用现有的目标输出"
    }

    $packageDir = Join-Path $outputRoot "local-module"
    $zipPath = Join-Path $outputRoot "storage.redirect.x-v$Version.zip"

    Write-Step "打包可刷入的模块 zip"
    Build-ModuleZip -PackageDir $packageDir -ZipPath $zipPath -ModuleVersion $Version -ModuleVersionCode $VersionCode

    Write-Step "验证模块 zip"
    Test-ModuleZip -ZipPath $zipPath
    if ($Version -match "^$([regex]::Escape($baseVersion))-ci\.(\d+)$") {
        Update-BuildCountBaseline -BaseVersion $baseVersion -BuildCount ([int]$Matches[1])
    }
    Write-Host "模块 zip 已就绪: $zipPath" -ForegroundColor Green

    if ($NoAdb) {
        Write-Host "已通过 -NoAdb 跳过 ADB 步骤。"
        exit 0
    }

    $adbCommand = Get-Command adb -ErrorAction SilentlyContinue
    if (-not $adbCommand) {
        Write-Host "在 PATH 中未找到 adb。跳过设备刷机提示。" -ForegroundColor Yellow
        exit 0
    }

    Write-Step "检查已连接的 ADB 设备"
    $devices = @(Get-AdbDevices -AdbPath $adbCommand.Source)
    $onlineDevices = @($devices | Where-Object { $_.State -eq "device" })
    if ($onlineDevices.Count -eq 0) {
        Write-Host "未找到在线的 adb 设备。仅完成了 Zip 构建。" -ForegroundColor Yellow
        if ($devices.Count -gt 0) {
            Write-Host "检测到非在线设备:"
            $devices | ForEach-Object { Write-Host "  $($_.Serial)  $($_.State)" }
        }
        exit 0
    }

    $serial = $onlineDevices[0].Serial
    if ($onlineDevices.Count -gt 1) {
        Write-Host "在线设备:"
        for ($i = 0; $i -lt $onlineDevices.Count; $i++) {
            Write-Host "  [$($i + 1)] $($onlineDevices[$i].Serial)"
        }
        $choice = Read-Host "选择设备编号，或按回车键默认选择 1"
        if ($choice -match '^\d+$') {
            $index = [int]$choice - 1
            if ($index -ge 0 -and $index -lt $onlineDevices.Count) {
                $serial = $onlineDevices[$index].Serial
            }
        }
    }

    if (-not (Confirm-YesNo "查找到已连接设备 $serial. 是否用ksud刷新此模块？")) {
        Write-Host "跳过刷入。Zip 文件保留在: $zipPath"
        exit 0
    }

    Write-Step "通过 ksud 刷入模块"
    $remoteZip = "/data/local/tmp/$([System.IO.Path]::GetFileName($zipPath))"
    Invoke-AdbChecked -AdbPath $adbCommand.Source -Serial $serial -Arguments @("push", $zipPath, $remoteZip)
    $ksudPath = Get-DeviceKsudPath -AdbPath $adbCommand.Source -Serial $serial
    Invoke-AdbChecked -AdbPath $adbCommand.Source -Serial $serial -Arguments @("shell", "su -c 'rm -rf /data/adb/modules_update/storage.redirect.x'")
    Invoke-AdbChecked -AdbPath $adbCommand.Source -Serial $serial -Arguments @("shell", "su -c '$ksudPath module install $remoteZip'")
    Write-Host "模块刷入命令已完成。" -ForegroundColor Green

    if (Confirm-YesNo "现在重启设备吗？") {
        Write-Step "正在重启设备"
        Invoke-AdbChecked -AdbPath $adbCommand.Source -Serial $serial -Arguments @("reboot")
    } else {
        Write-Host "已跳过重启。请稍后重启以使模块更新生效。" -ForegroundColor Yellow
    }
} finally {
    Pop-Location
}
