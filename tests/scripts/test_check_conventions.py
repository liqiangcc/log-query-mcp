from __future__ import annotations

from pathlib import Path
import tempfile
import unittest

from scripts.check_conventions import collect_violations


VALID_INITIALIZE = """# @name initializeMcp
# @tags smoke deployment production-safe

POST {{$processEnv BASE_URL}}/mcp
Content-Type: application/json

{}

?? status == 200
"""


def write_case(root: Path, relative: str, content: str) -> Path:
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    return path


class ConventionCheckerTest(unittest.TestCase):
    def make_root(self) -> tuple[tempfile.TemporaryDirectory[str], Path]:
        temp = tempfile.TemporaryDirectory()
        root = Path(temp.name)
        return temp, root

    def seed_valid_initialize(self, root: Path) -> None:
        write_case(root, "tests/http/mcp/initialize.http", VALID_INITIALIZE)

    def rules(self, root: Path) -> list[str]:
        violations, _ = collect_violations(root)
        return [violation.rule for violation in violations]

    def test_valid_http_asset_passes(self) -> None:
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.seed_valid_initialize(root)

        violations, checked = collect_violations(root)

        self.assertEqual([], violations)
        self.assertEqual(1, checked)

    def test_missing_initialize_is_reported(self) -> None:
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)

        self.assertIn("LQM_HTTP001", self.rules(root))

    def test_duplicate_name_is_reported(self) -> None:
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        self.seed_valid_initialize(root)
        write_case(
            root,
            "tests/http/mcp/duplicate.http",
            VALID_INITIALIZE.replace("POST {{$processEnv BASE_URL}}/mcp", "GET {{$processEnv BASE_URL}}/mcp"),
        )

        self.assertIn("LQM_HTTP002", self.rules(root))

    def test_production_safe_destructive_conflict_is_reported(self) -> None:
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        write_case(
            root,
            "tests/http/mcp/initialize.http",
            VALID_INITIALIZE.replace(
                "# @tags smoke deployment production-safe",
                "# @tags smoke deployment production-safe destructive",
            ),
        )

        self.assertIn("LQM_HTTP003", self.rules(root))

    def test_hard_coded_base_url_is_reported(self) -> None:
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        write_case(
            root,
            "tests/http/mcp/initialize.http",
            VALID_INITIALIZE.replace("{{$processEnv BASE_URL}}", "http://127.0.0.1:8000"),
        )

        self.assertIn("LQM_HTTP004", self.rules(root))

    def test_missing_status_assertion_is_reported(self) -> None:
        temp, root = self.make_root()
        self.addCleanup(temp.cleanup)
        write_case(
            root,
            "tests/http/mcp/initialize.http",
            VALID_INITIALIZE.replace("?? status == 200\n", ""),
        )

        self.assertIn("LQM_HTTP005", self.rules(root))


if __name__ == "__main__":
    unittest.main()
