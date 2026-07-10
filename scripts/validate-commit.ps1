[CmdletBinding(DefaultParameterSetName = "MessageFile")]
param(
    [Parameter(Mandatory = $true, ParameterSetName = "MessageFile")]
    [string]$MessageFile,

    [Parameter(Mandatory = $true, ParameterSetName = "Commit")]
    [string]$Commit
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[Console]::InputEncoding = $utf8NoBom
[Console]::OutputEncoding = $utf8NoBom
$OutputEncoding = $utf8NoBom

function Get-GitLines {
    param([string[]]$Arguments)

    $output = & git @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Git command failed: git $($Arguments -join ' ')"
    }
    return @($output)
}

function Get-PathKind {
    param([string]$Path)

    $normalized = $Path.Replace("\", "/")
    if (
        $normalized -match '(^|/)(AGENTS|CLAUDE|CONTRIBUTING|README)(\.[^/]*)?$' -or
        $normalized -match '(^|/)docs/' -or
        $normalized -match '\.(md|mdx|rst|adoc)$'
    ) {
        return "docs"
    }
    if ($normalized -match '^\.github/(workflows|actions)/') {
        return "ci"
    }
    if (
        $normalized -match '(^|/)(tests?|testdata|fixtures)(/|$)' -or
        $normalized -match '(^|/)[^/]*(Test|Tests|Spec)\.(kt|java|rs|js|ts|tsx)$'
    ) {
        return "tests"
    }
    return "code"
}

if ($PSCmdlet.ParameterSetName -eq "MessageFile") {
    $message = Get-Content -LiteralPath $MessageFile -Raw -Encoding UTF8
    $paths = Get-GitLines -Arguments @("diff", "--cached", "--name-only", "--diff-filter=ACMR")
} else {
    $message = (Get-GitLines -Arguments @("show", "-s", "--format=%B", $Commit)) -join "`n"
    $paths = Get-GitLines -Arguments @(
        "diff-tree", "--root", "--no-commit-id", "--name-only", "-r", $Commit
    )
}

$title = (($message -split "`r?`n", 2)[0]).Trim()
if ([string]::IsNullOrWhiteSpace($title)) {
    throw "Commit title must not be empty."
}
if ($title.Length -gt 72) {
    throw "Commit title must not exceed 72 characters; got $($title.Length)."
}

$releasePattern = '^Releases: \u53d1\u5e03 \d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$'
$titlePattern = '^(\u529f\u80fd|\u4fee\u590d|\u91cd\u6784|\u6027\u80fd|\u6d4b\u8bd5|\u6587\u6863|\u6784\u5efa|CI|\u4f9d\u8d56|\u754c\u9762|\u7ef4\u62a4|\u53d1\u5e03|\u56de\u9000)(?:\([^)\r\n]+\))?\uFF1A(.+)$'
$match = [regex]::Match($title, $titlePattern)
if (-not $match.Success -and $title -notmatch $releasePattern) {
    throw @"
Invalid commit title: $title
Use: TYPE(optional-scope) + FULLWIDTH COLON + Chinese description
Allowed TYPE values are documented in AGENTS.md and CONTRIBUTING.md.
"@
}

if ($match.Success) {
    $description = $match.Groups[2].Value.Trim()
    if ($description.Length -lt 2 -or $description -notmatch '[\u3400-\u9fff]') {
        throw "Commit description must be a clear Chinese phrase; technical English terms are allowed."
    }

    $pathKinds = @($paths | Where-Object { $_ } | ForEach-Object { Get-PathKind -Path $_ } | Sort-Object -Unique)
    if ($pathKinds.Count -gt 1 -and $pathKinds -contains "docs") {
        throw "Documentation and non-documentation changes must be split by purpose."
    }
    $isDocumentationType = $title -match '^\u6587\u6863(?:\(|\uFF1A)'
    $isCiType = $title -match '^CI(?:\(|\uFF1A)'
    if ($isDocumentationType -and ($pathKinds | Where-Object { $_ -ne "docs" })) {
        throw "A documentation commit may contain documentation files only."
    }
    if (-not $isDocumentationType -and $pathKinds.Count -eq 1 -and $pathKinds[0] -eq "docs") {
        throw "A documentation-only commit must use the documentation type."
    }
    if ($isCiType -and ($pathKinds | Where-Object { $_ -ne "ci" })) {
        throw "A CI commit may contain workflow/action files only; use the build type for scripts."
    }
}

Write-Host "Commit convention check passed: $title"
