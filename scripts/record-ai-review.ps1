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
        throw "Git 命令执行失败：git $($Arguments -join ' ')"
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
        throw "AI 审核报告缺少属性：$Name"
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
        throw "AI 审核报告属性 $Name 必须是非空字符串数组。"
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
        throw "AI 审核报告中的 $Name 与暂存改动不匹配。"
    }
}

$resolvedReport = (Resolve-Path -LiteralPath $ReportPath).Path
$repoRoot = (Invoke-Git -Arguments @("rev-parse", "--show-toplevel")) | Select-Object -First 1
$tempRoot = [IO.Path]::GetFullPath((Join-Path $repoRoot "temp"))
$tempPrefix = $tempRoot.TrimEnd([IO.Path]::DirectorySeparatorChar, [IO.Path]::AltDirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
if (-not $resolvedReport.StartsWith($tempPrefix, [StringComparison]::OrdinalIgnoreCase)) {
    throw "AI 审核报告必须存放在 temp/ 下。"
}
$report = Get-Content -LiteralPath $resolvedReport -Raw -Encoding UTF8 | ConvertFrom-Json

if ([int](Get-RequiredProperty -Object $report -Name "schema") -ne 1) {
    throw "AI 审核报告 schema 必须为 1。"
}
$reviewer = [string](Get-RequiredProperty -Object $report -Name "reviewer")
if ($reviewer -notmatch '(?i)\b(Codex|Claude(?: Code)?|GitHub Copilot|Cursor|GPT(?:-[0-9.]+)?|Gemini|AI Agent)\b') {
    throw "reviewer 必须标识一个 AI Agent。"
}
$verdict = [string](Get-RequiredProperty -Object $report -Name "verdict")
if ($verdict -ne "pass") {
    throw "提交前 AI 审核 verdict 必须为 pass。"
}
$summary = ([string](Get-RequiredProperty -Object $report -Name "summary")).Trim()
if ($summary.Length -lt 12) {
    throw "AI 审核 summary 必须至少包含 12 个字符。"
}

$stagedFiles = @(
    @(Invoke-Git -Arguments @("diff", "--cached", "--name-only", "--diff-filter=ACMRD")) |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        ForEach-Object { ([string]$_).Replace("\", "/") }
)
if ($stagedFiles.Count -eq 0) {
    throw "没有可供 AI 审核的暂存改动。"
}

$tree = (Invoke-Git -Arguments @("write-tree")) | Select-Object -First 1
$baseCommit = (Invoke-Git -Arguments @("rev-parse", "HEAD")) | Select-Object -First 1
$reportedTree = ([string](Get-RequiredProperty -Object $report -Name "tree")).Trim()
$reportedBaseCommit = ([string](Get-RequiredProperty -Object $report -Name "baseCommit")).Trim()
$reportedFiles = Get-StringArray -Object $report -Name "files"
if ($reportedTree -ne $tree) {
    throw "AI 审核报告指向 tree $reportedTree，但当前暂存 tree 为 $tree。"
}
if ($reportedBaseCommit -ne $baseCommit) {
    throw "AI 审核报告指向基线 $reportedBaseCommit，但当前 HEAD 为 $baseCommit。"
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
        throw "AI 审核检查项 $checkName 必须为 pass 或 not_applicable。"
    }
    if ($result -eq "not_applicable" -and $checkName -notin $notApplicableChecks) {
        throw "AI 审核检查项 $checkName 必须得到 pass 结果。"
    }
    if ($evidence.Length -lt 12) {
        throw "AI 审核检查项 $checkName 需要具体证据。"
    }
}

$findingsProperty = $report.PSObject.Properties["findings"]
if ($null -ne $findingsProperty -and $null -ne $findingsProperty.Value) {
    foreach ($finding in @($findingsProperty.Value)) {
        $severity = ([string](Get-RequiredProperty -Object $finding -Name "severity")).Trim()
        $evidence = ([string](Get-RequiredProperty -Object $finding -Name "evidence")).Trim()
        $resolution = ([string](Get-RequiredProperty -Object $finding -Name "resolution")).Trim()
        if ($severity -notin @("blocker", "major", "minor") -or $evidence.Length -lt 12 -or $resolution.Length -lt 12) {
            throw "每项 AI 审核发现都需要 severity、evidence 和具体 resolution。"
        }
        $status = [string](Get-RequiredProperty -Object $finding -Name "status")
        if ($status -ne "resolved") {
            throw "每项 AI 审核发现都必须在提交前解决。"
        }
    }
}

$validation = @(Get-RequiredProperty -Object $report -Name "validation")
if ($validation.Count -eq 0) {
    throw "AI 审核报告必须至少列出一项验证命令或检查。"
}
foreach ($item in $validation) {
    $command = ([string](Get-RequiredProperty -Object $item -Name "command")).Trim()
    $result = ([string](Get-RequiredProperty -Object $item -Name "result")).Trim()
    $evidence = ([string](Get-RequiredProperty -Object $item -Name "evidence")).Trim()
    if ($command.Length -lt 3 -or $result -notin @("pass", "not_applicable") -or $evidence.Length -lt 8) {
        throw "每项验证都需要 command、pass/not_applicable 结果和具体 evidence。"
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

Write-Host "已记录 $reviewer 对 tree $tree 的 AI 审核。"
