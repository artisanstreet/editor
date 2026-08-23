import { createHash, createPrivateKey, createPublicKey, sign, verify } from "node:crypto";
import { lstat, mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { join, relative, resolve, sep } from "node:path";

import { Data, Effect, Schema } from "effect";

import { ReleaseManifest } from "../../modules/distribution/src/release-manifest.ts";
import { ReleaseManifestSignature } from "../../modules/distribution/src/verification.ts";

const DistributionReleaseInput = Schema.Struct({
	architecture: Schema.Literal("x64"),
	channel: Schema.Literals(["stable", "beta", "nightly"]),
	editor_root: Schema.NonEmptyString,
	forge_root: Schema.NonEmptyString,
	key_id: Schema.NonEmptyString,
	minimum_installer_version: Schema.NonEmptyString,
	minimum_cli_version: Schema.NonEmptyString,
	native_installer_path: Schema.optional(Schema.NonEmptyString),
	native_cli_path: Schema.optional(Schema.NonEmptyString),
	output_root: Schema.NonEmptyString,
	private_key_pem: Schema.NonEmptyString,
	product_version: Schema.NonEmptyString,
});
export type DistributionReleaseInput = typeof DistributionReleaseInput.Type;

export interface DistributionReleaseOutput {
	readonly archive_entries: ReadonlyArray<string>;
	readonly archive_path: string;
	readonly manifest_path: string;
	readonly public_key_der: Uint8Array;
	readonly signature_path: string;
}

export class DistributionReleaseBuildError extends Data.TaggedError(
	"DistributionReleaseBuildError",
)<{
	readonly cause?: unknown;
	readonly code: "archive" | "configuration" | "input" | "manifest" | "signing" | "write";
}> {}

interface ArchiveEntry {
	readonly bytes: Uint8Array;
	readonly path: string;
}

const text_encoder = new TextEncoder();

const Crc32 = (bytes: Uint8Array) => {
	let crc = 0xffffffff;
	for (const byte of bytes) {
		crc ^= byte;
		for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
	}
	return (crc ^ 0xffffffff) >>> 0;
};

const Uint16 = (value: number) => {
	const bytes = Buffer.allocUnsafe(2);
	bytes.writeUInt16LE(value);
	return bytes;
};

const Uint32 = (value: number) => {
	const bytes = Buffer.allocUnsafe(4);
	bytes.writeUInt32LE(value);
	return bytes;
};

/** Creates a deterministic ZIP using stored entries and the DOS epoch. */
export const CreateDeterministicZip = (entries: ReadonlyArray<ArchiveEntry>): Uint8Array => {
	if (entries.length === 0 || entries.length > 65_535)
		throw new Error("ZIP entry count is outside the supported range");
	const local_parts: Array<Uint8Array> = [];
	const central_parts: Array<Uint8Array> = [];
	let offset = 0;

	for (const entry of [...entries].sort((left, right) => left.path.localeCompare(right.path))) {
		if (entry.bytes.byteLength > 0xffffffff)
			throw new Error(`ZIP entry exceeds 4 GiB: ${entry.path}`);
		const name = text_encoder.encode(entry.path);
		const crc = Crc32(entry.bytes);
		const local_header = Buffer.concat([
			Uint32(0x04034b50),
			Uint16(20),
			Uint16(0x0800),
			Uint16(0),
			Uint16(0),
			Uint16(0x0021),
			Uint32(crc),
			Uint32(entry.bytes.byteLength),
			Uint32(entry.bytes.byteLength),
			Uint16(name.byteLength),
			Uint16(0),
			name,
		]);
		local_parts.push(local_header, entry.bytes);
		central_parts.push(
			Buffer.concat([
				Uint32(0x02014b50),
				Uint16(20),
				Uint16(20),
				Uint16(0x0800),
				Uint16(0),
				Uint16(0),
				Uint16(0x0021),
				Uint32(crc),
				Uint32(entry.bytes.byteLength),
				Uint32(entry.bytes.byteLength),
				Uint16(name.byteLength),
				Uint16(0),
				Uint16(0),
				Uint16(0),
				Uint16(0),
				Uint32(0),
				Uint32(offset),
				name,
			]),
		);
		offset += local_header.byteLength + entry.bytes.byteLength;
		if (offset > 0xffffffff) throw new Error("ZIP local entries exceed 4 GiB");
	}

	const central_directory = Buffer.concat(central_parts);
	if (central_directory.byteLength > 0xffffffff)
		throw new Error("ZIP central directory exceeds 4 GiB");
	return Buffer.concat([
		...local_parts,
		central_directory,
		Uint32(0x06054b50),
		Uint16(0),
		Uint16(0),
		Uint16(entries.length),
		Uint16(entries.length),
		Uint32(central_directory.byteLength),
		Uint32(offset),
		Uint16(0),
	]);
};

export const ReadStoredZipEntryPaths = (bytes: Uint8Array): ReadonlyArray<string> => {
	const buffer = Buffer.from(bytes);
	const entries: Array<string> = [];
	let offset = 0;
	while (offset + 30 <= buffer.byteLength && buffer.readUInt32LE(offset) === 0x04034b50) {
		if (buffer.readUInt16LE(offset + 8) !== 0)
			throw new Error("Only stored ZIP entries are supported");
		const size = buffer.readUInt32LE(offset + 18);
		const name_length = buffer.readUInt16LE(offset + 26);
		const extra_length = buffer.readUInt16LE(offset + 28);
		const name_start = offset + 30;
		const content_start = name_start + name_length + extra_length;
		const next_offset = content_start + size;
		if (next_offset > buffer.byteLength) throw new Error("Truncated ZIP entry");
		const name = buffer.subarray(name_start, name_start + name_length).toString("utf8");
		if (
			name.length === 0 ||
			/(?:^|[\\/])\.\.(?:[\\/]|$)/u.test(name) ||
			/^[A-Za-z]:|^[\\/]/u.test(name)
		)
			throw new Error(`Unsafe ZIP entry path: ${name}`);
		entries.push(name);
		offset = next_offset;
	}
	if (offset + 4 > buffer.byteLength || buffer.readUInt32LE(offset) !== 0x02014b50)
		throw new Error("ZIP central directory is missing");
	if (new Set(entries).size !== entries.length) throw new Error("ZIP contains duplicate entries");
	return entries;
};

const ToArchivePath = (path: string) => path.split(sep).join("/");

const CollectFiles = (
	root: string,
	archive_root: string,
	excluded_prefixes: ReadonlyArray<string> = [],
) =>
	Effect.tryPromise({
		try: async () => {
			const entries: Array<ArchiveEntry> = [];
			const Visit = async (directory: string): Promise<void> => {
				const children = await readdir(directory);
				children.sort((left, right) => left.localeCompare(right));
				for (const child of children) {
					const absolute_path = join(directory, child);
					const source_relative_path = ToArchivePath(relative(root, absolute_path));
					if (
						excluded_prefixes.some(
							(prefix) =>
								source_relative_path === prefix ||
								source_relative_path.startsWith(`${prefix}/`),
						)
					)
						continue;
					const metadata = await lstat(absolute_path);
					if (metadata.isSymbolicLink())
						throw new Error(`Symbolic links are not allowed: ${absolute_path}`);
					if (metadata.isDirectory()) {
						await Visit(absolute_path);
						continue;
					}
					if (!metadata.isFile())
						throw new Error(`Unsupported archive entry: ${absolute_path}`);
					entries.push({
						bytes: await readFile(absolute_path),
						path: `${archive_root}/${source_relative_path}`,
					});
				}
			};
			await Visit(root);
			return entries;
		},
		catch: (cause) => new DistributionReleaseBuildError({ cause, code: "input" }),
	});

const CanonicalJson = (value: unknown) => `${JSON.stringify(value)}\n`;

const PermanentAe = text_encoder.encode(["@echo off", '"%~dp0ae.exe" %*', ""].join("\r\n"));

const ValidateForgeSeaPayload = (entries: ReadonlyArray<ArchiveEntry>) => {
	const forge_entries = entries.filter((entry) => entry.path.startsWith("forge/"));
	const expected = ["forge/Artisan Broker.exe", "forge/Artisan Forge.exe"];
	if (
		forge_entries.length !== expected.length ||
		forge_entries.some((entry, index) => entry.path !== expected[index])
	)
		throw new Error(
			`Forge payload must contain only Artisan Broker and Artisan Forge; received: ${forge_entries
				.map((entry) => entry.path)
				.join(", ")}`,
		);
};

export const BuildWindowsDistributionRelease = (input: DistributionReleaseInput) =>
	Effect.gen(function* () {
		const configuration = yield* Schema.decodeUnknownEffect(DistributionReleaseInput)(
			input,
		).pipe(
			Effect.mapError(
				(cause) => new DistributionReleaseBuildError({ cause, code: "configuration" }),
			),
		);
		const [editor_entries, forge_entries, native_entries] = yield* Effect.all([
			CollectFiles(configuration.editor_root, "editor", ["resources/artisan-forge"]),
			CollectFiles(configuration.forge_root, "forge"),
			Effect.tryPromise({
				try: async () => {
					if (!configuration.native_cli_path || !configuration.native_installer_path)
						return [] as Array<ArchiveEntry>;
					return [
						{
							bytes: await readFile(configuration.native_cli_path),
							path: "bin/ae.exe",
						},
						{
							bytes: await readFile(configuration.native_installer_path),
							path: "bin/ae-installer.exe",
						},
					];
				},
				catch: (cause) => new DistributionReleaseBuildError({ cause, code: "input" }),
			}),
		]);
		const entries = [
			...editor_entries,
			...forge_entries,
			...native_entries,
			{ bytes: PermanentAe, path: "bin/ae.cmd" },
		].sort((left, right) => left.path.localeCompare(right.path));
		yield* Effect.try({
			try: () => ValidateForgeSeaPayload(entries),
			catch: (cause) => new DistributionReleaseBuildError({ cause, code: "input" }),
		});
		if (
			!entries.some((entry) => entry.path === "editor/Artisan Editor.exe") ||
			!entries.some((entry) => entry.path === "forge/Artisan Broker.exe") ||
			!entries.some((entry) => entry.path === "forge/Artisan Forge.exe") ||
			(configuration.native_cli_path !== undefined &&
				!entries.some((entry) => entry.path === "bin/ae.exe")) ||
			(configuration.native_installer_path !== undefined &&
				!entries.some((entry) => entry.path === "bin/ae-installer.exe"))
		)
			return yield* Effect.fail(
				new DistributionReleaseBuildError({
					cause: "Editor, Forge, or permanent ae payload is incomplete",
					code: "input",
				}),
			);

		const archive_bytes = yield* Effect.try({
			try: () => {
				const bytes = CreateDeterministicZip(entries);
				const actual_entries = ReadStoredZipEntryPaths(bytes);
				const expected_entries = entries.map((entry) => entry.path);
				if (JSON.stringify(actual_entries) !== JSON.stringify(expected_entries))
					throw new Error("ZIP allowlist does not match its payload");
				return bytes;
			},
			catch: (cause) => new DistributionReleaseBuildError({ cause, code: "archive" }),
		});
		const artifact_file_name = `artisan-${configuration.product_version}-windows-x64.zip`;
		const artifact_digest = createHash("sha256").update(archive_bytes).digest("hex");
		const manifest = yield* Schema.decodeUnknownEffect(ReleaseManifest)({
			artifacts: [
				{
					architecture: "x64",
					archive_entries: entries.map((entry) => entry.path),
					archive_format: "zip",
					artifact_id: `windows-x64-${configuration.product_version}`,
					byte_size: archive_bytes.byteLength,
					file_name: artifact_file_name,
					platform: "windows",
					sha256: artifact_digest,
				},
			],
			channel: configuration.channel,
			editor_forge_compatibility_version: configuration.product_version,
			format_version: 1,
			minimum_installer_version: configuration.minimum_installer_version,
			minimum_cli_version: configuration.minimum_cli_version,
			product_version: configuration.product_version,
			signing_identity: {
				algorithm: "ed25519",
				key_id: configuration.key_id,
			},
		}).pipe(
			Effect.mapError(
				(cause) => new DistributionReleaseBuildError({ cause, code: "manifest" }),
			),
		);
		const manifest_bytes = text_encoder.encode(CanonicalJson(manifest));
		const signing = yield* Effect.try({
			try: () => {
				const private_key = createPrivateKey(configuration.private_key_pem);
				return {
					public_key_der: new Uint8Array(
						createPublicKey(configuration.private_key_pem).export({
							format: "der",
							type: "spki",
						}),
					),
					signature: sign(null, manifest_bytes, private_key),
				};
			},
			catch: (cause) => new DistributionReleaseBuildError({ cause, code: "signing" }),
		});
		const signature = yield* Schema.decodeUnknownEffect(ReleaseManifestSignature)({
			algorithm: "ed25519",
			key_id: configuration.key_id,
			signature: signing.signature.toString("base64"),
		}).pipe(
			Effect.mapError(
				(cause) => new DistributionReleaseBuildError({ cause, code: "signing" }),
			),
		);
		const signature_bytes = text_encoder.encode(CanonicalJson(signature));
		const archive_path = resolve(configuration.output_root, artifact_file_name);
		const manifest_path = resolve(configuration.output_root, "release-manifest.json");
		const signature_path = resolve(configuration.output_root, "release-manifest.sig");
		yield* Effect.tryPromise({
			try: async () => {
				await mkdir(configuration.output_root, { recursive: true });
				await Promise.all([
					writeFile(archive_path, archive_bytes),
					writeFile(manifest_path, manifest_bytes),
					writeFile(signature_path, signature_bytes),
				]);
				const [written_archive, written_manifest, written_signature] = await Promise.all([
					readFile(archive_path),
					readFile(manifest_path),
					readFile(signature_path, "utf8"),
				]);
				if (
					written_archive.byteLength !== archive_bytes.byteLength ||
					createHash("sha256").update(written_archive).digest("hex") !== artifact_digest
				)
					throw new Error("Written archive failed size or SHA-256 verification");
				if (
					!written_manifest.equals(Buffer.from(manifest_bytes)) ||
					(
						JSON.parse(written_signature) as {
							readonly signature?: string;
						}
					).signature !== signing.signature.toString("base64") ||
					!verify(
						null,
						written_manifest,
						createPublicKey(configuration.private_key_pem),
						signing.signature,
					)
				)
					throw new Error("Written manifest failed Ed25519 verification");
			},
			catch: (cause) => new DistributionReleaseBuildError({ cause, code: "write" }),
		});
		return {
			archive_entries: entries.map((entry) => entry.path),
			archive_path,
			manifest_path,
			public_key_der: signing.public_key_der,
			signature_path,
		} satisfies DistributionReleaseOutput;
	});

const LoadSigningKey = (environment: NodeJS.ProcessEnv) =>
	Effect.gen(function* () {
		if (environment.ARTISAN_RELEASE_SIGNING_KEY_PEM)
			return environment.ARTISAN_RELEASE_SIGNING_KEY_PEM;
		const key_path = environment.ARTISAN_RELEASE_SIGNING_KEY_FILE;
		if (!key_path)
			return yield* Effect.fail(
				new DistributionReleaseBuildError({
					cause: "Set ARTISAN_RELEASE_SIGNING_KEY_PEM or ARTISAN_RELEASE_SIGNING_KEY_FILE",
					code: "configuration",
				}),
			);
		return yield* Effect.tryPromise({
			try: () => readFile(key_path, "utf8"),
			catch: (cause) => new DistributionReleaseBuildError({ cause, code: "configuration" }),
		});
	});

export const BuildWindowsDistributionReleaseFromEnvironment = (environment: NodeJS.ProcessEnv) =>
	Effect.gen(function* () {
		if (environment.ARTISAN_RELEASE_ARCHITECTURE === "arm64")
			return yield* Effect.fail(
				new DistributionReleaseBuildError({
					cause: "Windows arm64 release artifacts are not supported",
					code: "configuration",
				}),
			);
		const private_key_pem = yield* LoadSigningKey(environment);
		const channel = yield* Schema.decodeUnknownEffect(
			Schema.Literals(["stable", "beta", "nightly"]),
		)(environment.ARTISAN_RELEASE_CHANNEL ?? "stable").pipe(
			Effect.mapError(
				(cause) => new DistributionReleaseBuildError({ cause, code: "configuration" }),
			),
		);
		return yield* BuildWindowsDistributionRelease({
			architecture: "x64",
			channel,
			editor_root: resolve(".dist/electron-release/win-unpacked"),
			forge_root: resolve(".dist/forge"),
			key_id: environment.ARTISAN_RELEASE_SIGNING_KEY_ID ?? "",
			minimum_installer_version: environment.ARTISAN_MINIMUM_INSTALLER_VERSION ?? "0.1.0",
			minimum_cli_version: environment.ARTISAN_MINIMUM_CLI_VERSION ?? "0.1.0",
			native_installer_path: resolve("target/release/ae-installer.exe"),
			native_cli_path: resolve("target/release/ae.exe"),
			output_root: resolve(".dist/distribution-release"),
			private_key_pem,
			product_version: environment.ARTISAN_RELEASE_VERSION ?? "0.1.0",
		});
	});
