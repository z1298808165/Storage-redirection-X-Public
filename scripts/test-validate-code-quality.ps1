[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Invoke-Git {
    param(
        [string]$WorkingDirectory,
        [string[]]$Arguments
    )

    & git -C $WorkingDirectory @Arguments | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Git 命令执行失败：git -C $WorkingDirectory $($Arguments -join ' ')"
    }
}

function Write-Utf8File {
    param(
        [string]$Path,
        [string]$Text
    )

    $parent = Split-Path -Parent $Path
    if ($parent) {
        New-Item -ItemType Directory -Force -Path $parent | Out-Null
    }
    [IO.File]::WriteAllText($Path, $Text, [Text.UTF8Encoding]::new($false))
}

function Invoke-StagedCase {
    param(
        [string]$Name,
        [string]$Path,
        [string]$Text,
        [bool]$ShouldPass
    )

    $target = Join-Path $fixtureRoot $Path
    Write-Utf8File -Path $target -Text $Text
    Invoke-Git -WorkingDirectory $fixtureRoot -Arguments @("add", "--", $Path)

    Push-Location -LiteralPath $fixtureRoot
    try {
        $previousErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & $shellBin -NoProfile -ExecutionPolicy Bypass -File $validator -Staged *> $null
        $passed = $LASTEXITCODE -eq 0
        $ErrorActionPreference = $previousErrorActionPreference
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
        Pop-Location
    }
    if ($passed -ne $ShouldPass) {
        throw "用例 $Name 结果不符合预期：expected=$ShouldPass actual=$passed"
    }

    Invoke-Git -WorkingDirectory $fixtureRoot -Arguments @("restore", "--staged", "--", $Path)
    Remove-Item -LiteralPath $target -Force
}

    # 先让原生 git 完整结束再取首行：pwsh 7 的 Select-Object -First 会提前终止上游
    # 管道，提前被终止的原生命令不会写入 $LASTEXITCODE，StrictMode 下读取会直接报错。
    $repoRoot = ((& git rev-parse --show-toplevel) | Select-Object -First 1)
    if ($LASTEXITCODE -ne 0) {
        throw "无法确定仓库根目录。"
    }
    # 按用户环境约定优先使用 pwsh；仅在确实不存在时才回退（本机已装 PowerShell 7）。
    $shellBin = "powershell"
    if (Get-Command pwsh -ErrorAction SilentlyContinue) {
        $shellBin = "pwsh"
    }
$validator = Join-Path $repoRoot "scripts/validate-code-quality.ps1"
$fixtureRoot = Join-Path $repoRoot "temp/test-validate-code-quality"

if (Test-Path -LiteralPath $fixtureRoot) {
    Remove-Item -LiteralPath $fixtureRoot -Recurse -Force
}

try {
    New-Item -ItemType Directory -Force -Path (Join-Path $fixtureRoot "scripts") | Out-Null
    Copy-Item -LiteralPath $validator -Destination (Join-Path $fixtureRoot "scripts/validate-code-quality.ps1")
    Invoke-Git -WorkingDirectory $fixtureRoot -Arguments @("init", "--initial-branch=main")
    Invoke-Git -WorkingDirectory $fixtureRoot -Arguments @("config", "user.name", "质量门禁测试")
    Invoke-Git -WorkingDirectory $fixtureRoot -Arguments @("config", "user.email", "quality-test@example.invalid")
    Invoke-Git -WorkingDirectory $fixtureRoot -Arguments @("add", "scripts/validate-code-quality.ps1")
    Invoke-Git -WorkingDirectory $fixtureRoot -Arguments @("commit", "-m", "测试：建立质量门禁基线")

    Invoke-StagedCase -Name "中文注释" -Path "src/chinese.rs" -Text "// 配置变化后刷新缓存。`n" -ShouldPass $true
    Invoke-StagedCase -Name "英文注释" -Path "src/english.rs" -Text "// Refresh the cache after configuration changes.`n" -ShouldPass $false
    Invoke-StagedCase -Name "中文理由豁免" -Path "src/allowed.rs" -Text "// quality-allow(chinese-language): 上游接口要求保留原始英文说明`n// Keep this upstream wording unchanged.`n" -ShouldPass $true
    Invoke-StagedCase -Name "英文文档" -Path "docs/english.md" -Text "# Build instructions for contributors`n" -ShouldPass $false
    Invoke-StagedCase -Name "文档命令" -Path "docs/command.md" -Text "git status --short --branch`n" -ShouldPass $true
    Invoke-StagedCase -Name "技术标识" -Path "src/identifier.rs" -Text "// HTTP_API_V2`n" -ShouldPass $true
    Invoke-StagedCase -Name "Rust 属性" -Path "src/attribute.rs" -Text "#[cfg(target_os = \"android\")]`n" -ShouldPass $true
    Invoke-StagedCase -Name "C 预处理器" -Path "native/include/example.h" -Text "#include <stdint.h>`n" -ShouldPass $true
    Invoke-StagedCase -Name "英文 PowerShell 注释" -Path "scripts/english.ps1" -Text "# Validate the generated archive before upload.`n" -ShouldPass $false
    Invoke-StagedCase -Name "许可证标识" -Path "src/license.rs" -Text "// SPDX-License-Identifier: Apache-2.0`n" -ShouldPass $true
    Invoke-StagedCase -Name "第三方内容" -Path "vendor/example/source.rs" -Text "// Keep upstream comments unchanged.`n" -ShouldPass $true
    # 解引用赋值行首星号后紧跟标识符，是代码不是注释文本，不得误报中文语言违规。
    Invoke-StagedCase -Name "解引用赋值" -Path "src/deref.rs" -Text "*slot = Some(Arc::new(host));`n" -ShouldPass $true
    # 行首星号后跟空白的块注释续行仍按注释文本参与中文语言判定。
    Invoke-StagedCase -Name "英文块注释续行" -Path "src/blockdoc.rs" -Text " * Refresh the cache after configuration changes.`n" -ShouldPass $false

    $commitPath = "src/commit-mode.rs"
    Write-Utf8File -Path (Join-Path $fixtureRoot $commitPath) -Text "// Validate this committed English sentence.`n"
    Invoke-Git -WorkingDirectory $fixtureRoot -Arguments @("add", "--", $commitPath)
    Invoke-Git -WorkingDirectory $fixtureRoot -Arguments @("commit", "-m", "测试：验证提交模式")
    Push-Location -LiteralPath $fixtureRoot
    try {
        $previousErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        & $shellBin -NoProfile -ExecutionPolicy Bypass -File $validator -Commit HEAD *> $null
        $commitModePassed = $LASTEXITCODE -eq 0
        $ErrorActionPreference = $previousErrorActionPreference
        if ($commitModePassed) {
            throw "提交模式未拦截英文自然语言。"
        }
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
        Pop-Location
    }

    Write-Host "中文内容增量质量门禁回归测试通过。"
}
finally {
    if (Test-Path -LiteralPath $fixtureRoot) {
        Remove-Item -LiteralPath $fixtureRoot -Recurse -Force
    }
}
