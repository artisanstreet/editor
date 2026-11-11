import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import zipfile
from zoneinfo import ZoneInfo

ANCHOR = "400a4b35cffdfa088227b9824176affb37b807fb26eb3c59b48ae56fbf5e4d8b"
PLATFORMS = {(system, arch) for system in ("linux", "macos", "windows") for arch in ("x86_64", "aarch64")}
EXPECTED = {(app, system, arch) for system, arch in PLATFORMS for app in ("ae", "forge")} | {("editor", "windows", "x86_64")}


def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def public_key(value):
    value = value.strip().lower()
    if value.startswith("302a300506032b6570032100"):
        value = value[24:]
    if value != ANCHOR:
        raise ValueError("release public key does not match the compiled trust anchor")
    return value


def version_for(reference, sha, date):
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("release requires a full commit SHA")
    if reference == "master":
        if not re.fullmatch(r"[0-9]{14}", date):
            raise ValueError("nightly date must be YYYYMMDDhhmmss")
        return f"0.0.0-nightly.{date}+{sha[:12]}", "nightly"
    if not re.fullmatch(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", reference):
        raise ValueError("release ref must be master or a stable vMAJOR.MINOR.PATCH tag")
    return reference[1:], reference


def midnight_in_oslo(moment):
    return moment.astimezone(ZoneInfo("Europe/Oslo")).hour == 0


def ci_green(runs, sha):
    eligible = [entry for entry in runs if entry["head_sha"] == sha and entry["event"] == "push" and entry["head_branch"] == "master"]
    return bool(eligible) and max(eligible, key=lambda entry: entry["id"])["conclusion"] == "success"


def gate(repository, sha):
    result = json.loads(run("gh", "api", f"repos/{repository}/actions/workflows/ci.yml/runs?head_sha={sha}&event=push&per_page=100"))
    if not ci_green(result["workflow_runs"], sha):
        raise ValueError("the exact release SHA lacks a successful latest master push CI run")


def stamp(root, version):
    import tomllib
    workspace_path = root / "Cargo.toml"
    workspace = workspace_path.read_text(encoding="utf-8")
    parsed = tomllib.loads(workspace)
    members = {tomllib.loads((root / member / "Cargo.toml").read_text(encoding="utf-8"))["package"]["name"] for member in parsed["workspace"]["members"]}
    workspace, count = re.subn(r'(?m)^(version = ")[^"]+("$)', lambda match: match[1] + version + match[2], workspace, count=1)
    if count != 1:
        raise ValueError("workspace version not found")
    lock_path = root / "Cargo.lock"
    blocks = lock_path.read_text(encoding="utf-8").split("[[package]]")
    for index, block in enumerate(blocks[1:], 1):
        record = tomllib.loads(block)
        if record["name"] in members and "source" not in record:
            blocks[index] = re.sub(r'(?m)^(version = ")[^"]+("$)', lambda match: match[1] + version + match[2], block, count=1)
    workspace_path.write_text(workspace, encoding="utf-8")
    lock_path.write_text("[[package]]".join(blocks), encoding="utf-8")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def package(root, binaries, output, app, system, arch, version):
    if (app, system, arch) not in EXPECTED:
        raise ValueError("unsupported release artifact")
    output.mkdir(parents=True, exist_ok=True)
    binary = "editor-aqua" if app == "editor" else app
    binary += ".exe" if system == "windows" else ""
    name = "Artisan Editor.exe" if app == "editor" else binary
    extension = "zip" if system == "windows" else "tar.gz"
    filename = f"{app}-{version}-{system}-{arch}.{extension}"
    with tempfile.TemporaryDirectory() as temporary:
        stage = Path(temporary)
        shutil.copyfile(binaries / binary, stage / name)
        (stage / name).chmod(0o755)
        shutil.copyfile(root / "LICENSE.md", stage / "LICENSE.md")
        if app == "editor":
            shutil.copytree(root / "crates/editor-assets/licenses", stage / "notices")
            shutil.copyfile(root / "vendor/highlighting/NOTICE.txt", stage / "HIGHLIGHTING-NOTICES.txt")
            shutil.copyfile(root / "vendor/mermaid-rs-renderer/LICENSE", stage / "MERMAID-LICENSE.txt")
        files = sorted(path for path in stage.rglob("*") if path.is_file())
        archive = output / filename
        if system == "windows":
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as writer:
                for path in files:
                    writer.write(path, path.relative_to(stage).as_posix())
        else:
            with tarfile.open(archive, "w:gz") as writer:
                for path in files:
                    writer.add(path, path.relative_to(stage).as_posix(), recursive=False)
    entry = {"app": app, "os": system, "arch": arch, "file": filename, "sha256": digest(archive)}
    (output / f"{filename}.json").write_text(json.dumps(entry), encoding="utf-8")
    return entry


def assemble(root, source, output, version, sha):
    entries = []
    seen = set()
    output.mkdir(parents=True, exist_ok=False)
    for metadata in sorted(source.rglob("*.json")):
        entry = json.loads(metadata.read_text(encoding="utf-8"))
        identity = (entry["app"], entry["os"], entry["arch"])
        if identity not in EXPECTED or identity in seen:
            raise ValueError("duplicate or unexpected release artifact")
        extension = "zip" if entry["os"] == "windows" else "tar.gz"
        expected_file = f"{entry['app']}-{version}-{entry['os']}-{entry['arch']}.{extension}"
        if entry["file"] != expected_file:
            raise ValueError("artifact name does not match the release")
        archive = metadata.parent / expected_file
        if digest(archive) != entry["sha256"]:
            raise ValueError("artifact checksum mismatch")
        shutil.copyfile(archive, output / expected_file)
        seen.add(identity)
        entries.append(entry)
    if seen != EXPECTED:
        raise ValueError("release is missing platform artifacts")
    lines = ",\n".join(json.dumps(entry, separators=(",", ":")) for entry in entries)
    (output / "release.json").write_text(f'{{"version":"{version}","commit":"{sha}","artifacts":[\n{lines}\n]}}\n', encoding="utf-8")
    for name in ("install.sh", "install.ps1"):
        shutil.copyfile(root / "scripts" / name, output / name)


def validate(output, sha=None):
    manifest_data = json.loads((output / "release.json").read_text(encoding="utf-8"))
    seen = set()
    for entry in manifest_data["artifacts"]:
        identity = (entry["app"], entry["os"], entry["arch"])
        if identity not in EXPECTED or identity in seen or Path(entry["file"]).name != entry["file"]:
            raise ValueError("invalid signed artifact set")
        if digest(output / entry["file"]) != entry["sha256"]:
            raise ValueError("artifact changed before signing")
        seen.add(identity)
    if seen != EXPECTED:
        raise ValueError("signed release is incomplete")
    if sha is not None and manifest_data.get("commit") != sha:
        raise ValueError("manifest commit differs from the release SHA")
    return manifest_data


def verify(output, expected, sha):
    validate(output, sha)
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        (directory / "public.der").write_bytes(bytes.fromhex("302a300506032b6570032100" + expected))
        (directory / "signature").write_bytes(base64.b64decode((output / "release.json.sig").read_bytes(), validate=True))
        run("openssl", "pkeyutl", "-verify", "-rawin", "-pubin", "-keyform", "DER", "-inkey", str(directory / "public.der"), "-in", str(output / "release.json"), "-sigfile", str(directory / "signature"))


def sign(output, pem, expected):
    validate(output)
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        key = directory / "key.pem"
        key.write_text(pem, encoding="utf-8")
        key.chmod(0o600)
        public = subprocess.check_output(["openssl", "pkey", "-in", str(key), "-pubout", "-outform", "DER"])
        if public != bytes.fromhex("302a300506032b6570032100" + expected):
            raise ValueError("signing key does not match the release trust anchor")
        (directory / "public.der").write_bytes(public)
        manifest = output / "release.json"
        signature = directory / "signature"
        run("openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(key), "-in", str(manifest), "-out", str(signature))
        run("openssl", "pkeyutl", "-verify", "-rawin", "-pubin", "-keyform", "DER", "-inkey", str(directory / "public.der"), "-in", str(manifest), "-sigfile", str(signature))
        (output / "release.json.sig").write_bytes(base64.b64encode(signature.read_bytes()))


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    key = commands.add_parser("key")
    key.add_argument("--value", default=os.environ.get("ARTISAN_RELEASE_PUBLIC_KEY_HEX", ""))
    check = commands.add_parser("gate")
    check.add_argument("--repository", required=True)
    check.add_argument("--sha", required=True)
    pack = commands.add_parser("package")
    for name in ("binaries", "output", "app", "os", "arch", "version"):
        pack.add_argument(f"--{name}", required=True)
    combine = commands.add_parser("assemble")
    for name in ("source", "output", "version", "sha"):
        combine.add_argument(f"--{name}", required=True)
    stamping = commands.add_parser("stamp")
    stamping.add_argument("--version", required=True)
    verification = commands.add_parser("verify")
    verification.add_argument("--output", required=True)
    verification.add_argument("--sha", required=True)
    signing = commands.add_parser("sign")
    signing.add_argument("--output", required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    if args.command == "key":
        print(public_key(args.value))
    elif args.command == "gate":
        gate(args.repository, args.sha)
    elif args.command == "package":
        package(root, Path(args.binaries), Path(args.output), args.app, args.os, args.arch, args.version)
    elif args.command == "assemble":
        assemble(root, Path(args.source), Path(args.output), args.version, args.sha)
    elif args.command == "stamp":
        stamp(root, args.version)
    elif args.command == "verify":
        verify(Path(args.output), public_key(os.environ["ARTISAN_RELEASE_PUBLIC_KEY_HEX"]), args.sha)
    elif args.command == "sign":
        sign(Path(args.output), os.environ["ARTISAN_RELEASE_SIGNING_KEY_PEM"], public_key(os.environ["ARTISAN_RELEASE_PUBLIC_KEY_HEX"]))


if __name__ == "__main__":
    main()
