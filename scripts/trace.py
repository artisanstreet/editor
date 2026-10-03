#!/usr/bin/env python3
"""Inspect Artisan flight recordings or merge Editor and Forge for Perfetto."""
import argparse
import json
from pathlib import Path


def read(path):
    data = json.loads(Path(path).read_text())
    if not isinstance(data.get('traceEvents'), list):
        raise ValueError(f'{path}: missing traceEvents array')
    return data


def durations(events):
    active = {}
    result = []
    for event in sorted(events, key=lambda e: e.get('ts', 0)):
        phase = event.get('ph')
        key = (event.get('pid'), event.get('cat'), event.get('name'), event.get('id'))
        if phase == 'b':
            active[key] = event
        elif phase == 'e' and key in active:
            begin = active.pop(key)
            result.append((event['ts'] - begin['ts'], begin,
                           event.get('args', {}).get('unfinished', False)))
        elif phase == 'X':
            result.append((event.get('dur', 0), event, False))
    return sorted(result, key=lambda row: row[0], reverse=True)


def summary(path):
    data = read(path)
    metadata = data.get('metadata', {})
    print(f"{metadata.get('role', 'unknown')} capture: {metadata.get('reason', 'unknown')}")
    print(f"Dropped: {metadata.get('dropped_records', 0)}; evicted: {metadata.get('evicted_records', 0)}")
    print('Longest operations:')
    for duration, event, unfinished in durations(data['traceEvents'])[:25]:
        args = event.get('args', {})
        context = ' '.join(f'{key}={args[key]}' for key in
                           ('command', 'event', 'expected', 'thread_id', 'request_id', 'generation')
                           if args.get(key) is not None)
        status = ' [unfinished at capture]' if unfinished else ''
        print(f"  {duration / 1000:10.2f} ms  {event.get('cat')}/{event.get('name')}{status} {context}")
    gates = [e for e in data['traceEvents'] if e.get('name') == 'navigation.gates']
    if gates:
        print('Last navigation gates:')
        print(json.dumps(max(gates, key=lambda e: e['ts']).get('args'), indent=2))


def merge(output, paths):
    events, processes, seen, boundaries, bindings = [], {}, set(), {}, {}
    for path in paths:
        data = read(path)
        metadata = data.get('metadata', {})
        role = metadata.get('role', str(path))
        for original in data['traceEvents']:
            event = dict(original)
            # Windows Editor and Linux Forge can have the same OS PID.
            identity = (role, event.get('pid', 0), metadata.get('session_start_us'))
            event['pid'] = processes.setdefault(identity, len(processes) + 1)
            if 'bind_id' in event:
                binding = (event['pid'], event['bind_id'])
                event['bind_id'] = format(bindings.setdefault(binding, len(bindings) + 1), 'x')
            if event.get('ph') in ('b', 'e'):
                key = (event['pid'], event.get('cat'), event.get('name'),
                       event.get('id'), event['ph'])
                previous = boundaries.get(key)
                if previous is None:
                    boundaries[key] = event
                elif event['ph'] == 'b':
                    boundaries[key] = min(previous, event, key=lambda e: e['ts'])
                else:
                    # A snapshot closes live spans synthetically. A later
                    # real completion supersedes every snapshot boundary.
                    boundaries[key] = max(previous, event, key=lambda e: (
                        not e.get('args', {}).get('unfinished', False), e['ts']))
                continue
            encoded = json.dumps(event, sort_keys=True)
            if encoded not in seen:
                seen.add(encoded)
                events.append(event)
    events.extend(boundaries.values())
    events.sort(key=lambda e: e.get('ts', 0))
    Path(output).write_text(json.dumps({'traceEvents': events, 'displayTimeUnit': 'ms',
                                      'metadata': {'sources': [str(p) for p in paths]}}))
    print(f'Saved {len(events)} events to {output}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    inspect = commands.add_parser('summary')
    inspect.add_argument('trace', type=Path)
    combine = commands.add_parser('merge')
    combine.add_argument('output', type=Path)
    combine.add_argument('traces', nargs='+', type=Path)
    args = parser.parse_args()
    if args.command == 'summary':
        summary(args.trace)
    else:
        merge(args.output, args.traces)


if __name__ == '__main__':
    main()
