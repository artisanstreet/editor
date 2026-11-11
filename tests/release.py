import datetime
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("release", Path(__file__).resolve().parents[1] / "src/release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


class Release(unittest.TestCase):
    def test_refs_and_nightly_ordering(self):
        sha = "a" * 40
        self.assertEqual(release.version_for("v1.2.3", sha, "20261111031700"), ("1.2.3", "v1.2.3"))
        self.assertEqual(release.version_for("master", sha, "20261111031700"), ("0.0.0-nightly.20261111031700+aaaaaaaaaaaa", "nightly"))
        for reference in ("v01.2.3", "v1.2.3-rc1", "feature/branch", "--help"):
            with self.assertRaises(ValueError):
                release.version_for(reference, sha, "20261111031700")

    def test_nightly_runs_at_oslo_midnight_in_summer_and_winter_time(self):
        utc = lambda month, hour: datetime.datetime(2026, month, 15, hour, 20, tzinfo=datetime.timezone.utc)
        self.assertTrue(release.midnight_in_oslo(utc(7, 22)))
        self.assertFalse(release.midnight_in_oslo(utc(7, 23)))
        self.assertFalse(release.midnight_in_oslo(utc(1, 22)))
        self.assertTrue(release.midnight_in_oslo(utc(1, 23)))

    def test_gate_rejects_pr_runs_and_newer_failed_or_pending_runs(self):
        sha = "a" * 40
        good = {"id": 1, "head_sha": sha, "event": "push", "head_branch": "master", "conclusion": "success"}
        self.assertTrue(release.ci_green([good], sha))
        for changes in ({"event": "pull_request"}, {"head_branch": "other"}, {"head_sha": "b" * 40}):
            self.assertFalse(release.ci_green([good | changes], sha))
        for conclusion in ("failure", "cancelled", None):
            self.assertFalse(release.ci_green([good, good | {"id": 2, "conclusion": conclusion}], sha))

    def test_key_normalizes_only_the_expected_der_or_raw_anchor(self):
        self.assertEqual(release.public_key(release.ANCHOR), release.ANCHOR)
        self.assertEqual(release.public_key("302a300506032b6570032100" + release.ANCHOR), release.ANCHOR)
        for key in ("", "00" * 32, "ff" + release.ANCHOR):
            with self.assertRaises(ValueError):
                release.public_key(key)

    def fixture(self, root):
        (root / "scripts").mkdir(parents=True)
        (root / "bin").mkdir()
        (root / "scripts/install.sh").write_text(f'feed="https://github.com/artisanstreet/editor/{release.LATEST}"\n')
        for binary in ("ae", "editor-aqua.exe"):
            (root / "bin" / binary).write_bytes(b"native fixture " + binary.encode())

    def test_packaging_assembly_and_signature_reject_corruption(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixture(root)
            for app, system, arch in release.EXPECTED:
                release.package(root / "bin", root / "input", app, system, arch)
            release.assemble(root, root / "input", root / "dist", "1.2.3", "a" * 40)
            manifest = json.loads((root / "dist/release.json").read_text())
            self.assertEqual(sorted(entry["file"] for entry in manifest["artifacts"]), ["Artisan-Editor-x86_64.exe", "ae-linux-aarch64", "ae-linux-x86_64", "ae-macos-aarch64"])
            self.assertEqual(len((root / "dist/release.json").read_text().splitlines()), 6)
            self.assertIn(release.LATEST, (root / "dist/install.sh").read_text())
            key = subprocess.check_output(["openssl", "genpkey", "-algorithm", "ED25519"], text=True)
            public = subprocess.check_output(["openssl", "pkey", "-pubout", "-outform", "DER"], input=key.encode())[-32:].hex()
            release.sign(root / "dist", key, public)
            release.verify(root / "dist", public, "a" * 40)
            with self.assertRaisesRegex(ValueError, "manifest commit"):
                release.verify(root / "dist", public, "b" * 40)
            with self.assertRaisesRegex(ValueError, "trust anchor"):
                release.sign(root / "dist", key, "00" * 32)
            self.assertTrue((root / "dist/release.json.sig").is_file())
            artifact = manifest["artifacts"][0]["file"]
            (root / "dist" / artifact).write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "changed before signing"):
                release.sign(root / "dist", key, public)
            (root / "input" / artifact).write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                release.assemble(root, root / "input", root / "broken", "1.2.3", "a" * 40)

    def test_a_release_lists_readable_names_and_takes_the_arm_editor_when_it_built(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixture(root)
            for app, system, arch in release.EXPECTED | release.OPTIONAL:
                release.package(root / "bin", root / "input", app, system, arch)
            release.assemble(root, root / "input", root / "dist", "0.0.0-nightly.20261111220000+aaaaaaaaaaaa", "a" * 40)
            listed = dict(line.rsplit("#", 1) for line in release.assets(root / "dist"))
            self.assertEqual(listed[str(root / "dist/Artisan-Editor-x86_64.exe")], "Artisan Editor for Windows (x86_64)")
            self.assertEqual(listed[str(root / "dist/Artisan-Editor-aarch64.exe")], "Artisan Editor for Windows (ARM64)")
            self.assertEqual(listed[str(root / "dist/ae-macos-aarch64")], "ae CLI for macOS (Apple Silicon)")
            self.assertEqual(listed[str(root / "dist/ae-linux-x86_64")], "ae CLI for Linux (x86_64)")
            self.assertEqual(len(listed), 8)
            for path in listed:
                self.assertTrue(path.endswith("release.json.sig") or Path(path).is_file(), path)
            installer = (root / "dist/install.sh").read_text()
            self.assertIn(release.NIGHTLY, installer)
            self.assertNotIn(release.LATEST, installer)

    def test_assembly_rejects_missing_and_duplicate_platforms(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixture(root)
            entry = release.package(root / "bin", root / "input", "ae", "linux", "x86_64")
            with self.assertRaisesRegex(ValueError, "missing platform"):
                release.assemble(root, root / "input", root / "missing", "1.2.3", "a" * 40)
            (root / "input/duplicate.json").write_text(json.dumps(entry))
            with self.assertRaisesRegex(ValueError, "duplicate"):
                release.assemble(root, root / "input", root / "duplicate", "1.2.3", "a" * 40)

    def test_version_stamp_updates_workspace_packages_only(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "crates/ae").mkdir(parents=True)
            (root / "crates/ae/Cargo.toml").write_text('[package]\nname = "ae"\nversion.workspace = true\n')
            (root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/ae"]\n[workspace.package]\nversion = "0.0.0"\n')
            (root / "Cargo.lock").write_text('version = 4\n[[package]]\nname = "ae"\nversion = "0.0.0"\n[[package]]\nname = "dependency"\nversion = "1.0.0"\nsource = "registry"\n')
            release.stamp(root, "1.2.3")
            self.assertIn('version = "1.2.3"', (root / "Cargo.toml").read_text())
            self.assertIn('name = "ae"\nversion = "1.2.3"', (root / "Cargo.lock").read_text())
            self.assertIn('name = "dependency"\nversion = "1.0.0"', (root / "Cargo.lock").read_text())


if __name__ == "__main__":
    unittest.main()
