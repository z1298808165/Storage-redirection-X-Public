[CmdletBinding()]
param(
    [string]$Ref = ":"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# git show 的文件内容必须按 UTF-8 解码：Windows PowerShell 5.1 用
# [Console]::OutputEncoding 解码原生命令输出，GBK 控制台下中文注释的尾字节
# （如「。」= E3 80 82 的 0x82）会把下一行开头的换行符一并吞掉，导致 #[test]
# 不再位于行首、正则漏匹配，白名单测试被误报缺失。无控制台句柄的宿主下设置
# 可能失败，失败时保持系统默认继续执行。
try {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
} catch {
}

$allowed = @{
    "src/hook/media_fuse.rs" = @(
        "caller_package_token_matches_xradiant_private_sqlite",
        "unrelated_package_token_does_not_match_generic_sqlite",
        "recent_sqlite_access_keeps_owner_caller_for_backend_retry",
        "zero_size_shm_is_not_poisoned",
        "full_size_shm_size_is_not_poisoned",
        "partial_shm_size_is_poisoned"
    )
    "src/hook/ops/mutation/meta.rs" = @(
        "futimesat_fallback_converts_timeval"
    )
    "src/hook/ops/open.rs" = @(
        "resolve_beneath_and_in_root_keep_original_dirfd",
        "nofollow_only_rejects_actual_symlink",
        "final_path_only_resolve_flags_can_still_redirect"
    )
    "src/hook/ops/rename.rs" = @(
        "exchange_or_whiteout_across_boundary_is_exdev",
        "noreplace_is_left_to_kernel_target_check"
    )
    "src/hook/path.rs" = @(
        "dot_segments_resolve_against_existing_parent",
        "duplicate_slashes_are_collapsed",
        "relative_dirfd_resolves_dot_segments",
        "magic_fd_link_resolves_to_target"
    )
    "src/zn_api.rs" = @(
        "hyos_identity_validation",
        "hyos_struct_layout_matches_api",
        "hyos_registration_accepts_only_matching_runtime",
        "null_specialize_args_is_ignored",
        "cstr_to_str_rejects_null_invalid_utf8_and_long_inputs"
    )
    "src/lifecycle/companion_mount.rs" = @(
        "collapsed_sibling_barriers_are_shadowed_and_served",
        "collapsed_sibling_barriers_dispose_inert",
        "healthy_collapsed_view_root_serves_subpaths",
        "reachable_barrier_on_top_is_not_served",
        "barrier_on_top_of_fuse_requires_detach",
        "stacked_fuse_on_top_of_barrier_is_served",
        "path_without_any_srx_fuse_is_not_served",
        "escaped_space_is_normalized"
    )
    "src/lifecycle/specialize_pre.rs" = @(
        "file_monitor_ui_bypass_covers_system_oem_and_legacy_names"
    )
    "src/monitor.rs" = @(
        "refines_dotless_native_identity_with_caller_uid",
        "refines_empty_identity_to_caller_uid_method",
        "keeps_identity_for_shared_uid_group",
        "keeps_dot_package_identity_unchanged",
        "keeps_identity_for_non_app_uid"
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
        throw "Git 命令执行失败：git $($Arguments -join ' ')"
    }
    return @($output)
}

if ($Ref -eq ":") {
    $files = Invoke-Git -Arguments @("ls-files", "--cached", "--", ":(glob)src/**/*.rs")
} else {
    $files = Invoke-Git -Arguments @("ls-tree", "-r", "--name-only", $Ref) |
        Where-Object {
            $_.StartsWith("src/", [StringComparison]::Ordinal) -and
            $_.EndsWith(".rs", [StringComparison]::OrdinalIgnoreCase)
        }
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
            $errors.Add("发现不允许的 Rust 内联测试：${file}::$name")
        }
    }
}
foreach ($file in $allowed.Keys) {
    foreach ($name in $allowed[$file]) {
        if (-not $actual.ContainsKey($file) -or $actual[$file] -notcontains $name) {
            $errors.Add("缺少应保留的上游 Rust 内联测试：${file}::$name")
        }
    }
}

if ($errors.Count -gt 0) {
    throw ($errors -join "`n")
}

$allowedCount = ($allowed.Values | ForEach-Object { $_.Count } | Measure-Object -Sum).Sum
Write-Host "Rust 内联测试白名单检查通过：已保留 $allowedCount 个登记测试。"
