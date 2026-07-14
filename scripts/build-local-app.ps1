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

function Write-Utf8LfFile {
    param(
        [string]$Path,
        [string]$Content
    )

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    $lf = $Content.Replace("`r`n", "`n").Replace("`r", "`n")
    [System.IO.File]::WriteAllText($Path, $lf, $utf8NoBom)
}

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
        Fail "Command failed with exit code ${LASTEXITCODE}: $FilePath $($Arguments -join ' ')"
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

function Get-LatestChildDirectory {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) {
        return $null
    }

    $dirs = @(Get-ChildItem -LiteralPath $Path -Directory | Sort-Object {
        try {
            [version]$_.Name
        } catch {
            [version]"0.0.0"
        }
    } -Descending)

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
}

function Get-CargoPackageVersion {
    $cargoToml = Join-Path $RepoRoot "Cargo.toml"
    $line = Get-Content -LiteralPath $cargoToml | Where-Object { $_ -match '^\s*version\s*=\s*"([^"]+)"' } | Select-Object -First 1
    if (-not $line -or $line -notmatch '^\s*version\s*=\s*"([^"]+)"') {
        Fail "Unable to read package version from Cargo.toml"
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
        Fail "git command failed: git $($Arguments -join ' ')"
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

    $fullWidthColon = [char]0xFF1A
    $updateManifest = -join @([char]0x66F4, [char]0x65B0, [char]0x66F4, [char]0x65B0, [char]0x6E05, [char]0x5355)
    $release = -join @([char]0x53D1, [char]0x5E03)
    return $subject.StartsWith("CI$fullWidthColon$updateManifest") -or $subject.StartsWith("$release$fullWidthColon$updateManifest")
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
        Fail "Build count must be positive, got: $BuildCount"
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
        Fail "Cargo.toml version must be MAJOR.MINOR.PATCH, got: $BaseVersion"
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
        Fail "Refusing to remove a path outside the expected parent: $pathFull"
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
        Fail "Missing release APK: $ApkPath"
    }

    $item = Get-Item -LiteralPath $ApkPath
    if ($item.Length -le 0) {
        Fail "Release APK is empty: $ApkPath"
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
        Fail "adb command failed with exit code ${LASTEXITCODE}: adb -s $Serial $($Arguments -join ' ')"
    }
}

function Restore-EnvVar {
    param(
        [string]$Name,
        [bool]$HadValue,
        [string]$OldValue
    )

    if ($HadValue) {
        Set-Item -Path "Env:\$Name" -Value $OldValue
    } else {
        Remove-Item -Path "Env:\$Name" -ErrorAction SilentlyContinue
    }
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

    $sourceApk = Join-Path $RepoRoot "app\build\outputs\apk\release\app-release.apk"
    $apkPath = Join-Path $outputRoot "storage.redirect.x-app-v$Version-release.apk"

    Write-Step "Build settings"
    Write-Host "Variant:      release"
    Write-Host "Version:      v$Version"
    Write-Host "Version code: $VersionCode"
    Write-Host "Output dir:   $outputRoot"

    if (-not $SkipBuild) {
        $gradlePath = Join-Path $RepoRoot "gradlew.bat"
        if (-not (Test-Path -LiteralPath $gradlePath)) {
            Fail "Missing Gradle wrapper: $gradlePath"
        }

        Write-Step "Build release APP"
        $hadVersion = Test-Path Env:\VERSION
        $oldVersion = $env:VERSION
        $hadVersionCode = Test-Path Env:\VERSION_CODE
        $oldVersionCode = $env:VERSION_CODE

        $env:VERSION = $Version
        $env:VERSION_CODE = [string]$VersionCode
        try {
            Invoke-Checked -FilePath $gradlePath -Arguments @("--no-daemon", "--console=plain", ":app:assembleRelease")
        } finally {
            Restore-EnvVar -Name "VERSION" -HadValue $hadVersion -OldValue $oldVersion
            Restore-EnvVar -Name "VERSION_CODE" -HadValue $hadVersionCode -OldValue $oldVersionCode
        }
    } else {
        Write-Step "Skip build and use existing release APK"
    }

    Write-Step "Copy APK output"
    Test-ReleaseApk -ApkPath $sourceApk
    Remove-LocalFile -Path $apkPath -ExpectedParent $outputRoot
    Copy-Item -LiteralPath $sourceApk -Destination $apkPath -Force
    Test-ReleaseApk -ApkPath $apkPath
    if ($Version -match "^$([regex]::Escape($baseVersion))-ci\.(\d+)$") {
        Update-BuildCountBaseline -BaseVersion $baseVersion -BuildCount ([int]$Matches[1])
    }
    Write-Host "Release APK ready: $apkPath" -ForegroundColor Green

    if ($NoAdb) {
        Write-Host "ADB step skipped by -NoAdb."
    } else {
        $adbCommand = Get-Command adb -ErrorAction SilentlyContinue
        if (-not $adbCommand) {
            Write-Host "adb was not found in PATH. APK build only." -ForegroundColor Yellow
        } else {
            Write-Step "Check connected ADB devices"
            $devices = @(Get-AdbDevices -AdbPath $adbCommand.Source)
            $onlineDevices = @($devices | Where-Object { $_.State -eq "device" })
            if ($onlineDevices.Count -eq 0) {
                Write-Host "No online adb device found. APK build only." -ForegroundColor Yellow
                if ($devices.Count -gt 0) {
                    Write-Host "Non-online devices:"
                    $devices | ForEach-Object { Write-Host "  $($_.Serial)  $($_.State)" }
                }
            } else {
                $serial = $onlineDevices[0].Serial
                if ($onlineDevices.Count -gt 1) {
                    Write-Host "Online devices:"
                    for ($i = 0; $i -lt $onlineDevices.Count; $i++) {
                        Write-Host "  [$($i + 1)] $($onlineDevices[$i].Serial)"
                    }
                    $choice = Read-Host "Select device number, or press Enter for 1"
                    if ($choice -match "^\d+$") {
                        $index = [int]$choice - 1
                        if ($index -ge 0 -and $index -lt $onlineDevices.Count) {
                            $serial = $onlineDevices[$index].Serial
                        }
                    }
                }

                if ((Confirm-YesNo "Connected device $serial found. Install this release APK?")) {
                    Write-Step "Install release APK"
                    Invoke-AdbChecked -AdbPath $adbCommand.Source -Serial $serial -Arguments @("install", "-r", $apkPath)
                    Write-Host "APK install completed." -ForegroundColor Green
                } else {
                    Write-Host "Install skipped. APK kept at: $apkPath"
                }
            }
        }
    }
} finally {
    Pop-Location
}
