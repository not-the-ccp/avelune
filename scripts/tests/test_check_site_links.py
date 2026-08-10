#!/usr/bin/env python3
"""Focused regression tests for base-path and fragment checking."""

from __future__ import annotations

from pathlib import Path
import subprocess
import tempfile
import unittest


CHECKER = Path(__file__).resolve().parents[1] / "check-site-links.py"


class SiteLinkCheckerTests(unittest.TestCase):
    def make_site(self, fragment: str = "target") -> Path:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        (root / "docs" / "guide").mkdir(parents=True)
        (root / "index.html").write_text(
            '<a id="home"></a><a href="/avelune/docs/guide/#target">guide</a><a href="#home">home</a>',
            encoding="utf-8",
        )
        (root / "docs" / "guide" / "index.html").write_text(f'<h1 id="{fragment}">Guide</h1>', encoding="utf-8")
        return root

    def run_checker(self, root: Path) -> subprocess.CompletedProcess[str]:
        return subprocess.run(["python3", str(CHECKER), str(root)], text=True, capture_output=True, check=False)

    def test_accepts_astro_base_path_and_existing_fragments(self) -> None:
        result = self.run_checker(self.make_site())
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_rejects_missing_destination_fragment(self) -> None:
        result = self.run_checker(self.make_site(fragment="different"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing local fragment", result.stderr)


if __name__ == "__main__":
    unittest.main()
