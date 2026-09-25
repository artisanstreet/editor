#!/usr/bin/env python3
"""Capture and sanitize Claude Code stream-json turns for thinking fixtures.

`capture` drives one finite turn with Artisan's managed argv and stdin user
line (see `modules/backend/src/engine_owner/claude/launch.rs` and
`protocol.rs`) inside a scratch project and records every stdout frame with
its receipt time. Prompts must have no external effects; the scratch project
holds only the files it writes itself. Captures bill the signed-in account.

`sanitize` turns one raw capture into a committed fixture: signatures,
signature deltas, and tool-input deltas are emptied; session ids, uuids, and
message/tool/request ids become stable placeholders; local paths are
rewritten; environment inventories and hook output are emptied. Field
presence and frame order are retained.
"""
import argparse
import json
import re
import subprocess
import time
import uuid
from pathlib import Path

ARGV = ['claude', '-p', '--output-format', 'stream-json', '--input-format', 'stream-json',
        '--verbose', '--include-partial-messages', '--forward-subagent-text',
        '--permission-prompt-tool', 'stdio', '--dangerously-skip-permissions']
UUID = re.compile(r'^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$')
PATHISH = re.compile(r'(wsl\.localhost|\\\\|[A-Za-z]:\\|/home/|/tmp/|/mnt/|pipe\\)')
INVENTORY = {'tools', 'mcp_servers', 'slash_commands', 'terminal_slash_commands', 'agents',
             'skills', 'plugins', 'capabilities'}
PLACEHOLDERS = [('msg_', 'msg', 'msg_fixture_{:02d}'), ('toolu_', 'tool', 'toolu_fixture_{:02d}'),
                ('req_', 'req', 'req_fixture_{:02d}')]


def capture(args):
    project = Path(args.scratch) / 'project'
    project.mkdir(parents=True, exist_ok=True)
    (project / 'numbers.txt').write_text('alpha 17\nbeta 42\ngamma 9\ndelta 31\n')
    session = args.session or str(uuid.uuid4())
    argv = [*ARGV, '--effort', args.effort]
    if args.display:
        argv += ['--thinking-display', args.display]
    argv += ['--resume' if args.resume else '--session-id', session, '--model', args.model]
    line = json.dumps({'message': {'content': [{'type': 'text', 'text': args.prompt}], 'role': 'user'},
                       'parent_tool_use_id': None, 'session_id': session, 'type': 'user'})
    started = time.time()
    child = subprocess.Popen(argv, cwd=project, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=subprocess.PIPE, text=True)
    child.stdin.write(line + '\n')
    child.stdin.flush()
    frames = []
    for raw in child.stdout:
        if raw.strip():
            frame = json.loads(raw)
            frames.append({'t_ms': int((time.time() - started) * 1000), 'frame': frame})
            if frame.get('type') == 'result':
                break
    child.stdin.close()
    child.wait(timeout=60)
    Path(args.out).write_text(''.join(json.dumps(record) + '\n' for record in frames))
    print(json.dumps({'out': args.out, 'session': session, 'frames': len(frames), 'exit': child.returncode}))


class Mapper:
    def __init__(self, session):
        self.session = session
        self.tables = {}

    def placeholder(self, kind, value, pattern):
        table = self.tables.setdefault(kind, {})
        return table.setdefault(value, pattern.format(len(table) + 1))

    def string(self, value, key):
        if key == 'session_id':
            return self.session
        if UUID.match(value):
            return self.placeholder('uuid', value, '00000000-0000-4000-8000-{:012d}')
        for prefix, kind, pattern in PLACEHOLDERS:
            if value.startswith(prefix):
                return self.placeholder(kind, value, pattern)
        if PATHISH.search(value):
            name = re.split(r'[\\/]', value.rstrip('\\/'))[-1]
            return '/project/' + name if key in ('file_path', 'filePath') else '/redacted'
        return value

    def scrub(self, value, key=None):
        if isinstance(value, dict):
            out = {}
            for name, item in value.items():
                if name == 'signature':
                    out[name] = ''
                elif name in INVENTORY and isinstance(item, list):
                    out[name] = []
                elif name in ('output', 'stdout', 'stderr') and isinstance(item, str):
                    out[name] = ''
                else:
                    out[self.string(name, None)] = self.scrub(item, name)
            return out
        if isinstance(value, list):
            return [self.scrub(item, key) for item in value]
        return self.string(value, key) if isinstance(value, str) else value


def sanitize(args):
    mapper = Mapper(args.session)
    lines = []
    for raw in Path(args.raw).read_text().splitlines():
        if not raw.strip():
            continue
        frame = json.loads(raw)['frame']
        delta = (frame.get('event') or {}).get('delta') or {}
        if delta.get('type') == 'input_json_delta':
            delta['partial_json'] = ''
        lines.append(json.dumps(mapper.scrub(frame), ensure_ascii=False) + '\n')
    Path(args.out).write_text(''.join(lines))
    print(json.dumps({'out': args.out, 'frames': len(lines)}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    run = commands.add_parser('capture', help='record one finite turn')
    run.add_argument('--out', required=True)
    run.add_argument('--prompt', required=True)
    run.add_argument('--scratch', default='/tmp/claude-thinking-capture')
    run.add_argument('--session', help='existing session id (required with --resume)')
    run.add_argument('--resume', action='store_true')
    run.add_argument('--display', default='summarized', help="thinking display; '' omits the flag")
    run.add_argument('--model', default='claude-sonnet-5')
    run.add_argument('--effort', default='high')
    run.set_defaults(handler=capture)
    clean = commands.add_parser('sanitize', help='turn one raw capture into a fixture')
    clean.add_argument('raw')
    clean.add_argument('out')
    clean.add_argument('--session', required=True, help='placeholder session id for the fixture')
    clean.set_defaults(handler=sanitize)
    parsed = parser.parse_args()
    parsed.handler(parsed)
