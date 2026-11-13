"""在单个 pwsh 进程内为每个用例建独立 runspace，替代「每用例一个 pwsh 进程」。

## 为什么存在

`.github/tests` 里有一批 parity 测试，做法是：从 `run-storage-redirect-scenarios.ps1`
抽出某个函数，拼上 stub 与尾部断言，写成临时 `.ps1`，再 `pwsh -File` 跑一次。
CI（ubuntu runner）实测单次 0.74–0.81s，其中 pwsh 冷启动约 0.2s，其余是**每次重新
解析编译那份 1.5–9.7KB 的函数体**——216 个用例合计重复解析 1.2MB PowerShell 源码，
约 180s，是 quality job 超时风险与关键路径的主要来源。

改成「一个进程 + 每用例一个新 runspace」后实测 0.030s/case（约 25 倍）。选 runspace
而不是「单进程内 for 循环」，是因为 runspace 给每个用例独立的 `$script:`/`$global:`
作用域，等价于今天的进程级隔离；同进程共作用域会让用例之间互相污染 `$script:Failures`
这类状态。

## 三条与 `pwsh -File` 的语义差异（已实测，调用方必须遵守）

1. **`exit N` 在 runspace 里被静默吞掉**：不抛异常、`$LASTEXITCODE` 与
   `[Environment]::ExitCode` 都不变，脚本静默结束。所以用例尾部**不得使用 `exit`**，
   必须写 `$global:SRX_CASE_RC = N`；本模块会扫描用例脚本，发现真 `exit` 直接报错，
   避免「测试静默全部变绿」。
2. **`throw` 之前已写出的输出**：直接 `Invoke()` 会整体丢弃，所以驱动在 runspace
   内部用 try/catch 包住并 `6>&1` 收流，保住抛错前的输出（与真进程一致）。
3. **`Write-Host`** 默认只进 Information 流、不进返回值；`6>&1` 合并后可保住它与
   `Write-Output` 的相对顺序。

## 环境变量隔离

环境变量是**进程级**的，runspace 之间共享。驱动在跑每个用例前清空全部 `SRX_*`
变量再按用例设置，等价于原来每个进程拿到的那份干净 env。
"""

import json
import os
import subprocess
import tempfile
from pathlib import Path

# 驱动通过这个变量读用例自报的退出码。
CASE_RC_VAR = "SRX_CASE_RC"
# 用例脚本里存在真 exit 语句时，驱动用它上报，与普通断言失败区分开。
EXIT_USAGE_ERROR = 99

_DRIVER = r"""
param([string]$SpecPath)

$ErrorActionPreference = 'Stop'
$spec = Get-Content -LiteralPath $SpecPath -Raw -Encoding utf8 | ConvertFrom-Json

function Set-CaseEnvironment {
    param($CaseEnv)

    foreach ($name in @([System.Environment]::GetEnvironmentVariables().Keys)) {
        if ($name -like 'SRX_*') { [System.Environment]::SetEnvironmentVariable($name, $null) }
    }
    if ($null -ne $CaseEnv) {
        foreach ($property in $CaseEnv.PSObject.Properties) {
            [System.Environment]::SetEnvironmentVariable($property.Name, [string]$property.Value)
        }
    }
}

function Find-RealExitStatements {
    param([string]$Path)

    # 只认 PowerShell 的真 exit 语句。正则分不清「bash heredoc 字符串里的 exit」与
    # 真 exit，前者在拼接 shell 片段的函数里大量存在（如 Wait-ServiceResult）。
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($Path, [ref]$tokens, [ref]$errors)
    if ($errors -and $errors.Count -gt 0) {
        return @("解析失败：" + $errors[0].Message)
    }
    return @($ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.ExitStatementAst] }, $true) |
        ForEach-Object { "第 $($_.Extent.StartLineNumber) 行：$($_.Extent.Text.Trim())" })
}

foreach ($case in $spec.cases) {
    Set-CaseEnvironment -CaseEnv $case.env

    $exits = @(Find-RealExitStatements -Path $case.script)
    if ($exits.Count -gt 0) {
        [System.IO.File]::WriteAllText($case.out, '', [System.Text.UTF8Encoding]::new($false))
        [System.IO.File]::WriteAllText($case.err,
            ("用例脚本含真 exit 语句，runspace 下会被静默吞掉；请改用 `$global:SRX_CASE_RC = <退出码>：" + [Environment]::NewLine + ($exits -join [Environment]::NewLine)),
            [System.Text.UTF8Encoding]::new($false))
        [System.IO.File]::WriteAllText($case.rc, '99', [System.Text.UTF8Encoding]::new($false))
        Write-Output ("@@CASE " + $case.name)
        continue
    }

    $runspace = [runspacefactory]::CreateRunspace()
    $runspace.Open()
    $shell = [powershell]::Create()
    $shell.Runspace = $runspace

    $arguments = @($case.args | ForEach-Object { "'" + ([string]$_).Replace("'", "''") + "'" })
    $invocation = "& '" + $case.script.Replace("'", "''") + "'"
    if ($arguments.Count -gt 0) { $invocation += ' ' + ($arguments -join ' ') }

    # try/catch 在 runspace 内：throw 之前写出的输出要保住，与真进程一致。
    $body = @"
`$global:SRX_CASE_RC = 0
`$global:SRX_CASE_ERR = ''
try {
    & { $invocation } 6>&1
} catch {
    `$global:SRX_CASE_ERR = `$_.Exception.Message
    `$global:SRX_CASE_RC = 1
}
"@

    $output = @()
    $failure = ''
    try {
        $shell.AddScript($body) | Out-Null
        $output = @($shell.Invoke())
    } catch {
        # 驱动自身出错（不是用例的 throw）：也必须显式上报，不能静默算通过。
        $failure = $_.Exception.Message
    }

    $rc = 0
    $caseError = ''
    try {
        $rawRc = $runspace.SessionStateProxy.GetVariable('SRX_CASE_RC')
        if ($null -ne $rawRc) { $rc = [int]$rawRc }
        $rawErr = $runspace.SessionStateProxy.GetVariable('SRX_CASE_ERR')
        if ($null -ne $rawErr) { $caseError = [string]$rawErr }
    } catch {
        $failure = $_.Exception.Message
    }
    if ($failure) { $rc = 1 }

    $text = (@($output | ForEach-Object { if ($null -eq $_) { 'null' } else { "$_" } }) -join "`n")
    if ($text) { $text += "`n" }

    [System.IO.File]::WriteAllText($case.out, $text, [System.Text.UTF8Encoding]::new($false))
    $errText = if ($caseError) { $caseError + "`n" } else { '' }
    if ($failure) { $errText += $failure + "`n" }
    [System.IO.File]::WriteAllText($case.err, $errText, [System.Text.UTF8Encoding]::new($false))
    [System.IO.File]::WriteAllText($case.rc, [string]$rc, [System.Text.UTF8Encoding]::new($false))

    $shell.Dispose()
    $runspace.Close()
    $runspace.Dispose()

    Write-Output ("@@CASE " + $case.name)
}
Write-Output '@@DRIVER_DONE'
"""


class CaseResult:
    """与 `subprocess.CompletedProcess` 对齐的最小接口，便于断言代码按原样保留。"""

    __slots__ = ("returncode", "stdout", "stderr")

    def __init__(self, returncode, stdout, stderr):
        self.returncode = returncode
        self.stdout = stdout
        self.stderr = stderr

    def __repr__(self):
        return f"CaseResult(returncode={self.returncode!r})"


class PwshCaseDriverError(RuntimeError):
    """驱动层失败：驱动自身异常退出，或结果文件缺失。"""


def run_pwsh_cases(cases, pwsh, timeout=1800):
    """在一个 pwsh 进程内跑完所有用例，返回 `{用例名: CaseResult}`。

    `cases` 每项为 dict：``name``（唯一名）、``script``（用例脚本路径）、
    ``args``（脚本位置参数，默认空）、``env``（本用例的 `SRX_*` 环境变量，默认空）。

    用例脚本若含真 `exit` 语句，该用例以 `EXIT_USAGE_ERROR` 作为 returncode 返回、
    并在 stderr 说明原因——不抛异常，让调用方的 subTest 报出具体是哪个用例写错了。
    """
    if not cases:
        return {}

    names = [case["name"] for case in cases]
    duplicates = {name for name in names if names.count(name) > 1}
    if duplicates:
        raise PwshCaseDriverError(f"用例名重复，结果会互相覆盖：{sorted(duplicates)}")

    for case in cases:
        if not Path(case["script"]).is_file():
            raise PwshCaseDriverError(f"用例脚本不存在：{case['script']}")

    with tempfile.TemporaryDirectory(prefix="srx-pwsh-cases-") as workdir:
        workdir = Path(workdir)
        entries = []
        for index, case in enumerate(cases):
            entries.append({
                "name": case["name"],
                "script": str(Path(case["script"]).resolve()),
                "args": [str(value) for value in case.get("args") or ()],
                "env": {str(k): str(v) for k, v in (case.get("env") or {}).items()},
                "out": str(workdir / f"{index}.out"),
                "err": str(workdir / f"{index}.err"),
                "rc": str(workdir / f"{index}.rc"),
            })

        spec_path = workdir / "spec.json"
        spec_path.write_text(json.dumps({"cases": entries}, ensure_ascii=False), encoding="utf-8")
        driver_path = workdir / "driver.ps1"
        driver_path.write_text(_DRIVER, encoding="utf-8", newline="\n")

        environment = {k: v for k, v in os.environ.items() if not k.startswith("SRX_")}
        completed = subprocess.run(
            [pwsh, "-NoProfile", "-NonInteractive", "-File", str(driver_path), str(spec_path)],
            text=True, capture_output=True, timeout=timeout, env=environment,
        )
        if completed.returncode != 0 or "@@DRIVER_DONE" not in completed.stdout:
            raise PwshCaseDriverError(
                "pwsh 用例驱动异常退出：rc=%s\nstdout:\n%s\nstderr:\n%s"
                % (completed.returncode, completed.stdout, completed.stderr)
            )

        results = {}
        for entry in entries:
            out_path = Path(entry["out"])
            err_path = Path(entry["err"])
            rc_path = Path(entry["rc"])
            if not (out_path.is_file() and err_path.is_file() and rc_path.is_file()):
                # 缺结果即视为失败：绝不让用例因驱动漏跑而静默通过。
                raise PwshCaseDriverError(f"用例 {entry['name']} 未产出结果文件")
            results[entry["name"]] = CaseResult(
                int(rc_path.read_text(encoding="utf-8").strip() or 0),
                out_path.read_text(encoding="utf-8"),
                err_path.read_text(encoding="utf-8"),
            )

        reported = {
            line[len("@@CASE "):] for line in completed.stdout.splitlines() if line.startswith("@@CASE ")
        }
        missing = {case["name"] for case in cases} - reported
        if missing:
            raise PwshCaseDriverError(f"驱动未报告这些用例（疑似中途中断）：{sorted(missing)}")
        return results


def write_case_script(base, filename, content):
    """把用例脚本写到 `base` 目录下并返回路径，统一 UTF-8 与 LF 行尾。"""
    path = Path(base) / filename
    path.write_text(content, encoding="utf-8", newline="\n")
    return path
