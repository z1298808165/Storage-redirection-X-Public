import json
import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
BASH = os.environ.get("SRX_TEST_BASH") or shutil.which("bash")
PWSH = shutil.which("pwsh")


class FileMutationScenarioTest(unittest.TestCase):
    @unittest.skipUnless(shutil.which("bash"), "需要 Bash 验证物理落点断言")
    def test_bash_physical_assertions(self) -> None:
        self.check_physical_assertions("bash")

    @unittest.skipUnless(shutil.which("pwsh") and shutil.which("bash"), "需要 PowerShell 和 Bash 验证物理落点断言")
    def test_powershell_physical_assertions(self) -> None:
        self.check_physical_assertions("pwsh")

    def check_physical_assertions(self, runner: str) -> None:
        extension = "sh" if runner == "bash" else "ps1"
        source = (ROOT / f".github/tests/run-storage-redirect-scenarios.{extension}").read_text(encoding="utf-8")
        start, end = (
            ("run_file_mutation_lifecycle_scenario() {", "run_standard_scenario() {")
            if runner == "bash" else
            ("function Invoke-FileMutationLifecycleScenario {", "function Invoke-StandardScenario {")
        )
        function = source[source.index(start):source.index(end)]
        for failure in (None, "missing", "content", "newline", "temporary", "mapped_request", "mapped_shadow", "request_missing", "service"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                base = Path(directory)
                for name in ("Sandbox", "Allowed", "Request", "RequestSibling", "Mapped"):
                    for storage in ("backend", "private"):
                        (base / storage / "Download" / f"SrtMutation{name}").mkdir(parents=True)
                for storage, name in (
                    ("private", "Sandbox"), ("backend", "Allowed"),
                    ("backend", "Mapped"), ("private", "RequestSibling"),
                ):
                    (base / storage / "Download" / f"SrtMutation{name}" / "complete.txt").write_text(
                        "srx-mutation-lifecycle-v1"
                    )
                marker = base / "backend/Download/SrtMutationMapped/complete.txt"
                if failure == "missing":
                    marker.unlink()
                elif failure == "content":
                    marker.write_text("wrong")
                elif failure == "newline":
                    marker.write_text("srx-mutation-lifecycle-v1\n")
                elif failure == "temporary":
                    (base / "backend/Download/SrtMutationSandbox/.partial.tmp").touch()
                elif failure == "mapped_request":
                    (base / "backend/Download/SrtMutationRequest/leak.txt").touch()
                elif failure == "mapped_shadow":
                    (base / "private/Download/SrtMutationMapped/leak.txt").touch()
                elif failure == "request_missing":
                    (base / "backend/Download/SrtMutationRequest").rmdir()
                if runner == "bash":
                    script = '''
                    set -eu
                    REAL_ROOT=/storage/emulated/0
                    BACKEND_ROOT=$1/backend
                    BACKEND_PRIVATE_ROOT=$1/private
                    run_service_case() { test "$FAIL_SERVICE" != 1; }
                    adb_su() { bash -eu -c "$1"; }
                    ''' + function + '\nrun_file_mutation_lifecycle_scenario 42'
                    script_path = base / "assertions.sh"
                    script_path.write_text(script, encoding="utf-8", newline="\n")
                    command = ["bash", script_path.as_posix(), base.as_posix()]
                else:
                    script = '''
                    param([string]$BasePath)
                    $ErrorActionPreference = "Stop"
                    $RealRoot = "/storage/emulated/0"
                    $BackendRoot = "$BasePath/backend"
                    $BackendPrivateRoot = "$BasePath/private"
                    $script:Failures = [System.Collections.Generic.List[string]]::new()
                    function Invoke-ServiceCase { [pscustomobject]@{ Ok = $env:FAIL_SERVICE -ne "1" } }
                    function Test-Su {
                        param([string]$Command)
                        & bash -eu -c $Command
                        return $LASTEXITCODE -eq 0
                    }
                    ''' + function + '\nif (-not (Invoke-FileMutationLifecycleScenario 42)) { exit 1 }'
                    script_path = base / "assertions.ps1"
                    script_path.write_text(script, encoding="utf-8")
                    command = ["pwsh", "-NoProfile", "-File", str(script_path), directory]
                result = subprocess.run(
                    command, text=True, capture_output=True, timeout=20,
                    env={**os.environ, "FAIL_SERVICE": "1" if failure == "service" else "0"},
                )
                self.assertEqual(failure is None, result.returncode == 0, result.stderr)

    def test_manifest_and_dispatch_register_lifecycle(self) -> None:
        manifest = json.loads((ROOT / ".github/tests/storage-redirect-scenarios.json").read_text(encoding="utf-8"))
        self.assertIn(42, [item["id"] for item in manifest["scenarios"]])
        bash = (ROOT / ".github/tests/run-storage-redirect-scenarios.sh").read_text(encoding="utf-8")
        powershell = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        self.assertIn('export -f run_file_mutation_lifecycle_scenario', bash)
        self.assertIn('run_file_mutation_lifecycle_scenario "$scenario"', bash)
        self.assertIn(
            '42 { (Invoke-FileMutationLifecycleScenario $Scenario) -and (Invoke-FileMutationMetadataScenario $Scenario) }',
            powershell,
        )

    @unittest.skipUnless(BASH and os.name == "posix", "需要 POSIX Bash 验证权限和链接断言")
    def test_metadata_assertions_cover_routes_and_reject_regressions(self) -> None:
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.sh").read_text(encoding="utf-8")
        function = source[
            source.index("run_file_mutation_metadata_scenario() {") :
            source.index("run_standard_scenario() {")
        ]
        cases = {
            None: (True, {}),
            "symlink-supported": (True, {"PASS_SYMLINK": "1"}),
            "symlink-eacces-without-control": (False, {"PASS_ALLOWED_SYMLINK": "1"}),
            "symlink-other-error": (False, {"FAIL_SYMLINK_OTHER": "1"}),
            "symlink-misleading-message": (False, {"FAIL_SYMLINK_OTHER": "1", "MISLEADING_MESSAGE": "1"}),
            "native-symlink-supported": (False, {"NATIVE_SYMLINK": "created"}),
            "native-hardlink-supported": (False, {"NATIVE_HARDLINK": "created"}),
            "symlink-shadow-leak": (False, {"LEAK_SYMLINK": "1"}),
            "hardlink-eacces": (True, {"HARDLINK_EACCES": "1"}),
            "hardlink-eacces-without-control": (False, {"HARDLINK_EACCES": "1", "PASS_ALLOWED_HARDLINK": "1"}),
            "hardlink-supported": (True, {"PASS_HARDLINK": "1"}),
            "hardlink-shadow-leak": (False, {"LEAK_HARDLINK": "1"}),
            "service-failure": (False, {"FAIL_CASE": "file_chmod"}),
            "hardlink-unsupported": (False, {"FAIL_HARDLINK": "1"}),
        }
        for route in ("Sandbox", "Allowed", "Request", "RequestSibling"):
            for fault in ("chmod", "fchmod", "content", "extra-entry", "missing-entry", "dangling-symlink", "wrong-symlink", "wrong-inode", "missing-hardlink"):
                cases[f"{route}-{fault}"] = (False, {"FAULT_ROUTE": route, "MUTATION_FAULT": fault, "PASS_SYMLINK": "1", "PASS_HARDLINK": "1"})
        for failure, (expected_success, failure_env) in cases.items():
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                base = Path(directory)
                backend = base / "backend" / "Download"
                private = base / "private" / "Download"
                for name, root in (
                    ("Sandbox", private),
                    ("Allowed", backend),
                    ("Mapped", backend),
                    ("RequestSibling", private),
                    ("Request", backend),
                    ("Mapped", private),
                ):
                    target = root / f"SrtMutation{name}"
                    target.mkdir(parents=True, exist_ok=True)
                    if (root == backend and name in ("Allowed", "Mapped")) or (
                        root == private and name in ("Sandbox", "RequestSibling")
                    ):
                        (target / "complete.txt").write_text("srx-mutation-lifecycle-v1")

                script = r'''
set -eu
REAL_ROOT=/storage/emulated/0
BACKEND_ROOT=$1/backend
BACKEND_PRIVATE_ROOT=$1/private
SRT_ARTIFACT_DIR=$1/artifacts
PAYLOAD=mutation-payload
native_symlink_result=${NATIVE_SYMLINK:-EACCES}
native_hardlink_result=${NATIVE_HARDLINK:-EPERM}
if [[ ${HARDLINK_EACCES:-0} == 1 ]]; then native_hardlink_result=EACCES; fi
mkdir -p "$SRT_ARTIFACT_DIR"
physical_path() {
  case "$1" in
    "$REAL_ROOT"/Download/SrtMutationSandbox/*) printf '%s/Download/SrtMutationSandbox/%s' "$BACKEND_PRIVATE_ROOT" "${1##*/}" ;;
    "$REAL_ROOT"/Download/SrtMutationAllowed/*) printf '%s/Download/SrtMutationAllowed/%s' "$BACKEND_ROOT" "${1##*/}" ;;
    "$REAL_ROOT"/Download/SrtMutationRequest/*) printf '%s/Download/SrtMutationMapped/%s' "$BACKEND_ROOT" "${1##*/}" ;;
    "$REAL_ROOT"/Download/SrtMutationRequestSibling/*) printf '%s/Download/SrtMutationRequestSibling/%s' "$BACKEND_PRIVATE_ROOT" "${1##*/}" ;;
    *) return 1 ;;
  esac
}
run_service_case() {
  local scenario=$1 label=$2 test_case=$3 pattern=$4 key
  shift 4
  declare -A extra=()
  while (($#)); do
    if [[ $1 == --es ]]; then extra[$2]=$3; shift 3; else shift; fi
  done
  [[ ${FAIL_CASE:-} != "$test_case" ]] || return 1
  local path target mode public_mode owner_bits result
  result="$SRT_ARTIFACT_DIR/scenario-${scenario}-${label}-result.txt"
  printf 'PASS [%s] operation succeeded\n' "$test_case" > "$result"
  case "$test_case" in
    file_create)
      path=$(physical_path "${extra[file_path]}")
      mkdir -p "$(dirname "$path")"
      printf '%s' "${extra[payload]:-}" > "$path"
      ;;
    file_chmod|file_fchmod)
      path=$(physical_path "${extra[file_path]}")
      mode=$((8#${extra[mode]}))
      owner_bits=$(((mode & 0700) >> 3))
      mode=$(((mode | owner_bits) & ~7))
      printf -v public_mode '%o' "$mode"
      chmod "$public_mode" "$path"
      ;;
    file_symlink)
      target=$(physical_path "${extra[target_file_path]}")
      if [[ ${LEAK_SYMLINK:-0} == 1 && ${extra[target_file_path]} == *SrtMutationRequestSibling/* ]]; then
        mkdir -p "$BACKEND_ROOT/Download/SrtMutationRequestSibling"
        ln -s missing-target "$BACKEND_ROOT/Download/SrtMutationRequestSibling/symbolic-link"
      fi
      if [[ ${PASS_SYMLINK:-0} == 1 || (${PASS_ALLOWED_SYMLINK:-0} == 1 && ${extra[target_file_path]} == *SrtMutationAllowed/*) ]]; then
        ln -s "${extra[file_path]}" "$target"
        printf '%s\n' 'PASS [file_symlink] link succeeded' > "$SRT_ARTIFACT_DIR/scenario-${scenario}-${label}-result.txt"
      elif [[ ${FAIL_SYMLINK_OTHER:-0} == 1 ]]; then
        printf '%s\n' "FAIL [file_symlink] link failed error=EIO ${MISLEADING_MESSAGE:+Permission denied EACCES}" > "$SRT_ARTIFACT_DIR/scenario-${scenario}-${label}-result.txt"
      else
        printf '%s\n' 'FAIL [file_symlink] link failed error=EACCES (Permission denied)' > "$SRT_ARTIFACT_DIR/scenario-${scenario}-${label}-result.txt"
      fi
      ;;
    file_readlink)
      path=$(physical_path "${extra[file_path]}")
      [[ $(readlink "$path") == "${extra[expected_path]}" ]]
      ;;
    file_read)
      path=$(physical_path "${extra[file_path]}")
      [[ $(cat "$path") == "${extra[expected_payload]}" ]]
      ;;
    file_link)
      result="$SRT_ARTIFACT_DIR/scenario-${scenario}-${label}-result.txt"
      if [[ ${LEAK_HARDLINK:-0} == 1 && ${extra[target_file_path]} == *SrtMutationSandbox/* ]]; then
        mkdir -p "$BACKEND_ROOT/Download/SrtMutationSandbox"
        ln "$(physical_path "${extra[file_path]}")" "$BACKEND_ROOT/Download/SrtMutationSandbox/hard-link"
      fi
      if [[ ${PASS_HARDLINK:-0} == 1 || (${PASS_ALLOWED_HARDLINK:-0} == 1 && ${extra[target_file_path]} == *SrtMutationAllowed/*) ]]; then
        path=$(physical_path "${extra[file_path]}")
        target=$(physical_path "${extra[target_file_path]}")
        ln "$path" "$target"
        printf '%s\n' 'PASS [file_link] link succeeded' > "$result"
      elif [[ ${HARDLINK_EACCES:-0} == 1 ]]; then
        printf '%s\n' 'FAIL [file_link] link failed error=EACCES (Permission denied)' > "$result"
      elif [[ ${FAIL_HARDLINK:-0} == 1 ]]; then
        printf '%s\n' 'FAIL [file_link] link failed error=EOPNOTSUPP' > "$result"
      else
        printf '%s\n' 'FAIL [file_link] link failed error=EPERM (Operation not permitted)' > "$result"
      fi
      ;;
    *) return 9 ;;
  esac
  if [[ -n ${FAULT_ROUTE:-} && $label == mutation-${FAULT_ROUTE}-* ]]; then
    case "${MUTATION_FAULT:-}:$test_case" in
      chmod:file_chmod)
        [[ ${extra[mode]} != 0400 ]] || chmod 777 "$(physical_path "${extra[file_path]}")" ;;
      fchmod:file_fchmod) chmod 777 "$(physical_path "${extra[file_path]}")" ;;
      content:file_link) printf corrupt > "$(physical_path "${extra[file_path]}")" ;;
      extra-entry:file_link) touch "$(dirname "$(physical_path "${extra[file_path]}")")/unexpected" ;;
      missing-entry:file_link) rm "$(dirname "$(physical_path "${extra[file_path]}")")/metadata.txt" ;;
      dangling-symlink:file_symlink|wrong-symlink:file_symlink)
        target=$(physical_path "${extra[target_file_path]}")
        rm "$target"
        ln -s complete.txt "$target"
        [[ $MUTATION_FAULT != dangling-symlink ]] || { rm "$target"; ln -s missing "$target"; } ;;
      wrong-inode:file_link)
        target=$(physical_path "${extra[target_file_path]}")
        rm "$target"
        cp "$(physical_path "${extra[file_path]}")" "$target" ;;
      missing-hardlink:file_link) rm "$(physical_path "${extra[target_file_path]}")" ;;
    esac
  fi
  [[ -z $pattern ]] || grep -q "$pattern" "$result"
  if [[ -z $pattern ]]; then
    printf 'PENDING [%s] raw_result=%s\n' "$test_case" "$result"
  else
    cat "$result"
  fi
}
adb_su() { bash -eu -c "$1"; }
'''
                body = script + function + "\nrun_file_mutation_metadata_scenario 42\n"
                script_path = base / "metadata-assertions.sh"
                script_path.write_text(body, encoding="utf-8", newline="\n")
                result = subprocess.run(
                    [BASH, "-eu", str(script_path), str(base)],
                    text=True,
                    capture_output=True,
                    timeout=20,
                    env={**os.environ, **failure_env},
                )
                self.assertEqual(expected_success, result.returncode == 0, result.stderr + result.stdout)
                verdicts = [line for line in result.stdout.splitlines() if line.startswith("PASS [file_") and "_boundary]" in line]
                self.assertEqual(8 if expected_success else 0, len(verdicts), result.stdout)
                if expected_success:
                    self.assertNotIn("FAIL [", result.stdout + result.stderr)
                    self.assertIn("checks=physical,content,zero_shadow", result.stdout)
                    last_pending = result.stdout.rindex("PENDING [")
                    self.assertGreater(result.stdout.index(verdicts[0]), last_pending)
                    expected_outcome = "created" if failure == "symlink-supported" else "denied errno=EACCES control=unredirected,Allowed"
                    self.assertIn(f"PASS [file_symlink_boundary] scenario=42 route=RequestSibling outcome={expected_outcome}", result.stdout)
                    raw_result = base / "artifacts/scenario-42-mutation-RequestSibling-symlink-result.txt"
                    self.assertIn("PASS [file_symlink]" if failure == "symlink-supported" else "FAIL [file_symlink]", raw_result.read_text())
                elif failure in ("symlink-other-error", "hardlink-unsupported"):
                    self.assertIn("reason=unexpected_result", result.stderr)
                    self.assertIn("FAIL [file_", result.stderr)
                elif failure in ("symlink-eacces-without-control", "hardlink-eacces-without-control"):
                    self.assertIn("reason=eacces_without_allowed_control", result.stderr)

    @unittest.skipUnless(PWSH, "需要 PowerShell 解析场景 runner")
    def test_metadata_scenario_is_registered_in_powershell(self) -> None:
        source = (ROOT / ".github/tests/run-storage-redirect-scenarios.ps1").read_text(encoding="utf-8")
        self.assertIn("function Invoke-FileMutationMetadataScenario {", source)
        self.assertIn("Invoke-FileMutationMetadataScenario $Scenario", source)
        self.assertIn("hardlink boundary changed", source)
        with tempfile.TemporaryDirectory() as directory:
            parser = Path(directory) / "parse-runner.ps1"
            parser.write_text(
                "param([string]$SourcePath)\n"
                "$tokens=$null; $errors=$null; "
                "[System.Management.Automation.Language.Parser]::ParseFile($SourcePath, [ref]$tokens, [ref]$errors) > $null; "
                "if ($errors.Count) { $errors | ForEach-Object { Write-Error $_ }; exit 1 }\n",
                encoding="utf-8",
            )
            result = subprocess.run(
                [PWSH, "-NoProfile", "-File", str(parser), str(ROOT / ".github/tests/run-storage-redirect-scenarios.ps1")],
                text=True,
                capture_output=True,
                timeout=20,
            )
        self.assertEqual(0, result.returncode, result.stderr)
