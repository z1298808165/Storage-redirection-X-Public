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

    $displayCommand = "$FilePath $($Arguments -join ' ')".Trim()
    $startedAt = Get-Date
    Write-Host "开始执行：$displayCommand"
    & $FilePath @Arguments
    $exitCode = $LASTEXITCODE
    $elapsed = (Get-Date) - $startedAt
    Write-Host ("命令完成：耗时 {0:N1} 秒，退出代码 {1}" -f $elapsed.TotalSeconds, $exitCode)
    if ($exitCode -ne 0) {
        Fail "命令执行失败，退出代码 ${exitCode}：$displayCommand"
    }
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

function Initialize-AdbEnvironment {
    if (Get-Command adb -ErrorAction SilentlyContinue) { return }
    $sdk = Get-FirstExistingPath @(
        $env:ANDROID_HOME,
        $env:ANDROID_SDK_ROOT,
        (Join-Path $env:LOCALAPPDATA "Android\Sdk")
    )

    if ($sdk) { Add-PathPrefix (Join-Path $sdk "platform-tools") }
}

function Invoke-WslGradle {
    param([string]$Version, [int]$VersionCode)

    $wsl = Get-Command wsl.exe -ErrorAction Stop
    Invoke-Checked -FilePath $wsl.Source -Arguments @(
        "--cd", $RepoRoot, "--exec", "bash", "-lc",
        'export VERSION="$1" VERSION_CODE="$2"; shift 2; exec ./gradlew "$@"',
        "_", $Version, [string]$VersionCode, "--console=plain", ":app:assembleRelease"
    )
}

# App 版本走日期时间线（yyyyMMddHHmm），与模块的 semver（2.3.2）分处两个命名空间，
# 用户一眼就能分出哪个是 App、哪个是模块。versionCode 用「自 2020-01-01 起的分钟数」，
# 既单调递增又落在 int32 内（Android 的 versionCode 上限约 21 亿）。
function Get-AppTimestampVersion {
    return (Get-Date).ToString("yyyyMMddHHmm")
}

function Get-AppTimestampVersionCode {
    param([string]$VersionName)
    if ($VersionName -notmatch '^\d{12}(\d{2})?$') {
        Fail "App 版本号必须是 yyyyMMddHHmm 形式的日期时间，当前值：$VersionName"
    }
    $text = $VersionName.Substring(0, 12)
    $builtAt = [datetime]::ParseExact($text, "yyyyMMddHHmm", $null)
    return [int][math]::Floor(($builtAt - [datetime]::new(2020, 1, 1, 0, 0, 0)).TotalMinutes)
}

function Assert-UnderPath {
    param(
        [string]$Path,
        [string]$Parent
    )

    $parentFull = [System.IO.Path]::GetFullPath($Parent).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    $pathFull = [System.IO.Path]::GetFullPath($Path).TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)

    if (-not $pathFull.StartsWith($parentFull + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        Fail "拒绝删除预期父目录以外的路径：$pathFull"
    }
}

function Remove-LocalFile {
    param(
        [string]$Path,
        [string]$ExpectedParent
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        return
    }

    Assert-UnderPath -Path $Path -Parent $ExpectedParent
    Remove-Item -LiteralPath $Path -Force
}

function Test-ReleaseApk {
    param([string]$ApkPath)

    if (-not (Test-Path -LiteralPath $ApkPath)) {
        if (Test-Path -LiteralPath ($ApkPath -replace '\.apk$', '-unsigned.apk')) {
            Fail "Gradle 只生成了未签名 release APK；请先在 WSL 配置 release 签名，见 docs/build-process.md。"
        }
        Fail "缺少 release APK：$ApkPath"
    }

    $item = Get-Item -LiteralPath $ApkPath
    if ($item.Length -le 0) {
        Fail "Release APK 为空：$ApkPath"
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
    return $answer -match "^(y|yes)$"
}

function Invoke-AdbChecked {
    param(
        [string]$AdbPath,
        [string]$Serial,
        [string[]]$Arguments
    )

    & $AdbPath -s $Serial @Arguments
    if ($LASTEXITCODE -ne 0) {
        Fail "adb 命令执行失败，退出码 ${LASTEXITCODE}：adb -s $Serial $($Arguments -join ' ')"
    }
}

Push-Location $RepoRoot
try {
    if (-not $NoAdb) { Initialize-AdbEnvironment }

    if ([string]::IsNullOrWhiteSpace($Version)) {
        $Version = Get-AppTimestampVersion
    }
    if ($VersionCode -le 0) {
        $VersionCode = Get-AppTimestampVersionCode -VersionName $Version
    }

    $outputRoot = [System.IO.Path]::GetFullPath((Join-Path $RepoRoot $OutputDir))
    New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null

    $sourceApk = Join-Path $RepoRoot "app\build\outputs\apk\release\app-release.apk"
    $apkPath = Join-Path $outputRoot "storage.redirect.x-app-v$Version-release.apk"

    Write-Step "构建设置"
    Write-Host "构建变体：    release"
    Write-Host "版本：        v$Version"
    Write-Host "版本代码：    $VersionCode"
    Write-Host "输出目录：    $outputRoot"

    if (-not $SkipBuild) {
        Write-Step "在 WSL 中构建 release 应用"
        Invoke-WslGradle -Version $Version -VersionCode $VersionCode
    } else {
        Write-Step "跳过构建并使用现有 release APK"
    }

    Write-Step "复制 APK 产物"
    Test-ReleaseApk -ApkPath $sourceApk
    Remove-LocalFile -Path $apkPath -ExpectedParent $outputRoot
    Copy-Item -LiteralPath $sourceApk -Destination $apkPath -Force
    Test-ReleaseApk -ApkPath $apkPath
    Write-Host "Release APK 已就绪：$apkPath" -ForegroundColor Green

    if ($NoAdb) {
        Write-Host "已通过 -NoAdb 跳过 ADB 步骤。"
    } else {
        $adbCommand = Get-Command adb -ErrorAction SilentlyContinue
        if (-not $adbCommand) {
            Write-Host "PATH 中未找到 adb，仅完成 APK 构建。" -ForegroundColor Yellow
        } else {
            Write-Step "检查已连接的 ADB 设备"
            $devices = @(Get-AdbDevices -AdbPath $adbCommand.Source)
            $onlineDevices = @($devices | Where-Object { $_.State -eq "device" })
            if ($onlineDevices.Count -eq 0) {
                Write-Host "未找到在线 adb 设备，仅完成 APK 构建。" -ForegroundColor Yellow
                if ($devices.Count -gt 0) {
                    Write-Host "非在线设备："
                    $devices | ForEach-Object { Write-Host "  $($_.Serial)  $($_.State)" }
                }
            } else {
                $serial = $onlineDevices[0].Serial
                if ($onlineDevices.Count -gt 1) {
                    Write-Host "在线设备："
                    for ($i = 0; $i -lt $onlineDevices.Count; $i++) {
                        Write-Host "  [$($i + 1)] $($onlineDevices[$i].Serial)"
                    }
                    $choice = Read-Host "请选择设备编号，或按 Enter 使用第 1 个设备"
                    if ($choice -match "^\d+$") {
                        $index = [int]$choice - 1
                        if ($index -ge 0 -and $index -lt $onlineDevices.Count) {
                            $serial = $onlineDevices[$index].Serial
                        }
                    }
                }

                if ((Confirm-YesNo "已发现连接设备 $serial，是否安装此 release APK？")) {
                    Write-Step "安装 release APK"
                    Invoke-AdbChecked -AdbPath $adbCommand.Source -Serial $serial -Arguments @("install", "-r", $apkPath)
                    Write-Host "APK 安装完成。" -ForegroundColor Green
                } else {
                    Write-Host "已跳过安装，APK 保留于：$apkPath"
                }
            }
        }
    }
} finally {
    Pop-Location
}
