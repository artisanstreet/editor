import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from zoneinfo import ZoneInfo

ANCHOR = "400a4b35cffdfa088227b9824176affb37b807fb26eb3c59b48ae56fbf5e4d8b"
SPKI = "302a300506032b6570032100"
NAMESPACE = "artisan-release"
EXPECTED = {("ae", "linux", "x86_64"), ("ae", "linux", "aarch64"), ("ae", "macos", "aarch64"), ("editor", "windows", "x86_64")}
OPTIONAL = {("editor", "windows", "aarch64")}
SYSTEMS = {"linux": "Linux", "macos": "macOS", "windows": "Windows"}
CHIPS = {("macos", "aarch64"): "Apple Silicon", ("linux", "aarch64"): "ARM64", ("windows", "aarch64"): "ARM64"}
SUPPORT = {"install.sh": "Install script for Linux and macOS", "release.json.sig": "Release manifest signature", "release.json.sshsig": "Release manifest signature for ssh-keygen", "release.json": "Signed release manifest"}
STABLE = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
NIGHTLY_VERSION = re.compile(r"0\.0\.0-nightly\.([0-9]{14})\+[0-9a-f]{12}")
LATEST = "releases/latest/download/release.json"
NIGHTLY = "releases/download/nightly/release.json"


def filename(app, system, arch):
    return f"Artisan-Editor-{arch}.exe" if app == "editor" else f"ae-{system}-{arch}"


def label(app, system, arch):
    product = "Artisan Editor" if app == "editor" else "ae CLI"
    return f"{product} for {SYSTEMS[system]} ({CHIPS.get((system, arch), arch)})"


def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def ssh_string(data):
    return len(data).to_bytes(4, "big") + data


def ssh_public(anchor):
    return ssh_string(b"ssh-ed25519") + ssh_string(bytes.fromhex(anchor))


def allowed_signer(anchor):
    return f"{NAMESPACE} ssh-ed25519 {base64.b64encode(ssh_public(anchor)).decode()}"


def pem_public(anchor):
    return base64.b64encode(bytes.fromhex(SPKI + anchor)).decode()


def sshsig_message(manifest):
    return b"SSHSIG" + ssh_string(NAMESPACE.encode()) + ssh_string(b"") + ssh_string(b"sha512") + ssh_string(hashlib.sha512(manifest).digest())


def sshsig_armor(anchor, signature):
    blob = b"SSHSIG" + (1).to_bytes(4, "big") + ssh_string(ssh_public(anchor)) + ssh_string(NAMESPACE.encode()) + ssh_string(b"") + ssh_string(b"sha512") + ssh_string(ssh_string(b"ssh-ed25519") + ssh_string(signature))
    text = base64.b64encode(blob).decode()
    lines = [text[index:index + 70] for index in range(0, len(text), 70)]
    return "-----BEGIN SSH SIGNATURE-----\n" + "\n".join(lines) + "\n-----END SSH SIGNATURE-----\n"


def tag_for(version):
    if STABLE.fullmatch(version) or NIGHTLY_VERSION.fullmatch(version):
        return "v" + version.split("+", 1)[0]
    raise ValueError(f"no release tag for version {version!r}")


def precedence(version):
    if STABLE.fullmatch(version):
        return tuple(int(part) for part in version.split(".")), 1, ""
    nightly = NIGHTLY_VERSION.fullmatch(version)
    if nightly:
        return (0, 0, 0), 0, nightly[1]
    raise ValueError(f"unordered release version {version!r}")


def advances(candidate, current):
    return current is None or precedence(candidate) > precedence(current)


def public_key(value):
    value = value.strip().lower()
    if value.startswith(SPKI):
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


def night_in_oslo(moment):
    return moment.astimezone(ZoneInfo("Europe/Oslo")).hour < 6


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


def package(binaries, output, app, system, arch):
    if (app, system, arch) not in EXPECTED | OPTIONAL:
        raise ValueError("unsupported release artifact")
    output.mkdir(parents=True, exist_ok=True)
    binary = "editor-aqua" if app == "editor" else app
    binary += ".exe" if system == "windows" else ""
    name = filename(app, system, arch)
    shutil.copyfile(binaries / binary, output / name)
    (output / name).chmod(0o755)
    entry = {"app": app, "os": system, "arch": arch, "file": name, "sha256": digest(output / name)}
    (output / f"{name}.json").write_text(json.dumps(entry), encoding="utf-8")
    return entry


def assemble(root, source, output, version, sha, anchor):
    tag_for(version)
    entries = []
    seen = set()
    output.mkdir(parents=True, exist_ok=False)
    for metadata in sorted(source.rglob("*.json")):
        entry = json.loads(metadata.read_text(encoding="utf-8"))
        identity = (entry["app"], entry["os"], entry["arch"])
        if identity not in EXPECTED | OPTIONAL or identity in seen:
            raise ValueError("duplicate or unexpected release artifact")
        if entry["file"] != filename(*identity):
            raise ValueError("artifact name does not match the release")
        artifact = metadata.parent / entry["file"]
        if digest(artifact) != entry["sha256"]:
            raise ValueError("artifact checksum mismatch")
        shutil.copyfile(artifact, output / entry["file"])
        seen.add(identity)
        entries.append(entry)
    if not EXPECTED <= seen:
        raise ValueError("release is missing platform artifacts")
    lines = ",\n".join(json.dumps(entry, separators=(",", ":")) for entry in entries)
    (output / "release.json").write_text(f'{{"version":"{version}","commit":"{sha}","artifacts":[\n{lines}\n]}}\n', encoding="utf-8")
    installer = (root / "scripts/install.sh").read_text(encoding="utf-8")
    if LATEST not in installer:
        raise ValueError("install.sh does not name the release feed")
    if f"signer='{allowed_signer(anchor)}'" not in installer or f"public_key='{pem_public(anchor)}'" not in installer:
        raise ValueError("install.sh does not pin the release trust anchor")
    if "-nightly." in version:
        installer = installer.replace(LATEST, NIGHTLY)
    (output / "install.sh").write_text(installer, encoding="utf-8")


def assets(output):
    manifest_data = json.loads((output / "release.json").read_text(encoding="utf-8"))
    labelled = [(entry["file"], label(entry["app"], entry["os"], entry["arch"])) for entry in manifest_data["artifacts"]]
    return [f"{output / name}#{text}" for name, text in labelled + list(SUPPORT.items())]


def validate(output, sha=None):
    manifest_data = json.loads((output / "release.json").read_text(encoding="utf-8"))
    seen = set()
    for entry in manifest_data["artifacts"]:
        identity = (entry["app"], entry["os"], entry["arch"])
        if identity not in EXPECTED | OPTIONAL or identity in seen or entry["file"] != filename(*identity):
            raise ValueError("invalid signed artifact set")
        if digest(output / entry["file"]) != entry["sha256"]:
            raise ValueError("artifact changed before signing")
        seen.add(identity)
    if not EXPECTED <= seen:
        raise ValueError("signed release is incomplete")
    if sha is not None and manifest_data.get("commit") != sha:
        raise ValueError("manifest commit differs from the release SHA")
    return manifest_data


def verify(output, expected, sha):
    manifest = output / "release.json"
    with tempfile.TemporaryDirectory() as temporary, manifest.open("rb") as signed:
        directory = Path(temporary)
        (directory / "public.der").write_bytes(bytes.fromhex(SPKI + expected))
        (directory / "signers").write_text(allowed_signer(expected) + "\n", encoding="utf-8")
        try:
            (directory / "signature").write_bytes(base64.b64decode((output / "release.json.sig").read_bytes(), validate=True))
            run("openssl", "pkeyutl", "-verify", "-rawin", "-pubin", "-keyform", "DER", "-inkey", str(directory / "public.der"), "-in", str(manifest), "-sigfile", str(directory / "signature"), stderr=subprocess.DEVNULL)
            run("ssh-keygen", "-Y", "verify", "-f", str(directory / "signers"), "-I", NAMESPACE, "-n", NAMESPACE, "-s", str(output / "release.json.sshsig"), stdin=signed, stderr=subprocess.DEVNULL)
        except (subprocess.CalledProcessError, ValueError) as error:
            raise ValueError("release.json is not signed by the release key") from error
    return validate(output, sha)


def sign(output, pem, expected):
    validate(output)
    with tempfile.TemporaryDirectory() as temporary:
        directory = Path(temporary)
        key = directory / "key.pem"
        key.write_text(pem, encoding="utf-8")
        key.chmod(0o600)
        public = subprocess.check_output(["openssl", "pkey", "-in", str(key), "-pubout", "-outform", "DER"])
        if public != bytes.fromhex(SPKI + expected):
            raise ValueError("signing key does not match the release trust anchor")
        manifest = output / "release.json"
        message = directory / "sshsig-message"
        message.write_bytes(sshsig_message(manifest.read_bytes()))
        signature = directory / "signature"
        sshsig = directory / "sshsig"
        run("openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(key), "-in", str(manifest), "-out", str(signature))
        run("openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(key), "-in", str(message), "-out", str(sshsig))
        (output / "release.json.sig").write_bytes(base64.b64encode(signature.read_bytes()))
        (output / "release.json.sshsig").write_text(sshsig_armor(expected, sshsig.read_bytes()), encoding="utf-8")
    verify(output, expected, None)


def main():
    parser = argparse.ArgumentParser()
    commands = parser.add_subparsers(dest="command", required=True)
    key = commands.add_parser("key")
    key.add_argument("--value", default=os.environ.get("ARTISAN_RELEASE_PUBLIC_KEY_HEX", ""))
    check = commands.add_parser("gate")
    check.add_argument("--repository", required=True)
    check.add_argument("--sha", required=True)
    pack = commands.add_parser("package")
    for name in ("binaries", "output", "app", "os", "arch"):
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
    listing = commands.add_parser("assets")
    listing.add_argument("--output", required=True)
    tagging = commands.add_parser("tag")
    tagging.add_argument("--version", required=True)
    promotion = commands.add_parser("advances")
    promotion.add_argument("--candidate", required=True)
    promotion.add_argument("--current")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    if args.command == "key":
        print(public_key(args.value))
    elif args.command == "gate":
        gate(args.repository, args.sha)
    elif args.command == "package":
        package(Path(args.binaries), Path(args.output), args.app, args.os, args.arch)
    elif args.command == "assemble":
        assemble(root, Path(args.source), Path(args.output), args.version, args.sha, ANCHOR)
    elif args.command == "stamp":
        stamp(root, args.version)
    elif args.command == "verify":
        verify(Path(args.output), public_key(os.environ["ARTISAN_RELEASE_PUBLIC_KEY_HEX"]), args.sha)
    elif args.command == "assets":
        print("\n".join(assets(Path(args.output))))
    elif args.command == "tag":
        print(tag_for(args.version))
    elif args.command == "advances":
        print(str(advances(args.candidate, args.current or None)).lower())
    elif args.command == "sign":
        sign(Path(args.output), os.environ["ARTISAN_RELEASE_SIGNING_KEY_PEM"], public_key(os.environ["ARTISAN_RELEASE_PUBLIC_KEY_HEX"]))


if __name__ == "__main__":
    main()
