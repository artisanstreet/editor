import { generateKeyPairSync } from "node:crypto";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { basename, join } from "node:path";
import { tmpdir } from "node:os";

import { afterEach, describe, expect, it } from "vitest";
import { Effect, Layer } from "effect";

import {
	BuildWindowsDistributionRelease,
	BuildWindowsDistributionReleaseFromEnvironment,
} from "../../../.scripts/package/build-distribution-release";
import {
	NodeReleaseCryptographyLive,
	ReleaseVerification,
	ReleaseVerificationLive,
	make_trusted_release_keys_layer,
} from "../../../modules/distribution/src/verification";

const temporary_roots: Array<string> = [];

const TemporaryRoot = async () => {
	const root = await mkdtemp(join(tmpdir(), "artisan-distribution-release-"));
	temporary_roots.push(root);
	return root;
};

afterEach(async () => {
	await Promise.all(
		temporary_roots.splice(0).map((root) => rm(root, { force: true, recursive: true })),
	);
});

const ReadStoredZip = (bytes: Uint8Array) => {
	const buffer = Buffer.from(bytes);
	const entries = new Map<string, Uint8Array>();
	let offset = 0;
	while (buffer.readUInt32LE(offset) === 0x04034b50) {
		expect(buffer.readUInt16LE(offset + 8)).toBe(0);
		const size = buffer.readUInt32LE(offset + 18);
		const name_length = buffer.readUInt16LE(offset + 26);
		const extra_length = buffer.readUInt16LE(offset + 28);
		const name_start = offset + 30;
		const content_start = name_start + name_length + extra_length;
		const name = buffer.subarray(name_start, name_start + name_length).toString("utf8");
		expect(name).not.toMatch(/(?:^|\/)\.\.(?:\/|$)/u);
		expect(name).not.toMatch(/^[A-Za-z]:|^[\\/]/u);
		entries.set(name, buffer.subarray(content_start, content_start + size));
		offset = content_start + size;
	}
	expect(buffer.readUInt32LE(offset)).toBe(0x02014b50);
	return entries;
};

describe("Windows distribution release artifact", () => {
	it("assembles one deterministic, allowlisted, signed x64 Editor/Forge/ae archive", async () => {
		const root = await TemporaryRoot();
		const editor_root = join(root, "editor");
		const forge_root = join(root, "forge");
		const native_installer_path = join(root, "ae-installer.exe");
		const native_cli_path = join(root, "ae.exe");
		await Promise.all([
			mkdir(join(editor_root, "resources", "artisan-forge"), { recursive: true }),
			mkdir(forge_root, { recursive: true }),
		]);
		await Promise.all([
			writeFile(join(editor_root, "Artisan Editor.exe"), "editor"),
			writeFile(join(editor_root, "resources", "app.asar"), "asar"),
			writeFile(join(editor_root, "resources", "artisan-forge", "duplicate.txt"), "excluded"),
			writeFile(join(forge_root, "Artisan Forge.exe"), "forge"),
			writeFile(join(forge_root, "ae.js"), "cli"),
			writeFile(join(forge_root, "node.exe"), "node"),
			writeFile(native_installer_path, "native bootstrap"),
			writeFile(native_cli_path, "native ae"),
		]);
		const keys = generateKeyPairSync("ed25519");
		const private_key_pem = keys.privateKey
			.export({
				format: "pem",
				type: "pkcs8",
			})
			.toString();
		const common = {
			architecture: "x64",
			channel: "stable",
			editor_root,
			forge_root,
			key_id: "test-key",
			minimum_installer_version: "0.1.0",
			minimum_cli_version: "0.1.0",
			native_installer_path,
			native_cli_path,
			private_key_pem,
			product_version: "0.1.0",
		} as const;
		const first = await Effect.runPromise(
			BuildWindowsDistributionRelease({
				...common,
				output_root: join(root, "release-a"),
			}),
		);
		const second = await Effect.runPromise(
			BuildWindowsDistributionRelease({
				...common,
				output_root: join(root, "release-b"),
			}),
		);
		const [first_archive, second_archive, raw_manifest, raw_signature] = await Promise.all([
			readFile(first.archive_path),
			readFile(second.archive_path),
			readFile(first.manifest_path),
			readFile(first.signature_path, "utf8"),
		]);

		expect(basename(first.signature_path)).toBe("release-manifest.sig");
		expect(first_archive).toEqual(second_archive);
		const archive = ReadStoredZip(first_archive);
		expect([...archive.keys()]).toEqual([
			"bin/ae-installer.exe",
			"bin/ae.cmd",
			"bin/ae.exe",
			"editor/Artisan Editor.exe",
			"editor/resources/app.asar",
			"forge/ae.js",
			"forge/Artisan Forge.exe",
			"forge/node.exe",
		]);
		expect([...archive.keys()]).toEqual(first.archive_entries);
		expect([...archive.keys()].some((path) => path.includes("duplicate"))).toBe(false);

		const verification_layer = ReleaseVerificationLive.pipe(
			Layer.provide(NodeReleaseCryptographyLive),
			Layer.provide(
				make_trusted_release_keys_layer({
					"test-key": first.public_key_der,
				}),
			),
		);
		const manifest = await Effect.runPromise(
			Effect.gen(function* () {
				const verification = yield* ReleaseVerification;
				const manifest = yield* verification.VerifyManifest(
					raw_manifest,
					JSON.parse(raw_signature),
				);
				yield* verification.VerifyArtifact(manifest.artifacts[0]!, first_archive);
				return manifest;
			}).pipe(Effect.provide(verification_layer)),
		);
		expect(manifest.artifacts[0]?.archive_entries).toEqual(first.archive_entries);
		expect(manifest.artifacts[0]?.architecture).toBe("x64");
		expect(manifest.artifacts[0]?.byte_size).toBe(first_archive.byteLength);
	});

	it("rejects unsupported Windows arm64 before reading signing secrets", async () => {
		await expect(
			Effect.runPromise(
				BuildWindowsDistributionReleaseFromEnvironment({
					ARTISAN_RELEASE_ARCHITECTURE: "arm64",
				}),
			),
		).rejects.toMatchObject({
			_tag: "DistributionReleaseBuildError",
			code: "configuration",
		});
	});
});
