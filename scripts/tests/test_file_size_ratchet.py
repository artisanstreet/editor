"""The update operation must never forgive growth, including a new large file."""
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'file_size_ratchet.py'


class RatchetTest(unittest.TestCase):
    def run_case(self, frozen, current, update=False):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'modules').mkdir()
            (root / 'scripts').mkdir()
            baseline = root / 'scripts/file-size-allowlist.txt'
            baseline.write_text(frozen)
            (root / 'modules/example.rs').write_text('line\n' * current)
            result = subprocess.run([sys.executable, str(SCRIPT), '--root', str(root),
                                     '--limit', '3'] + (['--update'] if update else []),
                                    capture_output=True, text=True)
            return result.returncode, baseline.read_text()

    def test_update_refuses_growth_and_preserves_allowlist(self):
        baseline = '4 modules/example.rs\n'
        self.assertEqual(self.run_case(baseline, 5, True), (1, baseline))

    def test_update_refuses_new_large_file(self):
        self.assertEqual(self.run_case('', 4, True), (1, ''))

    def test_update_shrinks(self):
        self.assertEqual(self.run_case('6 modules/example.rs\n', 4, True),
                         (0, '4 modules/example.rs\n'))

    def test_update_removes_small_files(self):
        self.assertEqual(self.run_case('4 modules/example.rs\n', 3, True), (0, ''))

    def test_check_does_not_rewrite_baseline(self):
        baseline = '6 modules/example.rs\n'
        self.assertEqual(self.run_case(baseline, 4), (0, baseline))
