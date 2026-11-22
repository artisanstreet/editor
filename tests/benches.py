import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location("benches", Path(__file__).resolve().parents[1] / "src/benches.py")
benches = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benches)

OUTPUT = """Timer precision: 10 ns
recall                                 fastest       │ slowest       │ median        │ mean          │ samples │ iters
├─ grep_pages_through_one_long_output  5.455 ms      │ 11.15 ms      │ 7.337 ms      │ 7.522 ms      │ 20      │ 20
├─ sizes                                             │               │               │               │         │
│  ├─ 10                               900 ns        │ 1.2 µs        │ 950 ns        │ 1 µs          │ 50      │ 50
│  ╰─ 1000                             1.75 ms       │ 15.65 ms      │ 2.243 ms      │ 3.53 ms       │ 10      │ 10
╰─ reindex_stored_plan                 17.58 ms      │ 33.75 ms      │ 24.13 s       │ 23.96 ms      │ 20      │ 20
"""


class Benches(unittest.TestCase):
    def test_medians_follow_the_tree_and_convert_every_unit_to_nanoseconds(self):
        self.assertEqual(
            benches.medians(OUTPUT),
            {
                "recall/grep_pages_through_one_long_output": 7.337e6,
                "recall/sizes/10": 950.0,
                "recall/sizes/1000": 2.243e6,
                "recall/reindex_stored_plan": 24.13e9,
            },
        )

    def test_only_gross_regressions_and_unmatched_benchmarks_fail(self):
        baseline = {"a": 100.0, "b": 100.0, "gone": 5.0}
        measured = {"a": 299.0, "b": 301.0, "new": 1.0}
        rows, failures = benches.judge(baseline, measured)
        self.assertEqual([name for name, *_ in rows], ["a", "b"])
        self.assertEqual(len(failures), 3)
        self.assertTrue(failures[0].startswith("b took 3.0x"))
        self.assertIn("gone has a baseline but did not run", failures)
        self.assertIn("new has no baseline; run python3 src/benches.py --write", failures)

    def test_an_unknown_unit_is_rejected_instead_of_misread(self):
        with self.assertRaises(ValueError):
            benches.medians("g\n╰─ x  1 fortnights │ 2 ms │ 3 parsecs │ 4 ms │ 1 │ 1\n")


if __name__ == "__main__":
    unittest.main()
