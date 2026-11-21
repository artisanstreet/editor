import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

VERIFY = Path(__file__).resolve().parents[3] / "vendor/highlighting/verify.py"
PACKAGE_FILE = b"pub fn search() {}\n"


def vendored_tree(repository, inputs):
    vendor = repository / "vendor/highlighting"
    (vendor / "syntect/src").mkdir(parents=True)
    (vendor / "syntect/src/lib.rs").write_bytes(PACKAGE_FILE)
    shutil.copyfile(VERIFY, vendor / "verify.py")
    provenance = {
        "retained_files": [
            {"path": "syntect/src/lib.rs", "sha256": hashlib.sha256(PACKAGE_FILE).hexdigest()}
        ],
        "integration_inputs": inputs,
    }
    (vendor / "PROVENANCE.json").write_text(json.dumps(provenance))
    return vendor / "verify.py"


def run(verify):
    return subprocess.run([sys.executable, "-I", str(verify)], capture_output=True, text=True)


class Verify(unittest.TestCase):
    def setUp(self):
        self.repository = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.repository)
        (self.repository / "Cargo.toml").write_text('syntect = { path = "vendor/highlighting/syntect" }\n')

    def test_inputs_that_still_wire_the_packages_pass(self):
        verify = vendored_tree(
            self.repository,
            [{"path": "Cargo.toml", "wires": "vendor/highlighting/syntect"}],
        )
        result = run(verify)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_a_deleted_integration_input_fails(self):
        verify = vendored_tree(
            self.repository,
            [
                {"path": "Cargo.toml", "wires": "vendor/highlighting/syntect"},
                {"path": "nix/release.nix", "wires": "vendor/highlighting/NOTICE.txt"},
            ],
        )
        result = run(verify)
        self.assertEqual(result.returncode, 1)
        self.assertIn("Integration input is missing: nix/release.nix", result.stdout)

    def test_an_input_that_stopped_wiring_the_packages_fails(self):
        verify = vendored_tree(
            self.repository,
            [{"path": "Cargo.toml", "wires": "vendor/highlighting/onig_sys"}],
        )
        result = run(verify)
        self.assertEqual(result.returncode, 1)
        self.assertIn("Cargo.toml lacks vendor/highlighting/onig_sys", result.stdout)

    def test_a_changed_package_file_fails(self):
        verify = vendored_tree(
            self.repository,
            [{"path": "Cargo.toml", "wires": "vendor/highlighting/syntect"}],
        )
        (verify.parent / "syntect/src/lib.rs").write_bytes(b"pub fn changed() {}\n")
        result = run(verify)
        self.assertEqual(result.returncode, 1)
        self.assertIn("Checksum or file inventory mismatch: syntect/src/lib.rs", result.stdout)


if __name__ == "__main__":
    unittest.main()
