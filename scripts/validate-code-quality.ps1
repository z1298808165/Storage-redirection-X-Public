[CmdletBinding(DefaultParameterSetName = "Staged")]
param(
    [Parameter(Mandatory = $true, ParameterSetName = "Staged")]
    [switch]$Staged,

    [Parameter(Mandatory = $true, ParameterSetName = "Commit")]
    [string]$Commit,

    [Parameter(Mandatory = $true, ParameterSetName = "Range")]
    [string]$BaseRef,

    [Parameter(Mandatory = $true, ParameterSetName = "Range")]
    [string]$Ref
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-Git {
    param([string[]]$Arguments)

    $output = & git @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Git 命令执行失败：git $($Arguments -join ' ')"
    }
    return @($output)
}

if ($PSCmdlet.ParameterSetName -eq "Commit") {
    $scriptPath = (Invoke-Git -Arguments @(
        "ls-tree", "--name-only", $Commit, "--", "scripts/validate-code-quality.ps1"
    )) | Select-Object -First 1
    if ($scriptPath -ne "scripts/validate-code-quality.ps1") {
        Write-Host "旧提交 $Commit 不包含质量检查脚本，已跳过代码质量检查。"
        exit 0
    }
}

$diffArguments = @("--no-ext-diff", "--no-color", "--unified=0", "--diff-filter=ACMR")
switch ($PSCmdlet.ParameterSetName) {
    "Staged" { $lines = Invoke-Git -Arguments (@("diff", "--cached") + $diffArguments) }
    "Commit" { $lines = Invoke-Git -Arguments (@("show", "--format=") + $diffArguments + @($Commit)) }
    "Range" { $lines = Invoke-Git -Arguments (@("diff") + $diffArguments + @($BaseRef, $Ref)) }
}

$additions = New-Object System.Collections.Generic.List[object]
$currentFile = ""
$newLine = 0
foreach ($line in $lines) {
    if ($line -like "+++ b/*") {
        $currentFile = $line.Substring(6).Replace("\", "/")
        continue
    }
    if ($line -match '^@@\s+-\d+(?:,\d+)?\s+\+(\d+)') {
        $newLine = [int]$matches[1]
        continue
    }
    if ($line.StartsWith("+") -and -not $line.StartsWith("+++")) {
        $additions.Add([pscustomobject]@{
            File = $currentFile
            Line = $newLine
            Text = $line.Substring(1)
        })
        $newLine++
    }
    elseif (-not $line.StartsWith("-") -and -not $line.StartsWith("diff ") -and -not $line.StartsWith("index ")) {
        $newLine++
    }
}

function Is-CodePath {
    param([string]$Path)

    return $Path -match '^(src/|app/src/main/|java_src/|tools/|assets/zygisk_module/webroot/|scripts/|\.github/scripts/|\.github/tests/)' -or
        $Path -eq "build.rs"
}

function Is-ChineseLanguagePath {
    param([string]$Path)

    if ($Path -match '^(vendor/|\.github/vendor/)' -or
        $Path -match '(?i)(^|/)(?:LICENSE|COPYING|NOTICE)(?:\.|$)' -or
        $Path -match '(?i)\.patch$') {
        return $false
    }

    return $Path -match '^(src/|app/|java_src/|native/|tools/|assets/zygisk_module/|scripts/|\.githooks/|\.github/(?:scripts|tests|workflows)/|tests/storage-redirect-test/|docs/)' -or
        $Path -match '^(?:AGENTS|CLAUDE|CONTRIBUTING|README)\.md$' -or
        $Path -eq ".github/pull_request_template.md" -or
        $Path -eq "build.rs" -or
        $Path -match '\.(?:gradle|gradle\.kts)$'
}

function Get-HumanReadableText {
    param(
        [string]$Path,
        [string]$Text
    )

    $trimmed = $Text.Trim()
    if (-not $trimmed -or $trimmed -match '^(?:#!|```|~~~)') {
        return ""
    }

    if ($Path -match '(?i)\.md$') {
        $candidate = $trimmed -replace '^#{1,6}\s*', ''
        $candidate = $candidate -replace '^(?:[-*+]\s+|\d+[.)]\s+|>\s*)', ''
        if ($candidate -match '^\|.*\|$' -or
            $candidate -match '^[-:| ]+$' -or
            $candidate -match '^(?i:Storage Redirect X)(?:\s+v?[0-9.]+)?$' -or
            $candidate -match '^(?i:cargo|git|node|npm|pnpm|yarn|gradle|adb|powershell|bash|sh|rustup)\s+' -or
            $candidate -match '^(?:[A-Za-z]:[\\/]|[./~][\\/])') {
            return ""
        }
        return $candidate
    }

    if ($trimmed -match '^(?://[/!]?|/\*+|\*|<!--[ ]?)(.*?)(?:\*/|-->)?$') {
        return $matches[1].Trim()
    }

    if (($Path -match '(?i)\.(?:ps1|psm1|psd1|sh|bash|py|yml|yaml|properties)$' -or
        $Path -match '^\.githooks/') -and
        $trimmed -match '^#(?!\!)(.*)$') {
        return $matches[1].Trim()
    }

    if ($Path -match '(?i)\.(?:bat|cmd)$' -and $trimmed -match '^(?i:REM)\s+(.*)$') {
        return $matches[1].Trim()
    }

    return ""
}

function Is-EnglishNaturalLanguage {
    param([string]$Text)

    if (-not $Text -or $Text -match '[\p{IsCJKUnifiedIdeographs}]') {
        return $false
    }
    if ($Text -match '(?i)^(?:SPDX-License-Identifier|quality-allow\(|https?://)' -or
        $Text -match '^[A-Z0-9_.:/<>|=+*`-]+$') {
        return $false
    }

    $withoutTechnicalContent = $Text -replace '`[^`]+`', ''
    $withoutTechnicalContent = $withoutTechnicalContent -replace 'https?://\S+', ''
    return $withoutTechnicalContent -match '(?i)\b[a-z]{2,}\b(?:[^A-Za-z\r\n]+|\s+)\b[a-z]{2,}\b'
}

function Has-Allowance {
    param(
        [int]$Index,
        [string]$Rule
    )

    $item = $additions[$Index]
    $start = [Math]::Max(0, $Index - 2)
    for ($cursor = $start; $cursor -le $Index; $cursor++) {
        $candidate = $additions[$cursor]
        if ($candidate.File -ne $item.File) {
            continue
        }
        if ($candidate.Text -match "quality-allow\($([regex]::Escape($Rule))\):\s*(?=.*[\p{IsCJKUnifiedIdeographs}]).{10,}") {
            return $true
        }
    }
    return $false
}

$violations = New-Object System.Collections.Generic.List[string]
function Add-Violation {
    param(
        [int]$Index,
        [string]$Rule,
        [string]$Message
    )

    if (-not (Has-Allowance -Index $Index -Rule $Rule)) {
        $item = $additions[$Index]
        $violations.Add("$($item.File):$($item.Line): [$Rule] $Message")
    }
}

for ($index = 0; $index -lt $additions.Count; $index++) {
    $item = $additions[$index]
    $isCodePath = Is-CodePath -Path $item.File
    $isChineseLanguagePath = Is-ChineseLanguagePath -Path $item.File
    if (-not $isCodePath -and -not $isChineseLanguagePath -and $item.File -ne "scripts/validate-code-quality.ps1") {
        continue
    }
    $text = $item.Text

    if ($isChineseLanguagePath) {
        $humanReadableText = Get-HumanReadableText -Path $item.File -Text $text
        if (Is-EnglishNaturalLanguage -Text $humanReadableText) {
            Add-Violation -Index $index -Rule "chinese-language" -Message "项目自维护的人类可读内容必须使用中文；必要英文需添加带中文理由的紧邻豁免。"
        }
    }

    if (-not $isCodePath -and $item.File -ne "scripts/validate-code-quality.ps1") {
        continue
    }

    if ($text -match '(__DQ__|__PLACEHOLDER__|REPLACE_ME|TODO_IMPLEMENT|YOUR_[A-Z0-9_]+_HERE)') {
        Add-Violation -Index $index -Rule "placeholder" -Message "临时占位符不得进入已提交代码。"
    }
    if ($text -match '^\s*(<<<<<<<|=======|>>>>>>>)') {
        Add-Violation -Index $index -Rule "conflict-marker" -Message "发现合并冲突标记。"
    }
    if ($text -match '\bdbg!\s*\(|\bdebugger\s*;') {
        Add-Violation -Index $index -Rule "debug-output" -Message "必须移除仅用于调试的语句，或明确说明保留理由。"
    }
    if ($text -match '\b(todo|unimplemented)!\s*\(' -or $text -match 'panic!\s*\(\s*"(?:TODO|FIXME|not implemented)') {
        Add-Violation -Index $index -Rule "incomplete-code" -Message "发现未完成实现宏。"
    }
    if ($text -match '(TODO|FIXME|HACK)' -and $text -notmatch '(https?://|#[0-9]+|GH-[0-9]+)') {
        Add-Violation -Index $index -Rule "untracked-debt" -Message "技术债标记必须关联 Issue 或 URL。"
    }
    if ($item.File -match '^(?:app/src/main|java_src)/.*\.(?:kt|java)$' -and $text -match '@(?:org\.junit\.)?Test\b') {
        Add-Violation -Index $index -Rule "test-residue" -Message "生产源码目录中不得新增测试注解。"
    }
    if ($item.File -match '^(?:app/src/main|assets/zygisk_module/webroot)/.*\.(?:js|ts|tsx)$' -and
        $text -match '^\s*(?:describe|it|test)\s*\(') {
        Add-Violation -Index $index -Rule "test-residue" -Message "生产源码目录中不得新增内联测试块。"
    }
    if ($text -match '#\[allow\((?:dead_code|unused[^)]*|clippy::[^)]*)\)\]' -or $text -match '@Suppress\("(?:UNUSED|unused)') {
        Add-Violation -Index $index -Rule "lint-suppression" -Message "新增 lint 抑制必须在附近给出具体理由。"
    }
    if ($item.File -match '^src/.*\.rs$' -and $text -match '\.(unwrap|expect)\s*\(') {
        Add-Violation -Index $index -Rule "error-handling" -Message "生产 Rust 代码必须处理失败或说明不变量。"
    }
    if ($item.File -match '^src/.*\.rs$' -and $text -match '\bunsafe\s*\{') {
        $hasSafety = $false
        $start = [Math]::Max(0, $index - 3)
        for ($cursor = $start; $cursor -le $index; $cursor++) {
            if ($additions[$cursor].File -eq $item.File -and $additions[$cursor].Text -match 'SAFETY:\s*.{10,}') {
                $hasSafety = $true
            }
        }
        if (-not $hasSafety) {
            Add-Violation -Index $index -Rule "unsafe-contract" -Message "新增 unsafe 块必须在附近提供 SAFETY 注释。"
        }
    }
}

$codeAdditions = @($additions | Where-Object { Is-CodePath -Path $_.File })
$touchedCodeFiles = @($codeAdditions | ForEach-Object { $_.File } | Sort-Object -Unique)
if ($codeAdditions.Count -gt 800 -or $touchedCodeFiles.Count -gt 12) {
    Write-Warning "检测到大型代码改动：$($codeAdditions.Count) 行新增代码，涉及 $($touchedCodeFiles.Count) 个文件。AI 审核必须说明范围和拆分决策。"
}

if ($violations.Count -gt 0) {
    throw ($violations -join "`n")
}

Write-Host "增量代码质量检查通过：$($codeAdditions.Count) 行新增代码，涉及 $($touchedCodeFiles.Count) 个文件。"
