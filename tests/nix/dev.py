import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest


SOURCE = Path(__file__).resolve().parents[2] / "nix/dev.sh"
WSL = any(Path(f"/proc/sys/fs/binfmt_misc/{name}").exists() for name in ("WSLInterop", "WSLInterop-late"))
STUB = '''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import subprocess
import sys
name = Path(sys.argv[0]).name
arguments = sys.argv[1:]
with Path(os.environ["TEST_LOG"]).open("a") as log:
    log.write(json.dumps([name, arguments]) + "\\n")
if name == "git":
    if "rev-parse" in arguments and "--show-toplevel" not in arguments:
        print("0123456789")
    elif "--show-toplevel" in arguments:
        print(os.environ["TEST_ROOT"])
elif name == "nix":
    if os.environ.get("TEST_BUILD_FAILURE"):
        sys.exit(23)
    output = Path(arguments[arguments.index("--target-dir") + 1])
    profile = {"dev": "debug", "release": "release"}[arguments[arguments.index("--profile") + 1]]
    if any(argument.endswith("#cargo-windows") for argument in arguments):
        directory = output / "x86_64-pc-windows-gnu" / profile
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "editor-aqua.exe").write_bytes(b"MZfixture")
    else:
        directory = output / profile
        directory.mkdir(parents=True, exist_ok=True)
        for binary in ("ae", "forge"):
            (directory / binary).write_text(Path(sys.argv[0]).read_text())
            (directory / binary).chmod(0o755)
elif name == "ae":
    source = Path(arguments[arguments.index("--from") + 1])
    with Path(os.environ["TEST_LOG"]).open("a") as log:
        log.write(json.dumps(["stage", str(source), (source / "linux/VERSION").read_text().strip()]) + "\\n")
    if os.environ.get("TEST_INSTALL_FAILURE"):
        sys.exit(31)
elif name == "systemctl":
    sys.exit(0 if os.environ.get("TEST_SYSTEMD") else 1)
elif name == "systemd-run":
    command = arguments[next(i for i, argument in enumerate(arguments) if not argument.startswith("--")):]
    sys.exit(subprocess.run(command).returncode)
elif name == "wslpath":
    if arguments[0] == "-u":
        print(os.environ["TEST_WINDOWS_TEMP"])
    else:
        print("C:\\\\client's workspace\\\\editor-aqua.exe")
elif name == "powershell.exe":
    if "GetTempPath" in arguments[-1]:
        print("C:\\\\Temp\\\\\\r")
'''


class DevLauncher(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        root = Path(self.temporary.name)
        self.root = root / "client's workspace"
        (self.root / "crates/ae").mkdir(parents=True)
        (self.root / "crates/ae/Cargo.toml").touch()
        (self.root / "Cargo.toml").write_text('version = "1.2.3"\n')
        binaries = root / "bin"
        binaries.mkdir()
        for name in ("git", "nix", "wslpath", "powershell.exe", "systemctl", "systemd-run"):
            executable = binaries / name
            executable.write_text(STUB)
            executable.chmod(0o755)
        self.windows_temp = root / "windows-temp"
        self.windows_temp.mkdir()
        self.log = root / "calls.jsonl"
        self.environment = os.environ | {
            "PATH": str(binaries) + os.pathsep + os.environ["PATH"],
            "TEST_ROOT": str(self.root),
            "TEST_LOG": str(self.log),
            "TEST_WINDOWS_TEMP": str(self.windows_temp),
            "ARTISAN_DEV_PROFILE": "debug",
            "ARTISAN_DEV_BLAZE": "",
        }

    def run_dev(self, *arguments):
        result = subprocess.run(
            ["bash", "-euo", "pipefail", str(SOURCE), *arguments],
            cwd=self.root, env=self.environment, capture_output=True, text=True,
        )
        calls = [json.loads(line) for line in self.log.read_text().splitlines()]
        return result, calls


class Staging(DevLauncher):
    def install_twice(self):
        for _ in range(2):
            result, calls = self.run_dev("forge", "--linux")
        return result, [Path(entry[1]) for entry in calls if entry[0] == "stage"], calls

    def test_each_install_stages_into_its_own_directory_and_removes_it(self):
        for systemd in ("", "1"):
            with self.subTest(systemd=bool(systemd)):
                self.environment["TEST_SYSTEMD"] = systemd
                self.log.unlink(missing_ok=True)
                result, stages, calls = self.install_twice()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(len(stages), 2)
                self.assertNotEqual(stages[0], stages[1])
                self.assertTrue(all(entry[2] == "1.2.3-dev+0123456789" for entry in calls if entry[0] == "stage"))
                self.assertFalse(any(stage.exists() for stage in stages))

    def test_failed_install_still_removes_its_stage(self):
        self.environment["TEST_INSTALL_FAILURE"] = "1"
        for systemd in ("", "1"):
            with self.subTest(systemd=bool(systemd)):
                self.environment["TEST_SYSTEMD"] = systemd
                self.log.unlink(missing_ok=True)
                result, stages, _ = self.install_twice()
                self.assertEqual(result.returncode, 31)
                self.assertFalse(any(stage.exists() for stage in stages))


@unittest.skipUnless(WSL, "client-only launching requires WSL")
class ClientOnly(DevLauncher):
    def test_client_only_builds_only_aqua_and_dispatches_a_launch(self):
        result, calls = self.run_dev("--client-only")
        self.assertEqual(result.returncode, 0, result.stderr)
        builds = [arguments for name, arguments in calls if name == "nix"]
        self.assertEqual(len(builds), 1)
        self.assertIn(str(self.root) + "#cargo-windows", builds[0])
        self.assertEqual(builds[0][-4:], ["-p", "editor-aqua", "--bin", "editor-aqua"])
        launches = [
            arguments for name, arguments in calls
            if name == "powershell.exe" and "Start-Process" in arguments[-1]
        ]
        self.assertEqual(len(launches), 1)
        self.assertIn("'C:\\client''s workspace\\editor-aqua.exe'", launches[0][-1])

    def test_launch_reclaims_copies_left_by_closed_clients(self):
        copies = {
            name: self.windows_temp / f"artisan-editor-dev-{name}"
            for name in ("closed", "running", "starting")
        }
        for directory in copies.values():
            directory.mkdir()
            (directory / "Artisan Editor Dev.exe").write_bytes(b"MZold")
        an_hour_ago = time.time() - 3600
        for name in ("closed", "running"):
            os.utime(copies[name], (an_hour_ago, an_hour_ago))
        copies["running"].chmod(0o555)
        self.addCleanup(copies["running"].chmod, 0o755)
        result, _ = self.run_dev("--client-only")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(copies["closed"].exists())
        self.assertTrue((copies["running"] / "Artisan Editor Dev.exe").exists())
        self.assertTrue((copies["starting"] / "Artisan Editor Dev.exe").exists())
        launched = [path for path in self.windows_temp.iterdir() if path not in copies.values()]
        self.assertEqual(len(launched), 1)
        self.assertEqual((launched[0] / "Artisan Editor Dev.exe").read_bytes(), b"MZfixture")

    def test_failed_build_never_launches(self):
        self.environment["TEST_BUILD_FAILURE"] = "1"
        result, calls = self.run_dev("--client-only")
        self.assertEqual(result.returncode, 23)
        self.assertNotIn("powershell.exe", [name for name, _ in calls])

    def test_client_only_build_mode_skips_launching(self):
        result, calls = self.run_dev("build", "--client-only")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([name for name, _ in calls], ["git", "nix"])

    def test_forge_mode_rejects_client_only_before_building(self):
        result, calls = self.run_dev("forge", "--client-only")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual([name for name, _ in calls], ["git"])


if __name__ == "__main__":
    unittest.main()
