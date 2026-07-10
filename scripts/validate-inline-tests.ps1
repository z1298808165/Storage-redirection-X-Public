[CmdletBinding()]
param(
    [string]$Ref = ":"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$allowed = @{
    "src/hook/media_fuse.rs" = @(
        "caller_package_token_matches_xradiant_private_sqlite",
        "unrelated_package_token_does_not_match_generic_sqlite",
        "recent_sqlite_access_keeps_owner_caller_for_backend_retry"
    )
    "src/lifecycle/specialize_pre.rs" = @(
        "file_monitor_ui_bypass_covers_system_oem_and_legacy_names"
    )
    "src/redirect/policy.rs" = @(
        "file_monitor_ui_detects_system_and_oem_file_shells",
        "media_intermediate_includes_file_ui_for_attribution"
    )
}

function Invoke-Git {
    param([string[]]$Arguments)

    $output = & git @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Git command failed: git $($Arguments -join ' ')"
    }
    return @($output)
}

if ($Ref -eq ":") {
    $files = Invoke-Git -Arguments @("ls-files", "--cached", "--", "*.rs")
} else {
    $files = Invoke-Git -Arguments @("ls-tree", "-r", "--name-only", $Ref) |
        Where-Object { $_.EndsWith(".rs", [StringComparison]::OrdinalIgnoreCase) }
}

$actual = @{}
$testPattern = '(?ms)^[ \t]*#\[(?:[A-Za-z_][A-Za-z0-9_:]*::)?test(?:\([^\]]*\))?\][ \t]*\r?\n(?:[ \t]*#\[[^\]]+\][ \t]*\r?\n)*[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)'

foreach ($file in $files) {
    if ([string]::IsNullOrWhiteSpace($file)) {
        continue
    }
    $object = if ($Ref -eq ":") { ":$file" } else { "${Ref}:$file" }
    $content = (Invoke-Git -Arguments @("show", $object)) -join "`n"
    foreach ($match in [regex]::Matches($content, $testPattern)) {
        $name = $match.Groups[1].Value
        if (-not $actual.ContainsKey($file)) {
            $actual[$file] = New-Object System.Collections.Generic.List[string]
        }
        $actual[$file].Add($name)
    }
}

$errors = New-Object System.Collections.Generic.List[string]
foreach ($file in $actual.Keys) {
    foreach ($name in $actual[$file]) {
        if (-not $allowed.ContainsKey($file) -or $allowed[$file] -notcontains $name) {
            $errors.Add("Disallowed Rust inline test: ${file}::$name")
        }
    }
}
foreach ($file in $allowed.Keys) {
    foreach ($name in $allowed[$file]) {
        if (-not $actual.ContainsKey($file) -or $actual[$file] -notcontains $name) {
            $errors.Add("Missing preserved upstream Rust inline test: ${file}::$name")
        }
    }
}

if ($errors.Count -gt 0) {
    throw ($errors -join "`n")
}

Write-Host "Rust inline test allowlist check passed: 6 upstream tests preserved."
