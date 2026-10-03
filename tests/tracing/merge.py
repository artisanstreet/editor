"""Regression checks for merging overlapping flight-recorder captures."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('artisan_trace', Path(__file__).resolve().parents[2] / 'scripts/trace.py')
trace = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace)


class MergeTests(unittest.TestCase):
    def merge(self, captures):
        with tempfile.TemporaryDirectory() as directory:
            paths = []
            for index, capture in enumerate(captures):
                path = Path(directory) / f'{index}.json'
                path.write_text(json.dumps(capture))
                paths.append(path)
            output = Path(directory) / 'merged.json'
            with contextlib.redirect_stdout(io.StringIO()):
                trace.merge(output, paths)
            return trace.read(output)['traceEvents']

    @staticmethod
    def capture(events, role='editor', session=1):
        return {'metadata': {'role': role, 'session_start_us': session}, 'traceEvents': events}

    @staticmethod
    def boundary(phase, ts, unfinished=False):
        return {'pid': 7, 'tid': 1, 'cat': 'navigation', 'name': 'open', 'id': '1',
                'ph': phase, 'ts': ts, 'args': {'unfinished': True} if unfinished else {}}

    def test_real_end_supersedes_every_snapshot_and_begin_is_deduplicated(self):
        begin = self.boundary('b', 10)
        events = self.merge([
            self.capture([begin, self.boundary('e', 30, True)]),
            self.capture([begin, self.boundary('e', 50)]),
            self.capture([begin, self.boundary('e', 70, True)]),
        ])
        self.assertEqual(len(events), 2)
        self.assertEqual(trace.durations(events)[0][0], 40)
        self.assertFalse(trace.durations(events)[0][2])

    def test_latest_unfinished_end_and_earliest_begin_survive(self):
        events = self.merge([
            self.capture([self.boundary('b', 10), self.boundary('e', 30, True)]),
            self.capture([self.boundary('b', 20), self.boundary('e', 50, True)]),
        ])
        self.assertEqual(trace.durations(events)[0][0], 40)
        self.assertTrue(trace.durations(events)[0][2])

    def test_roles_and_process_restarts_have_distinct_process_and_flow_ids(self):
        endpoint = {'pid': 7, 'tid': 1, 'ph': 'X', 'name': 'handoff', 'ts': 10,
                    'dur': 1, 'bind_id': 'a', 'flow_out': True}
        events = self.merge([
            self.capture([endpoint]), self.capture([endpoint]),
            self.capture([endpoint], role='forge'),
            self.capture([endpoint], session=2),
        ])
        self.assertEqual(len(events), 3)
        self.assertEqual(len({e['pid'] for e in events}), 3)
        self.assertEqual(len({e['bind_id'] for e in events}), 3)
        for event in events:
            int(event['bind_id'], 16)


if __name__ == '__main__':
    unittest.main()
