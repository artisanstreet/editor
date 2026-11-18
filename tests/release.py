import datetime
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("release", Path(__file__).resolve().parents[1] / "src/release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)
INSTALLER = Path(__file__).resolve().parents[1] / "scripts/install.sh"
FIXTURE_AE = b'#!/bin/sh\nprintf "%s\\n" "$*" > "$ARTISAN_TEST_MARKER"\n'
TOOLS = ("uname", "mktemp", "rm", "cp", "grep", "head", "sed", "chmod", "sha256sum", "cut", "tr", "cat")


def throwaway_key():
    key = subprocess.check_output(["openssl", "genpkey", "-algorithm", "ED25519"], text=True)
    public = subprocess.check_output(["openssl", "pkey", "-pubout", "-outform", "DER"], input=key.encode())[-32:].hex()
    return key, public


class Release(unittest.TestCase):
    def test_refs_and_nightly_ordering(self):
        sha = "a" * 40
        self.assertEqual(release.version_for("v1.2.3", sha, "20261111031700"), ("1.2.3", "v1.2.3"))
        self.assertEqual(release.version_for("master", sha, "20261111031700"), ("0.0.0-nightly.20261111031700+aaaaaaaaaaaa", "nightly"))
        for reference in ("v01.2.3", "v1.2.3-rc1", "feature/branch", "--help"):
            with self.assertRaises(ValueError):
                release.version_for(reference, sha, "20261111031700")

    def test_nightly_runs_after_oslo_midnight_however_late_the_schedule_fires(self):
        utc = lambda month, hour: datetime.datetime(2026, month, 15, hour, 20, tzinfo=datetime.timezone.utc)
        self.assertTrue(release.night_in_oslo(utc(7, 22)))
        self.assertFalse(release.night_in_oslo(utc(7, 21)))
        self.assertFalse(release.night_in_oslo(utc(1, 22)))
        self.assertTrue(release.night_in_oslo(utc(1, 23)))
        self.assertTrue(release.night_in_oslo(utc(1, 3)))
        self.assertFalse(release.night_in_oslo(utc(1, 5)))

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

    def fixture(self, root, anchor=release.ANCHOR):
        (root / "scripts").mkdir(parents=True)
        (root / "bin").mkdir()
        installer = INSTALLER.read_text()
        installer = installer.replace(release.allowed_signer(release.ANCHOR), release.allowed_signer(anchor))
        installer = installer.replace(release.pem_public(release.ANCHOR), release.pem_public(anchor))
        (root / "scripts/install.sh").write_text(installer)
        (root / "bin/ae").write_bytes(FIXTURE_AE)
        (root / "bin/editor-aqua.exe").write_bytes(b"native fixture editor")

    def signed_release(self, root):
        key, public = throwaway_key()
        self.fixture(root, public)
        for app, system, arch in release.EXPECTED:
            release.package(root / "bin", root / "input", app, system, arch)
        release.assemble(root, root / "input", root / "dist", "1.2.3", "a" * 40, public)
        release.sign(root / "dist", key, public)
        return key, public

    def bootstrap(self, root, verifiers):
        tools = root / "tools"
        tools.mkdir(exist_ok=True)
        for tool in TOOLS + verifiers:
            os.symlink(shutil.which(tool), tools / tool)
        marker = root / "ran"
        marker.unlink(missing_ok=True)
        environment = {"PATH": str(tools), "ARTISAN_RELEASES": str(root / "dist"), "ARTISAN_TEST_MARKER": str(marker), "HOME": str(root)}
        result = subprocess.run(["/bin/sh", str(root / "dist/install.sh"), "--yes"], env=environment, capture_output=True, text=True, stdin=subprocess.DEVNULL)
        shutil.rmtree(tools)
        return result, marker.read_text() if marker.exists() else None

    def tamper(self, root):
        manifest = json.loads((root / "dist/release.json").read_text())
        evil = b"#!/bin/sh\necho evil > \"$ARTISAN_TEST_MARKER\"\n"
        text = (root / "dist/release.json").read_text()
        for entry in manifest["artifacts"]:
            if entry["app"] == "ae":
                (root / "dist" / entry["file"]).write_bytes(evil)
                text = text.replace(entry["sha256"], release.digest(root / "dist" / entry["file"]))
        (root / "dist/release.json").write_text(text)

    def test_packaging_assembly_and_signature_reject_corruption(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixture(root)
            for app, system, arch in release.EXPECTED:
                release.package(root / "bin", root / "input", app, system, arch)
            key, public = throwaway_key()
            with self.assertRaisesRegex(ValueError, "trust anchor"):
                release.assemble(root, root / "input", root / "unpinned", "1.2.3", "a" * 40, public)
            release.assemble(root, root / "input", root / "dist", "1.2.3", "a" * 40, release.ANCHOR)
            manifest = json.loads((root / "dist/release.json").read_text())
            self.assertEqual(sorted(entry["file"] for entry in manifest["artifacts"]), ["Artisan-Editor-x86_64.exe", "ae-linux-aarch64", "ae-linux-x86_64", "ae-macos-aarch64"])
            self.assertEqual(len((root / "dist/release.json").read_text().splitlines()), 6)
            self.assertIn(release.LATEST, (root / "dist/install.sh").read_text())
            with self.assertRaisesRegex(ValueError, "trust anchor"):
                release.sign(root / "dist", key, "00" * 32)
            release.sign(root / "dist", key, public)
            release.verify(root / "dist", public, "a" * 40)
            with self.assertRaisesRegex(ValueError, "manifest commit"):
                release.verify(root / "dist", public, "b" * 40)
            self.assertTrue((root / "dist/release.json.sig").is_file())
            artifact = manifest["artifacts"][0]["file"]
            (root / "dist" / artifact).write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "changed before signing"):
                release.sign(root / "dist", key, public)
            (root / "input" / artifact).write_bytes(b"tampered")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                release.assemble(root, root / "input", root / "broken", "1.2.3", "a" * 40, release.ANCHOR)

    def test_a_release_lists_readable_names_and_takes_the_arm_editor_when_it_built(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixture(root)
            for app, system, arch in release.EXPECTED | release.OPTIONAL:
                release.package(root / "bin", root / "input", app, system, arch)
            release.assemble(root, root / "input", root / "dist", "0.0.0-nightly.20261111220000+aaaaaaaaaaaa", "a" * 40, release.ANCHOR)
            listed = dict(line.rsplit("#", 1) for line in release.assets(root / "dist"))
            self.assertEqual(listed[str(root / "dist/Artisan-Editor-x86_64.exe")], "Artisan Editor for Windows (x86_64)")
            self.assertEqual(listed[str(root / "dist/Artisan-Editor-aarch64.exe")], "Artisan Editor for Windows (ARM64)")
            self.assertEqual(listed[str(root / "dist/ae-macos-aarch64")], "ae CLI for macOS (Apple Silicon)")
            self.assertEqual(listed[str(root / "dist/ae-linux-x86_64")], "ae CLI for Linux (x86_64)")
            self.assertEqual(len(listed), 9)
            for path in listed:
                self.assertTrue(path.endswith((".sig", ".sshsig")) or Path(path).is_file(), path)
            installer = (root / "dist/install.sh").read_text()
            self.assertIn(release.NIGHTLY, installer)
            self.assertNotIn(release.LATEST, installer)

    def test_assembly_rejects_missing_and_duplicate_platforms(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixture(root)
            entry = release.package(root / "bin", root / "input", "ae", "linux", "x86_64")
            with self.assertRaisesRegex(ValueError, "missing platform"):
                release.assemble(root, root / "input", root / "missing", "1.2.3", "a" * 40, release.ANCHOR)
            (root / "input/duplicate.json").write_text(json.dumps(entry))
            with self.assertRaisesRegex(ValueError, "duplicate"):
                release.assemble(root, root / "input", root / "duplicate", "1.2.3", "a" * 40, release.ANCHOR)

    def test_verification_rejects_a_signed_manifest_changed_after_signing(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            _, public = self.signed_release(root)
            release.verify(root / "dist", public, "a" * 40)
            self.tamper(root)
            with self.assertRaisesRegex(ValueError, "not signed"):
                release.verify(root / "dist", public, "a" * 40)

    def test_verification_rejects_altered_signature_bytes(self):
        for name in ("release.json.sig", "release.json.sshsig"):
            with self.subTest(name), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                _, public = self.signed_release(root)
                signature = root / "dist" / name
                text = signature.read_text()
                index = text.index("=") - 8 if name.endswith(".sig") else text.index("\n-----END") - 8
                signature.write_text(text[:index] + ("A" if text[index] != "A" else "B") + text[index + 1:])
                with self.assertRaisesRegex(ValueError, "not signed"):
                    release.verify(root / "dist", public, "a" * 40)
                with self.assertRaisesRegex(ValueError, "not signed"):
                    release.verify(root / "dist", throwaway_key()[1], "a" * 40)

    def test_bootstrap_runs_ae_only_from_a_release_signed_by_the_pinned_key(self):
        for verifiers in (("ssh-keygen",), ("openssl",)):
            with self.subTest(verifiers), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                self.signed_release(root)
                result, ran = self.bootstrap(root, verifiers)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(ran, f"install --feed {root / 'dist'} --yes\n")
                self.tamper(root)
                result, ran = self.bootstrap(root, verifiers)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("not signed by Artisan's release key", result.stderr)
                self.assertIsNone(ran)

    def test_bootstrap_rejects_a_release_signed_by_another_key(self):
        for verifiers in (("ssh-keygen",), ("openssl",)):
            with self.subTest(verifiers), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                self.signed_release(root)
                key, public = throwaway_key()
                (root / "dist/release.json.sig").unlink()
                (root / "dist/release.json.sshsig").unlink()
                release.sign(root / "dist", key, public)
                result, ran = self.bootstrap(root, verifiers)
                self.assertIn("not signed by Artisan's release key", result.stderr)
                self.assertIsNone(ran)

    def test_bootstrap_refuses_to_run_ae_when_it_cannot_check_the_signature(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.signed_release(root)
            result, ran = self.bootstrap(root, ())
            self.assertIn("needs ssh-keygen (OpenSSH) or OpenSSL 3", result.stderr)
            self.assertIsNone(ran)

    def test_versions_name_their_immutable_tag_and_order_the_channels(self):
        self.assertEqual(release.tag_for("1.2.3"), "v1.2.3")
        self.assertEqual(release.tag_for("0.0.0-nightly.20261111220000+aaaaaaaaaaaa"), "v0.0.0-nightly.20261111220000")
        for version in ("1.2", "v1.2.3", "0.0.0-nightly.2026+aaaaaaaaaaaa", "1.2.3/../evil"):
            with self.assertRaises(ValueError):
                release.tag_for(version)
        self.assertTrue(release.advances("1.2.3", None))
        self.assertTrue(release.advances("2.0.0", "1.9.9"))
        self.assertTrue(release.advances("1.10.0", "1.9.0"))
        self.assertFalse(release.advances("1.9.1", "2.0.0"))
        self.assertFalse(release.advances("2.0.0", "2.0.0"))
        self.assertTrue(release.advances("0.0.0-nightly.20261112220000+bbbbbbbbbbbb", "0.0.0-nightly.20261111220000+aaaaaaaaaaaa"))
        self.assertFalse(release.advances("0.0.0-nightly.20261110220000+bbbbbbbbbbbb", "0.0.0-nightly.20261111220000+aaaaaaaaaaaa"))

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
