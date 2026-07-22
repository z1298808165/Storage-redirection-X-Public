import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]


class InlineTestValidatorTest(unittest.TestCase):
    def test_allowlist_only_scans_production_rust_sources(self) -> None:
        validator = (ROOT / "scripts/validate-inline-tests.ps1").read_text(
            encoding="utf-8"
        )
        self.assertIn('"src/*.rs", "src/**/*.rs"', validator)
        self.assertIn(
            'StartsWith("src/", [StringComparison]::Ordinal)', validator
        )


if __name__ == "__main__":
    unittest.main()
