[CmdletBinding(DefaultParameterSetName = "Staged")]
param(
    [Parameter(Mandatory = $true, ParameterSetName = "Staged")]
    [switch]$Staged,

    [Parameter(Mandatory = $true, ParameterSetName = "Message")]
    [string]$MessageFile,

    [Parameter(Mandatory = $true, ParameterSetName = "Commit")]
    [string]$Commit
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-Git {
    param(
        [string[]]$Arguments,
        [switch]$AllowFailure
    )

    $output = & git @Arguments
    if ($LASTEXITCODE -ne 0 -and -not $AllowFailure) {
        throw "Git 命令执行失败：git $($Arguments -join ' ')"
    }
    return @($output)
}

function Get-Receipt {
    $gitPath = (Invoke-Git -Arguments @("rev-parse", "--git-path", "srx-ai-review.json")) | Select-Object -First 1
    if (-not [IO.Path]::IsPathRooted($gitPath)) {
        $repoRoot = (Invoke-Git -Arguments @("rev-parse", "--show-toplevel")) | Select-Object -First 1
        $gitPath = Join-Path $repoRoot $gitPath
    }
    if (-not (Test-Path -LiteralPath $gitPath)) {
        throw "缺少 AI 审核凭据。必须由 AI Agent 审核暂存差异并运行 scripts/record-ai-review.ps1。"
    }
    $receipt = Get-Content -LiteralPath $gitPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if ([int]$receipt.schema -ne 1) {
        throw "AI 审核凭据使用了不支持的 schema。"
    }
    return $receipt
}

function Get-FileHash256 {
    param([string]$Path)

    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([BitConverter]::ToString($sha.ComputeHash([IO.File]::ReadAllBytes($Path)))).Replace("-", "").ToLowerInvariant()
    }
    finally {
        $sha.Dispose()
    }
}

function Assert-StagedReceipt {
    $receipt = Get-Receipt
    $tree = (Invoke-Git -Arguments @("write-tree")) | Select-Object -First 1
    $baseCommit = (Invoke-Git -Arguments @("rev-parse", "HEAD")) | Select-Object -First 1
    if ([string]$receipt.tree -ne $tree) {
        throw "AI 审核已过期：审核后暂存 tree 发生变化。预期 $($receipt.tree)，实际 $tree。"
    }
    if ([string]$receipt.baseCommit -ne $baseCommit) {
        throw "AI 审核已过期：审核后 HEAD 发生变化。"
    }
    if ([string]$receipt.reportHash -notmatch '^[0-9a-f]{64}$') {
        throw "AI 审核凭据包含无效的报告哈希。"
    }
    $reportPath = [string]$receipt.reportPath
    if ([string]::IsNullOrWhiteSpace($reportPath) -or -not (Test-Path -LiteralPath $reportPath)) {
        throw "AI 审核报告不存在。提交完成前必须将已审核报告保留在 temp/ 下。"
    }
    $actualHash = Get-FileHash256 -Path $reportPath
    if ($actualHash -ne [string]$receipt.reportHash) {
        throw "记录凭据后 AI 审核报告发生变化。"
    }
    $receiptFiles = @($receipt.files | ForEach-Object { ([string]$_).Replace("\", "/") } | Sort-Object -Unique)
    $stagedFiles = @(
        @(Invoke-Git -Arguments @("diff", "--cached", "--name-only", "--diff-filter=ACMRD")) |
            ForEach-Object { ([string]$_).Replace("\", "/") } |
            Sort-Object -Unique
    )
    $differences = @(Compare-Object -ReferenceObject $receiptFiles -DifferenceObject $stagedFiles)
    if ($receiptFiles.Count -ne $stagedFiles.Count -or
        $differences.Count -gt 0) {
        throw "AI 审核凭据中的文件清单与暂存改动不匹配。"
    }
    return $receipt
}

function Get-SingleTrailer {
    param(
        [string]$Message,
        [string]$Name
    )

    $matches = [regex]::Matches($Message, "(?m)^$([regex]::Escape($Name)):\s*(.+?)\s*$")
    if ($matches.Count -ne 1) {
        throw "Commit 必须且只能包含一个 $Name trailer。"
    }
    return $matches[0].Groups[1].Value.Trim()
}

function Test-HasStagedChanges {
    & git diff --cached --quiet --exit-code
    return $LASTEXITCODE -ne 0
}

function Test-IsEmptyCommit {
    param([string]$Commitish)

    $parents = @(Invoke-Git -Arguments @("show", "-s", "--format=%P", $Commitish))
    $parentLine = ($parents -join " ").Trim()
    if ([string]::IsNullOrWhiteSpace($parentLine)) {
        return $false
    }
    $firstParent = ($parentLine -split '\s+')[0]
    & git diff --quiet --exit-code $firstParent $Commitish
    return $LASTEXITCODE -eq 0
}

if ($PSCmdlet.ParameterSetName -eq "Commit") {
    $scriptPath = (Invoke-Git -Arguments @(
        "ls-tree", "--name-only", $Commit, "--", "scripts/validate-ai-review.ps1"
    )) | Select-Object -First 1
    if ($scriptPath -ne "scripts/validate-ai-review.ps1") {
        Write-Host "旧提交 $Commit 不包含 AI 审核门禁，已跳过检查。"
        exit 0
    }

    if (Test-IsEmptyCommit -Commitish $Commit) {
        Write-Host "空提交 $Commit 已跳过 AI 审核检查。"
        exit 0
    }

    $tree = (Invoke-Git -Arguments @("show", "-s", "--format=%T", $Commit)) | Select-Object -First 1
    $message = (Invoke-Git -Arguments @("show", "-s", "--format=%B", $Commit)) -join "`n"
    $reviewTree = Get-SingleTrailer -Message $message -Name "AI-Review-Tree"
    $reviewer = Get-SingleTrailer -Message $message -Name "AI-Review-Agent"
    $reportHash = Get-SingleTrailer -Message $message -Name "AI-Review-Report"
    $summary = Get-SingleTrailer -Message $message -Name "AI-Review-Summary"
    if ($reviewTree -ne $tree) {
        throw "Commit $Commit 审核时对应 tree $reviewTree，但实际包含 tree $tree。"
    }
    if ($reviewer -notmatch '(?i)\b(Codex|Claude(?: Code)?|GitHub Copilot|Cursor|GPT(?:-[0-9.]+)?|Gemini|AI Agent)\b' -or
        $reportHash -notmatch '^[0-9a-f]{64}$' -or $summary.Length -lt 12) {
        throw "Commit $Commit 包含无效的 AI 审核 trailers。"
    }
    Write-Host "Commit 的 AI 审核检查通过：$Commit"
    exit 0
}

$hasStagedChanges = Test-HasStagedChanges
if (-not $hasStagedChanges) {
    if ($PSCmdlet.ParameterSetName -eq "Message") {
        $message = Get-Content -LiteralPath $MessageFile -Raw -Encoding UTF8
        if ($message -match '(?m)^AI-Review-(?:Agent|Tree|Report|Summary):') {
            $tree = (Invoke-Git -Arguments @("write-tree")) | Select-Object -First 1
            $reviewTree = Get-SingleTrailer -Message $message -Name "AI-Review-Tree"
            $null = Get-SingleTrailer -Message $message -Name "AI-Review-Agent"
            $null = Get-SingleTrailer -Message $message -Name "AI-Review-Report"
            $null = Get-SingleTrailer -Message $message -Name "AI-Review-Summary"
            if ($reviewTree -ne $tree) {
                throw "现有 AI 审核 trailers 与修订后的 Commit tree 不匹配。"
            }
        }
    }
    Write-Host "暂存 tree 没有改动，已跳过 AI 审核检查。"
    exit 0
}

$receipt = Assert-StagedReceipt
if ($PSCmdlet.ParameterSetName -eq "Message") {
    $message = Get-Content -LiteralPath $MessageFile -Raw -Encoding UTF8
    $message = [regex]::Replace($message, '(?m)^AI-Review-(?:Agent|Tree|Report|Summary):.*\r?\n?', '').TrimEnd()
    if ($env:SRX_PUBLIC_COMMIT -ne "1") {
        $summary = ([string]$receipt.summary -replace '[\r\n]+', ' ').Trim()
        if ($summary.Length -gt 180) {
            $summary = $summary.Substring(0, 180)
        }
        $message += "`n`n" +
            "AI-Review-Agent: $([string]$receipt.reviewer)`n" +
            "AI-Review-Tree: $([string]$receipt.tree)`n" +
            "AI-Review-Report: $([string]$receipt.reportHash)`n" +
            "AI-Review-Summary: $summary"
    }
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [IO.File]::WriteAllText((Resolve-Path -LiteralPath $MessageFile).Path, $message + "`n", $utf8NoBom)
}

Write-Host "AI 审核暂存 tree 检查通过：$([string]$receipt.tree)"
