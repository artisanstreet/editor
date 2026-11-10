import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SOURCE = Path(__file__).resolve().parents[2] / "nix/dev.sh"
WSL = any(Path(f"/proc/sys/fs/binfmt_misc/{name}").exists() for name in ("WSLInterop", "WSLInterop-late"))
STUB = '''#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys
name = Path(sys.argv[0]).name
arguments = sys.argv[1:]
with Path(os.environ["TEST_LOG"]).open("a") as log:
    log.write(json.dumps([name, arguments]) + "\\n")
if name == "git":
    print(os.environ["TEST_ROOT"])
elif name == "nix":
    if os.environ.get("TEST_BUILD_FAILURE"):
        sys.exit(23)
    output = Path(arguments[arguments.index("--target-dir") + 1])
    profile = arguments[arguments.index("--profile") + 1]
    directory = output / "x86_64-pc-windows-gnu" / {"dev": "debug", "release": "release"}[profile]
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "editor-aqua.exe").write_bytes(b"MZfixture")
elif name == "wslpath":
    print("C:\\\\client's workspace\\\\editor-aqua.exe")
'''


@unittest.skipUnless(WSL, "client-only launching requires WSL")
class ClientOnly(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        root = Path(self.temporary.name)
        self.root = root / "client's workspace"
        (self.root / "crates/ae").mkdir(parents=True)
        (self.root / "crates/ae/Cargo.toml").touch()
        binaries = root / "bin"
        binaries.mkdir()
        for name in ("git", "nix", "wslpath", "powershell.exe"):
            executable = binaries / name
            executable.write_text(STUB)
            executable.chmod(0o755)
        self.log = root / "calls.jsonl"
        self.environment = os.environ | {
            "PATH": str(binaries) + os.pathsep + os.environ["PATH"],
            "TEST_ROOT": str(self.root),
            "TEST_LOG": str(self.log),
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

    def test_client_only_builds_only_aqua_and_dispatches_a_launch(self):
        result, calls = self.run_dev("--client-only")
        self.assertEqual(result.returncode, 0, result.stderr)
        builds = [arguments for name, arguments in calls if name == "nix"]
        self.assertEqual(len(builds), 1)
        self.assertIn(str(self.root) + "#cargo-windows", builds[0])
        self.assertEqual(builds[0][-4:], ["-p", "editor-aqua", "--bin", "editor-aqua"])
        launches = [arguments for name, arguments in calls if name == "powershell.exe"]
        self.assertEqual(len(launches), 1)
        self.assertIn("'C:\\client''s workspace\\editor-aqua.exe'", launches[0][-1])

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
