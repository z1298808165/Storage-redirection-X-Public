[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ReportPath
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$requiredChecks = @(
    "scope_and_necessity",
    "dead_code_and_test_residue",
    "duplication_and_reuse",
    "abstraction_and_complexity",
    "error_and_resource_paths",
    "concurrency_and_global_state",
    "compatibility_and_hook_boundaries",
    "tests_and_verification",
    "readability_and_maintenance"
)

function Invoke-Git {
    param([string[]]$Arguments)

    $output = & git @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Git command failed: git $($Arguments -join ' ')"
    }
    return @($output)
}

function Get-RequiredProperty {
    param(
        [object]$Object,
        [string]$Name
    )

    $property = $Object.PSObject.Properties[$Name]
    if ($null -eq $property) {
        throw "AI review report is missing property: $Name"
    }
    return $property.Value
}

function Get-StringArray {
    param(
        [object]$Object,
        [string]$Name
    )

    $values = @(Get-RequiredProperty -Object $Object -Name $Name)
    if ($values.Count -eq 0 -or @($values | Where-Object { [string]::IsNullOrWhiteSpace([string]$_) }).Count -gt 0) {
        throw "AI review report property $Name must be a non-empty string array."
    }
    return @($values | ForEach-Object { ([string]$_).Replace("\", "/").Trim() })
}

function Assert-EqualStringArrays {
    param(
        [string[]]$Expected,
        [string[]]$Actual,
        [string]$Name
    )

    $expectedValues = @($Expected | Sort-Object -Unique)
    $actualValues = @($Actual | Sort-Object -Unique)
    $differences = @(Compare-Object -ReferenceObject $expectedValues -DifferenceObject $actualValues)
    if ($expectedValues.Count -ne $actualValues.Count -or
        $differences.Count -gt 0) {
        throw "AI review report $Name does not match the staged change."
    }
}

$resolvedReport = (Resolve-Path -LiteralPath $ReportPath).Path
$repoRoot = (Invoke-Git -Arguments @("rev-parse", "--show-toplevel")) | Select-Object -First 1
$tempRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot "temp"))
$tempPrefix = $tempRoot.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (-not $resolvedReport.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "AI review reports must be stored under temp/."
}
$report = Get-Content -LiteralPath $resolvedReport -Raw -Encoding UTF8 | ConvertFrom-Json

if ([int](Get-RequiredProperty -Object $report -Name "schema") -ne 1) {
    throw "AI review report schema must be 1."
}
$reviewer = [string](Get-RequiredProperty -Object $report -Name "reviewer")
if ($reviewer -notmatch '(?i)\b(Codex|Claude(?: Code)?|GitHub Copilot|Cursor|GPT(?:-[0-9.]+)?|Gemini|AI Agent)\b') {
    throw "Reviewer must identify an AI agent."
}
$verdict = [string](Get-RequiredProperty -Object $report -Name "verdict")
if ($verdict -ne "pass") {
    throw "AI review verdict must be pass before commit."
}
$summary = ([string](Get-RequiredProperty -Object $report -Name "summary")).Trim()
if ($summary.Length -lt 12) {
    throw "AI review summary must contain at least 12 characters."
}

$stagedFiles = @(
    @(Invoke-Git -Arguments @("diff", "--cached", "--name-only", "--diff-filter=ACMRD")) |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        ForEach-Object { ([string]$_).Replace("\", "/") }
)
if ($stagedFiles.Count -eq 0) {
    throw "No staged changes are available for AI review."
}

$tree = (Invoke-Git -Arguments @("write-tree")) | Select-Object -First 1
$baseCommit = (Invoke-Git -Arguments @("rev-parse", "HEAD")) | Select-Object -First 1
$reportedTree = ([string](Get-RequiredProperty -Object $report -Name "tree")).Trim()
$reportedBaseCommit = ([string](Get-RequiredProperty -Object $report -Name "baseCommit")).Trim()
$reportedFiles = Get-StringArray -Object $report -Name "files"
if ($reportedTree -ne $tree) {
    throw "AI review report targets tree $reportedTree, but the staged tree is $tree."
}
if ($reportedBaseCommit -ne $baseCommit) {
    throw "AI review report targets base $reportedBaseCommit, but HEAD is $baseCommit."
}
Assert-EqualStringArrays -Expected $stagedFiles -Actual $reportedFiles -Name "files"

$checks = Get-RequiredProperty -Object $report -Name "checks"
$notApplicableChecks = @(
    "error_and_resource_paths",
    "concurrency_and_global_state",
    "compatibility_and_hook_boundaries"
)
foreach ($checkName in $requiredChecks) {
    $check = Get-RequiredProperty -Object $checks -Name $checkName
    $result = [string](Get-RequiredProperty -Object $check -Name "result")
    $evidence = ([string](Get-RequiredProperty -Object $check -Name "evidence")).Trim()
    if ($result -notin @("pass", "not_applicable")) {
        throw "AI review check $checkName must be pass or not_applicable."
    }
    if ($result -eq "not_applicable" -and $checkName -notin $notApplicableChecks) {
        throw "AI review check $checkName requires a pass result."
    }
    if ($evidence.Length -lt 12) {
        throw "AI review check $checkName needs concrete evidence."
    }
}

$findingsProperty = $report.PSObject.Properties["findings"]
if ($null -ne $findingsProperty -and $null -ne $findingsProperty.Value) {
    foreach ($finding in @($findingsProperty.Value)) {
        $severity = ([string](Get-RequiredProperty -Object $finding -Name "severity")).Trim()
        $evidence = ([string](Get-RequiredProperty -Object $finding -Name "evidence")).Trim()
        $resolution = ([string](Get-RequiredProperty -Object $finding -Name "resolution")).Trim()
        if ($severity -notin @("blocker", "major", "minor") -or $evidence.Length -lt 12 -or $resolution.Length -lt 12) {
            throw "Every AI review finding needs severity, evidence, and a concrete resolution."
        }
        $status = [string](Get-RequiredProperty -Object $finding -Name "status")
        if ($status -ne "resolved") {
            throw "Every AI review finding must be resolved before commit."
        }
    }
}

$validation = @(Get-RequiredProperty -Object $report -Name "validation")
if ($validation.Count -eq 0) {
    throw "AI review report must list at least one validation command or inspection."
}
foreach ($item in $validation) {
    $command = ([string](Get-RequiredProperty -Object $item -Name "command")).Trim()
    $result = ([string](Get-RequiredProperty -Object $item -Name "result")).Trim()
    $evidence = ([string](Get-RequiredProperty -Object $item -Name "evidence")).Trim()
    if ($command.Length -lt 3 -or $result -notin @("pass", "not_applicable") -or $evidence.Length -lt 8) {
        throw "Each validation item needs a command, pass/not_applicable result, and concrete evidence."
    }
}

$sha = [System.Security.Cryptography.SHA256]::Create()
try {
    $reportHash = ([BitConverter]::ToString($sha.ComputeHash([IO.File]::ReadAllBytes($resolvedReport)))).Replace("-", "").ToLowerInvariant()
}
finally {
    $sha.Dispose()
}

$receipt = [ordered]@{
    schema = 1
    tree = $tree
    baseCommit = $baseCommit
    reviewer = $reviewer.Trim()
    summary = ($summary -replace '[\r\n]+', ' ')
    reportHash = $reportHash
    reportPath = $resolvedReport
    reviewedAtUtc = [DateTime]::UtcNow.ToString("o")
    files = @($stagedFiles | Sort-Object -Unique)
}

$gitPath = (Invoke-Git -Arguments @("rev-parse", "--git-path", "srx-ai-review.json")) | Select-Object -First 1
if (-not [IO.Path]::IsPathRooted($gitPath)) {
    $gitPath = Join-Path $repoRoot $gitPath
}
$parent = Split-Path -Parent $gitPath
New-Item -ItemType Directory -Force -Path $parent | Out-Null
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[IO.File]::WriteAllText($gitPath, ($receipt | ConvertTo-Json -Depth 5) + "`n", $utf8NoBom)

Write-Host "AI review recorded for tree $tree by $reviewer."
