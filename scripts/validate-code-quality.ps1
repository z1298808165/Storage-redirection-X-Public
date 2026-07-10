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
        throw "Git command failed: git $($Arguments -join ' ')"
    }
    return @($output)
}

if ($PSCmdlet.ParameterSetName -eq "Commit") {
    $scriptPath = (Invoke-Git -Arguments @(
        "ls-tree", "--name-only", $Commit, "--", "scripts/validate-code-quality.ps1"
    )) | Select-Object -First 1
    if ($scriptPath -ne "scripts/validate-code-quality.ps1") {
        Write-Host "Code quality check skipped for legacy commit $Commit."
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
        if ($candidate.Text -match "quality-allow\($([regex]::Escape($Rule))\):\s*.{10,}") {
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
    if (-not (Is-CodePath -Path $item.File) -or $item.File -eq "scripts/validate-code-quality.ps1") {
        continue
    }
    $text = $item.Text

    if ($text -match '(__DQ__|__PLACEHOLDER__|REPLACE_ME|TODO_IMPLEMENT|YOUR_[A-Z0-9_]+_HERE)') {
        Add-Violation -Index $index -Rule "placeholder" -Message "Temporary placeholder must not enter committed code."
    }
    if ($text -match '^\s*(<<<<<<<|=======|>>>>>>>)') {
        Add-Violation -Index $index -Rule "conflict-marker" -Message "Merge conflict marker found."
    }
    if ($text -match '\bdbg!\s*\(|\bdebugger\s*;') {
        Add-Violation -Index $index -Rule "debug-output" -Message "Debug-only statement must be removed or explicitly justified."
    }
    if ($text -match '\b(todo|unimplemented)!\s*\(' -or $text -match 'panic!\s*\(\s*"(?:TODO|FIXME|not implemented)') {
        Add-Violation -Index $index -Rule "incomplete-code" -Message "Incomplete implementation macro found."
    }
    if ($text -match '(TODO|FIXME|HACK)' -and $text -notmatch '(https?://|#[0-9]+|GH-[0-9]+)') {
        Add-Violation -Index $index -Rule "untracked-debt" -Message "Debt marker requires an issue or URL."
    }
    if ($item.File -match '^(?:app/src/main|java_src)/.*\.(?:kt|java)$' -and $text -match '@(?:org\.junit\.)?Test\b') {
        Add-Violation -Index $index -Rule "test-residue" -Message "Test annotation added under a production source directory."
    }
    if ($item.File -match '^(?:app/src/main|assets/zygisk_module/webroot)/.*\.(?:js|ts|tsx)$' -and
        $text -match '^\s*(?:describe|it|test)\s*\(') {
        Add-Violation -Index $index -Rule "test-residue" -Message "Inline test block added under a production source directory."
    }
    if ($text -match '#\[allow\((?:dead_code|unused[^)]*|clippy::[^)]*)\)\]' -or $text -match '@Suppress\("(?:UNUSED|unused)') {
        Add-Violation -Index $index -Rule "lint-suppression" -Message "New lint suppression requires a concrete nearby justification."
    }
    if ($item.File -match '^src/.*\.rs$' -and $text -match '\.(unwrap|expect)\s*\(') {
        Add-Violation -Index $index -Rule "error-handling" -Message "Production Rust code must handle failure or document the invariant."
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
            Add-Violation -Index $index -Rule "unsafe-contract" -Message "New unsafe block requires a nearby SAFETY comment."
        }
    }
}

$codeAdditions = @($additions | Where-Object { Is-CodePath -Path $_.File })
$touchedCodeFiles = @($codeAdditions | ForEach-Object { $_.File } | Sort-Object -Unique)
if ($codeAdditions.Count -gt 800 -or $touchedCodeFiles.Count -gt 12) {
    Write-Warning "Large code change detected: $($codeAdditions.Count) added lines across $($touchedCodeFiles.Count) files. AI review must justify scope and split decisions."
}

if ($violations.Count -gt 0) {
    throw ($violations -join "`n")
}

Write-Host "Incremental code quality check passed: $($codeAdditions.Count) added code lines across $($touchedCodeFiles.Count) files."
