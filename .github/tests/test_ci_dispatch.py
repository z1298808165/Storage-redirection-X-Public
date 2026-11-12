"""验证 fork 仅在手动派发时进入 CI，维护提交仍跳过自动构建。"""

from pathlib import Path
import unittest

import yaml


class CiDispatchTests(unittest.TestCase):
    def test_entry_gates(self):
        workflow = Path(__file__).resolve().parents[1] / "workflows/ci.yml"
        jobs = yaml.safe_load(workflow.read_text(encoding="utf-8"))["jobs"]
        for repository, event, message, expected in (
            ("example/fork", "workflow_dispatch", "CI：更新更新清单", True),
            ("example/fork", "push", "fix", False),
            ("example/fork", "pull_request", "fix", False),
            ("z1298808165/Storage-redirection-X-Public", "push", "fix", True),
            ("z1298808165/Storage-redirection-X-Public", "push", "CI：更新更新清单", False),
        ):
            for job in ("quality", "prepare"):
                expression = jobs[job]["if"]
                for name, value in (("github.repository", repository), ("github.event_name", event),
                                    ("github.event.head_commit.message", message)):
                    expression = expression.replace(name, repr(value))
                expression = expression.replace("&&", " and ").replace("||", " or ").replace("!startsWith", "not startsWith")
                with self.subTest(repository=repository, event=event, job=job):
                    self.assertEqual(expected, eval(expression, {"__builtins__": {}, "startsWith": str.startswith}))


if __name__ == "__main__":
    unittest.main()
