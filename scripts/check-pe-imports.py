#!/usr/bin/env python3
"""Find nonzero PE32+ IAT slots that no import descriptor asks Windows to bind."""
from pathlib import Path
import argparse
import hashlib
import json
import struct


def inspect(path):
    data = path.read_bytes()
    pe = struct.unpack_from('<I', data, 0x3c)[0]
    if data[pe:pe + 4] != b'PE\0\0':
        raise ValueError('Not a PE executable')
    section_count = struct.unpack_from('<H', data, pe + 6)[0]
    optional_size = struct.unpack_from('<H', data, pe + 20)[0]
    optional = pe + 24
    if struct.unpack_from('<H', data, optional)[0] != 0x20b:
        raise ValueError('Only PE32+ images are supported')
    sections = []
    for index in range(section_count):
        at = optional + optional_size + index * 40
        virtual_size, address, raw_size, raw_offset = struct.unpack_from('<IIII', data, at + 8)
        sections.append((address, raw_size, raw_offset))

    def offset(rva):
        for address, size, raw in sections:
            if address <= rva < address + size:
                return raw + rva - address
        raise ValueError(f'RVA {rva:#x} has no file contents')

    def string(rva):
        at = offset(rva)
        end = data.index(0, at)
        return data[at:end].decode('ascii')

    def name(value):
        if value & (1 << 63):
            return f'ordinal {value & 0xffff}'
        return string(value + 2)

    imports_rva, imports_size = struct.unpack_from('<II', data, optional + 112 + 8)
    iat_rva, iat_size = struct.unpack_from('<II', data, optional + 112 + 12 * 8)
    covered = set()
    descriptors = []
    for index in range(imports_size // 20):
        original, stamp, chain, library, first = struct.unpack_from('<IIIII', data, offset(imports_rva + index * 20))
        if (original, stamp, chain, library, first) == (0, 0, 0, 0, 0):
            break
        names = []
        position = 0
        while True:
            value = struct.unpack_from('<Q', data, offset((original or first) + position * 8))[0]
            if value == 0:
                break
            covered.add(first + position * 8)
            names.append(name(value))
            position += 1
        descriptors.append({'library': string(library), 'first_thunk': hex(first), 'functions': names})
    uncovered = []
    for relative in range(0, iat_size, 8):
        address = iat_rva + relative
        value = struct.unpack_from('<Q', data, offset(address))[0]
        if value and address not in covered:
            uncovered.append({'iat_slot_rva': hex(address), 'unbound_value': hex(value), 'function': name(value)})
    return {'binary': str(path), 'sha256': hashlib.sha256(data).hexdigest(), 'iat_rva': hex(iat_rva),
            'iat_bytes': iat_size, 'import_descriptors': descriptors, 'uncovered_iat_entries': uncovered,
            'valid': not uncovered, 'scope': 'Static PE32+ import descriptor coverage; does not validate DLL availability or function semantics'}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path, nargs='+')
    parser.add_argument('--output', type=Path)
    arguments = parser.parse_args()
    results = [inspect(path) for path in arguments.binary]
    if arguments.output:
        arguments.output.write_text(json.dumps(results, indent=2) + '\n')
    for result in results:
        print(result['binary'], 'PASS' if result['valid'] else 'FAIL', result['uncovered_iat_entries'])
    raise SystemExit(0 if all(result['valid'] for result in results) else 1)
