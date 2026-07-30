import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { createReadStream, createWriteStream } from "node:fs";
import { pipeline } from "node:stream/promises";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Context, Data, Effect, FileSystem, Layer, Path, Schema } from "effect";
import { fromBuffer, type Entry, type ZipFile } from "yauzl";

import { ReleaseChannel, SafeArchivePath, SemanticVersion } from "./common";
import {
	ArtifactDownloadFailure,
	ArtifactDownloader,
	ArtifactStager,
	ArtifactStagingFailure,
	ForgeUpdateLifecycle,
	ForgeUpdateLifecycleFailure,
	InstallationHealth,
	InstallationHealthFailure,
	ReleaseSource,
	ReleaseSourceFailure,
	type ReleaseEnvelope,
} from "./installer";
import {
	MaximumArchiveEntries,
	MaximumArchiveEntryBytes,
	MaximumExpandedArtifactBytes,
} from "./release-manifest";

const MaximumRedirects = 5;
const MaximumManifestBytes = 1024 * 1024;
const MaximumProcessOutputBytes = 1024 * 1024;
const ForgeStatusJson = Schema.Struct({
	state: Schema.Literals(["missing", "running", "stale"]),
});
const ForgeDoctorJson = Schema.Struct({
	distribution: Schema.Struct({ healthy: Schema.Boolean }),
	forge: Schema.Struct({ healthy: Schema.Boolean }),
});

const GithubRepository = Schema.Struct({
	owner: Schema.String.check(Schema.isPattern(/^[A-Za-z0-9_.-]+$/)),
	repository: Schema.String.check(Schema.isPattern(/^[A-Za-z0-9_.-]+$/)),
});
export type GithubRepository = typeof GithubRepository.Type;

const HttpResult = Schema.Struct({
	status: Schema.Int,
	url: Schema.String,
	bytes: Schema.Uint8Array,
});
export type HttpResult = typeof HttpResult.Type;

export class SecureReleaseHttpFailure extends Data.TaggedError("SecureReleaseHttpFailure")<{
	readonly cause?: unknown;
	readonly code: "body_too_large" | "disallowed_url" | "http_status" | "network" | "redirect";
	readonly url: string;
}> {}

export class SecureReleaseHttp extends Context.Service<
	SecureReleaseHttp,
	{
		readonly Get: (
			url: string,
			maximum_bytes: number,
		) => Effect.Effect<HttpResult, SecureReleaseHttpFailure>;
	}
>()("Artisan/Distribution/SecureReleaseHttp") {}

const IsAllowedGithubUrl = (value: string) =>
	Effect.try({
		try: () => {
			const url = new URL(value);
			const hostname = url.hostname.toLowerCase();
			if (
				url.protocol !== "https:" ||
				!(
					hostname === "github.com" ||
					hostname === "api.github.com" ||
					hostname === "objects.githubusercontent.com" ||
					hostname.endsWith(".githubusercontent.com")
				)
			)
				throw new Error("Release URL is outside the trusted GitHub HTTPS boundary");
			if (url.username !== "" || url.password !== "")
				throw new Error("Release URLs must not contain credentials");
			return url;
		},
		catch: (cause) =>
			new SecureReleaseHttpFailure({
				cause,
				code: "disallowed_url",
				url: value,
			}),
	});

const ReadBoundedResponse = (response: Response, maximum_bytes: number) =>
	Effect.gen(function* () {
		const declared_length = response.headers.get("content-length");
		if (
			declared_length !== null &&
			Number.isSafeInteger(Number(declared_length)) &&
			Number(declared_length) > maximum_bytes
		)
			return yield* new SecureReleaseHttpFailure({
				code: "body_too_large",
				url: response.url,
			});

		const reader = response.body?.getReader();
		if (reader === undefined) return new Uint8Array();
		const chunks: Array<Uint8Array> = [];
		let received = 0;
		while (true) {
			const next = yield* Effect.tryPromise({
				try: () => reader.read(),
				catch: (cause) =>
					new SecureReleaseHttpFailure({
						cause,
						code: "network",
						url: response.url,
					}),
			});
			if (next.done) break;
			received += next.value.byteLength;
			if (received > maximum_bytes) {
				yield* Effect.promise(() => reader.cancel());
				return yield* new SecureReleaseHttpFailure({
					code: "body_too_large",
					url: response.url,
				});
			}
			chunks.push(next.value);
		}
		const bytes = new Uint8Array(received);
		let offset = 0;
		for (const chunk of chunks) {
			bytes.set(chunk, offset);
			offset += chunk.byteLength;
		}
		return bytes;
	});

/** Fetch adapter with manual, allowlisted redirects and bounded streaming bodies. */
export const NodeSecureReleaseHttpLive = Layer.succeed(
	SecureReleaseHttp,
	SecureReleaseHttp.of({
		Get: (input_url, maximum_bytes) =>
			Effect.gen(function* () {
				let url = yield* IsAllowedGithubUrl(input_url);
				for (let redirect_count = 0; redirect_count <= MaximumRedirects; redirect_count++) {
					const response = yield* Effect.tryPromise({
						try: () =>
							fetch(url, {
								headers: {
									Accept: "application/octet-stream",
									"User-Agent": "Artisan-Distribution",
								},
								redirect: "manual",
							}),
						catch: (cause) =>
							new SecureReleaseHttpFailure({
								cause,
								code: "network",
								url: url.toString(),
							}),
					});
					if ([301, 302, 303, 307, 308].includes(response.status)) {
						const location = response.headers.get("location");
						if (location === null || redirect_count === MaximumRedirects)
							return yield* new SecureReleaseHttpFailure({
								code: "redirect",
								url: url.toString(),
							});
						url = yield* IsAllowedGithubUrl(new URL(location, url).toString());
						continue;
					}
					if (!response.ok)
						return yield* new SecureReleaseHttpFailure({
							code: "http_status",
							url: url.toString(),
						});
					return {
						status: response.status,
						url: response.url,
						bytes: yield* ReadBoundedResponse(response, maximum_bytes),
					};
				}
				return yield* new SecureReleaseHttpFailure({
					code: "redirect",
					url: input_url,
				});
			}),
	}),
);

const ReleaseBaseUrl = (repository: GithubRepository, channel: ReleaseChannel) => {
	const release_path = channel === "stable" ? "latest/download" : `download/${channel}`;
	return `https://github.com/${repository.owner}/${repository.repository}/releases/${release_path}`;
};

export const make_github_release_source_layer = (repository_input: unknown) =>
	Layer.effect(
		ReleaseSource,
		Effect.gen(function* () {
			const repository =
				yield* Schema.decodeUnknownEffect(GithubRepository)(repository_input);
			const http = yield* SecureReleaseHttp;
			return ReleaseSource.of({
				Resolve: (channel) =>
					Effect.gen(function* () {
						const base_url = ReleaseBaseUrl(repository, channel);
						const manifest = yield* http.Get(
							`${base_url}/release-manifest.json`,
							MaximumManifestBytes,
						);
						const signature = yield* http.Get(
							`${base_url}/release-manifest.sig`,
							MaximumManifestBytes,
						);
						const signature_value = yield* Schema.decodeUnknownEffect(
							Schema.UnknownFromJsonString,
						)(new TextDecoder().decode(signature.bytes)).pipe(
							Effect.mapError((cause) => new ReleaseSourceFailure({ cause })),
						);
						return {
							manifest: manifest.bytes,
							signature: signature_value,
						} satisfies ReleaseEnvelope;
					}).pipe(
						Effect.mapError((cause) =>
							cause instanceof ReleaseSourceFailure
								? cause
								: new ReleaseSourceFailure({ cause }),
						),
					),
			});
		}),
	);

export const make_github_artifact_downloader_layer = (
	repository_input: unknown,
	channel: ReleaseChannel = "stable",
) =>
	Layer.effect(
		ArtifactDownloader,
		Effect.gen(function* () {
			const repository =
				yield* Schema.decodeUnknownEffect(GithubRepository)(repository_input);
			const http = yield* SecureReleaseHttp;
			return ArtifactDownloader.of({
				Download: (artifact) =>
					http
						.Get(
							`${ReleaseBaseUrl(repository, channel)}/${artifact.file_name}`,
							artifact.byte_size,
						)
						.pipe(
							Effect.map((response) => response.bytes),
							Effect.mapError(
								(cause) =>
									new ArtifactDownloadFailure({
										cause,
										artifact_id: artifact.artifact_id,
									}),
							),
						),
			});
		}),
	);

const WindowsReservedDeviceName = /^(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)/i;
const WindowsForbiddenCharacter = /[<>:"\\|?*]/;

const HasWindowsControlCharacter = (value: string) =>
	[...value].some((character) => character.charCodeAt(0) <= 0x1f);

/**
 * Produces the case-insensitive identity Windows will use for an archive path.
 * ZIP names are deliberately restricted to printable ASCII so Unicode case
 * folding cannot create platform-dependent aliases.
 */
export const CanonicalizeWindowsArchivePath = (input: string) =>
	Schema.decodeUnknownEffect(SafeArchivePath)(input).pipe(
		Effect.flatMap((path) => {
			if (!/^[\x20-\x7e]+$/.test(path) || path.includes("\\"))
				return Effect.fail(
					new Error("Windows archive paths must use printable ASCII and '/'"),
				);
			const segments = path.split("/");
			if (
				segments.some(
					(segment) =>
						segment === "" ||
						segment === "." ||
						segment === ".." ||
						segment.endsWith(".") ||
						segment.endsWith(" ") ||
						HasWindowsControlCharacter(segment) ||
						WindowsForbiddenCharacter.test(segment) ||
						WindowsReservedDeviceName.test(segment),
				)
			)
				return Effect.fail(new Error("Archive path is unsafe on Windows"));
			return Effect.succeed({
				canonical: segments.map((segment) => segment.toLowerCase()).join("/"),
				path,
				segments,
			});
		}),
	);

interface ValidatedZipEntry {
	readonly canonical: string;
	readonly compressed_size: number;
	readonly path: string;
	readonly segments: ReadonlyArray<string>;
	readonly uncompressed_size: number;
}

const IsRegularZipEntry = (entry: Entry) => {
	const unix_mode = (entry.externalFileAttributes >>> 16) & 0xffff;
	const unix_type = unix_mode & 0xf000;
	return (
		!entry.fileName.endsWith("/") &&
		(entry.generalPurposeBitFlag & 0x1) === 0 &&
		(entry.compressionMethod === 0 || entry.compressionMethod === 8) &&
		(unix_type === 0 || unix_type === 0x8000)
	);
};

const OpenZip = (bytes: Uint8Array) =>
	Effect.callback<ZipFile, Error>((resume) => {
		fromBuffer(
			Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength),
			{
				autoClose: true,
				decodeStrings: true,
				lazyEntries: true,
				strictFileNames: true,
				validateEntrySizes: true,
			},
			(cause, zip_file) => {
				if (cause !== null) resume(Effect.fail(cause));
				else if (zip_file === undefined)
					resume(Effect.fail(new Error("ZIP reader did not return an archive")));
				else resume(Effect.succeed(zip_file));
			},
		);
	});

const ReadZipEntries = (bytes: Uint8Array) =>
	Effect.gen(function* () {
		const zip_file = yield* OpenZip(bytes);
		return yield* Effect.callback<ReadonlyArray<ValidatedZipEntry>, Error>((resume) => {
			const entries: Array<ValidatedZipEntry> = [];
			const identities = new Set<string>();
			let expanded_bytes = 0;
			let settled = false;
			const Finish = (outcome: Effect.Effect<ReadonlyArray<ValidatedZipEntry>, Error>) => {
				if (settled) return;
				settled = true;
				zip_file.close();
				resume(outcome);
			};
			zip_file.on("error", (cause) => Finish(Effect.fail(cause)));
			zip_file.on("end", () => Finish(Effect.succeed(entries)));
			zip_file.on("entry", (entry) => {
				void Effect.runPromiseExit(
					Effect.gen(function* () {
						if (!IsRegularZipEntry(entry))
							return yield* Effect.fail(
								new Error("ZIP contains a non-regular entry"),
							);
						if (entries.length >= MaximumArchiveEntries)
							return yield* Effect.fail(
								new Error("ZIP entry count exceeds the safety limit"),
							);
						if (
							!Number.isSafeInteger(entry.uncompressedSize) ||
							entry.uncompressedSize < 0 ||
							entry.uncompressedSize > MaximumArchiveEntryBytes
						)
							return yield* Effect.fail(
								new Error("ZIP entry exceeds the size limit"),
							);
						expanded_bytes += entry.uncompressedSize;
						if (
							!Number.isSafeInteger(expanded_bytes) ||
							expanded_bytes > MaximumExpandedArtifactBytes
						)
							return yield* Effect.fail(
								new Error("Expanded ZIP exceeds the total safety limit"),
							);
						const normalized = yield* CanonicalizeWindowsArchivePath(entry.fileName);
						if (identities.has(normalized.canonical))
							return yield* Effect.fail(
								new Error("ZIP contains a case-insensitive path collision"),
							);
						identities.add(normalized.canonical);
						entries.push({
							...normalized,
							compressed_size: entry.compressedSize,
							uncompressed_size: entry.uncompressedSize,
						});
					}),
				).then((exit) => {
					if (exit._tag === "Failure") {
						Finish(Effect.fail(new Error(String(exit.cause))));
						return;
					}
					if (!settled) zip_file.readEntry();
				});
			});
			zip_file.readEntry();
			return Effect.sync(() => {
				settled = true;
				zip_file.close();
			});
		});
	});

const OpenEntryStream = (zip_file: ZipFile, entry: Entry) =>
	Effect.callback<NodeJS.ReadableStream, Error>((resume) => {
		zip_file.openReadStream(entry, (cause, stream) => {
			if (cause !== null) resume(Effect.fail(cause));
			else if (stream === undefined)
				resume(Effect.fail(new Error("ZIP reader did not return an entry stream")));
			else resume(Effect.succeed(stream));
		});
	});

const ExtractValidatedZip = (
	bytes: Uint8Array,
	expected_entries: ReadonlyMap<string, ValidatedZipEntry>,
	staging_path: string,
	path_service: Path.Path,
) =>
	Effect.gen(function* () {
		const zip_file = yield* OpenZip(bytes);
		return yield* Effect.callback<void, Error>((resume) => {
			let settled = false;
			let extracted = 0;
			const Finish = (outcome: Effect.Effect<void, Error>) => {
				if (settled) return;
				settled = true;
				zip_file.close();
				resume(outcome);
			};
			zip_file.on("error", (cause) => Finish(Effect.fail(cause)));
			zip_file.on("end", () =>
				Finish(
					extracted === expected_entries.size
						? Effect.void
						: Effect.fail(
								new Error("ZIP extraction ended before every entry was written"),
							),
				),
			);
			zip_file.on("entry", (entry) => {
				void Effect.runPromiseExit(
					Effect.gen(function* () {
						const normalized = yield* CanonicalizeWindowsArchivePath(entry.fileName);
						const expected = expected_entries.get(normalized.canonical);
						if (
							expected === undefined ||
							expected.path !== normalized.path ||
							expected.uncompressed_size !== entry.uncompressedSize ||
							expected.compressed_size !== entry.compressedSize ||
							!IsRegularZipEntry(entry)
						)
							return yield* Effect.fail(
								new Error("ZIP changed between validation and extraction"),
							);
						const source = yield* OpenEntryStream(zip_file, entry);
						const destination = path_service.join(staging_path, ...expected.segments);
						yield* Effect.tryPromise({
							try: () =>
								pipeline(
									source,
									createWriteStream(destination, {
										flags: "wx",
									}),
								),
							catch: (cause) => cause as Error,
						});
						extracted += 1;
					}),
				).then((exit) => {
					if (exit._tag === "Failure") {
						Finish(Effect.fail(new Error(String(exit.cause))));
						return;
					}
					if (!settled) zip_file.readEntry();
				});
			});
			zip_file.readEntry();
			return Effect.sync(() => {
				settled = true;
				zip_file.close();
			});
		});
	});

const HashFile = (path: string) =>
	Effect.tryPromise({
		try: async () => {
			const digest = createHash("sha256");
			for await (const chunk of createReadStream(path)) digest.update(chunk);
			return digest.digest("hex");
		},
		catch: (cause) => cause,
	});

const ListRegularFiles = (
	root: string,
	file_system: FileSystem.FileSystem,
	path_service: Path.Path,
) =>
	Effect.gen(function* () {
		const found: Array<string> = [];
		const Visit = (directory: string, prefix: string): Effect.Effect<void, unknown> =>
			Effect.gen(function* () {
				for (const name of yield* file_system.readDirectory(directory)) {
					const absolute = path_service.join(directory, name);
					const relative = prefix === "" ? name : `${prefix}/${name}`;
					const metadata = yield* file_system.stat(absolute);
					if (metadata.type === "Directory") yield* Visit(absolute, relative);
					else if (metadata.type === "File") {
						if (found.length >= MaximumArchiveEntries)
							return yield* Effect.fail(
								new Error("Staged layout exceeds the file-count limit"),
							);
						found.push(relative);
					} else
						return yield* Effect.fail(new Error("Staged layout contains a non-file"));
				}
			});
		yield* Visit(root, "");
		return found.sort();
	});

/** ZIP-only production stager with an exact manifest allowlist and atomic publish. */
export const NodeZipArtifactStagerLive = Layer.effect(
	ArtifactStager,
	Effect.gen(function* () {
		const file_system = yield* FileSystem.FileSystem;
		const path_service = yield* Path.Path;
		return ArtifactStager.of({
			Stage: (request) =>
				Effect.gen(function* () {
					if (request.artifact.archive_format !== "zip")
						return yield* Effect.fail(new Error("Windows stager requires ZIP"));
					const declared = yield* Effect.forEach(
						request.artifact.archive_entries,
						CanonicalizeWindowsArchivePath,
					);
					const allowed = new Map<string, string>();
					for (const entry of declared) {
						if (allowed.has(entry.canonical))
							return yield* Effect.fail(
								new Error("Manifest contains a case-insensitive path collision"),
							);
						allowed.set(entry.canonical, entry.path);
					}
					const entries = yield* ReadZipEntries(request.bytes);
					const validated = new Map(
						entries.map((entry) => [entry.canonical, entry] as const),
					);
					if (
						validated.size !== allowed.size ||
						entries.some((entry) => allowed.get(entry.canonical) !== entry.path)
					)
						return yield* Effect.fail(
							new Error("ZIP entries do not match the release allowlist"),
						);
					yield* file_system
						.remove(request.staging_path, { recursive: true })
						.pipe(Effect.ignore);
					yield* file_system.makeDirectory(request.staging_path, { recursive: true });
					for (const entry of entries) {
						const destination = path_service.join(
							request.staging_path,
							...entry.segments,
						);
						const parent = path_service.dirname(destination);
						yield* file_system.makeDirectory(parent, { recursive: true });
					}
					yield* ExtractValidatedZip(
						request.bytes,
						validated,
						request.staging_path,
						path_service,
					);
					const staged_files = yield* ListRegularFiles(
						request.staging_path,
						file_system,
						path_service,
					);
					if (
						staged_files.length !== allowed.size ||
						staged_files.some(
							(file) =>
								!allowed.has(file.toLowerCase()) ||
								allowed.get(file.toLowerCase()) !== file,
						)
					)
						return yield* Effect.fail(
							new Error("Staged layout does not match manifest"),
						);

					const target_exists = yield* file_system.exists(request.version_path);
					if (target_exists) {
						const target_files = yield* ListRegularFiles(
							request.version_path,
							file_system,
							path_service,
						);
						if (
							target_files.length !== allowed.size ||
							target_files.some(
								(file) =>
									!allowed.has(file.toLowerCase()) ||
									allowed.get(file.toLowerCase()) !== file,
							)
						)
							return yield* Effect.fail(
								new Error("Existing version does not match manifest"),
							);
						for (const relative of target_files) {
							const [expected_hash, actual_hash] = yield* Effect.all([
								HashFile(path_service.join(request.staging_path, relative)),
								HashFile(path_service.join(request.version_path, relative)),
							]);
							if (actual_hash !== expected_hash)
								return yield* Effect.fail(
									new Error("Existing version content does not match artifact"),
								);
						}
						yield* file_system.remove(request.staging_path, { recursive: true });
					} else {
						yield* file_system.makeDirectory(
							path_service.dirname(request.version_path),
							{
								recursive: true,
							},
						);
						yield* file_system.rename(request.staging_path, request.version_path);
					}
				}).pipe(
					Effect.mapError(
						(cause) =>
							new ArtifactStagingFailure({
								cause,
								target_version: request.release.product_version,
							}),
					),
					Effect.ensuring(
						file_system
							.remove(request.staging_path, { recursive: true })
							.pipe(Effect.ignore),
					),
				),
		});
	}),
);

export interface HealthProcessRequest {
	readonly argv: ReadonlyArray<string>;
	readonly cwd: string;
	readonly executable: string;
	readonly timeout_ms: number;
	readonly windows_verbatim_arguments?: boolean;
}

export interface HealthProcessResult {
	readonly exit_code: number;
	readonly stderr: string;
	readonly stdout: string;
}

export class HealthProcessFailure extends Data.TaggedError("HealthProcessFailure")<{
	readonly cause?: unknown;
	readonly code: "output_limit" | "spawn" | "timeout";
}> {}

export class HealthProcessHost extends Context.Service<
	HealthProcessHost,
	{
		readonly Run: (
			request: HealthProcessRequest,
		) => Effect.Effect<HealthProcessResult, HealthProcessFailure>;
	}
>()("Artisan/Distribution/HealthProcessHost") {}

export const NodeHealthProcessHostLive = Layer.succeed(
	HealthProcessHost,
	HealthProcessHost.of({
		Run: (request) =>
			Effect.callback<HealthProcessResult, HealthProcessFailure>((resume) => {
				const child = spawn(request.executable, [...request.argv], {
					cwd: request.cwd,
					shell: false,
					stdio: ["ignore", "pipe", "pipe"],
					windowsHide: true,
					windowsVerbatimArguments: request.windows_verbatim_arguments ?? false,
				});
				let settled = false;
				let stdout = "";
				let stderr = "";
				const Finish = (
					effect: Effect.Effect<HealthProcessResult, HealthProcessFailure>,
				) => {
					if (settled) return;
					settled = true;
					clearTimeout(timer);
					resume(effect);
				};
				const Append = (current: string, chunk: Buffer) => {
					const next = current + chunk.toString("utf8");
					if (Buffer.byteLength(next) > MaximumProcessOutputBytes) {
						child.kill();
						Finish(Effect.fail(new HealthProcessFailure({ code: "output_limit" })));
					}
					return next;
				};
				child.stdout.on("data", (chunk: Buffer) => {
					stdout = Append(stdout, chunk);
				});
				child.stderr.on("data", (chunk: Buffer) => {
					stderr = Append(stderr, chunk);
				});
				child.on("error", (cause) =>
					Finish(Effect.fail(new HealthProcessFailure({ cause, code: "spawn" }))),
				);
				child.on("close", (exit_code) =>
					Finish(
						Effect.succeed({
							exit_code: exit_code ?? -1,
							stderr,
							stdout,
						}),
					),
				);
				const timer = setTimeout(() => {
					child.kill();
					Finish(Effect.fail(new HealthProcessFailure({ code: "timeout" })));
				}, request.timeout_ms);
				return Effect.sync(() => {
					child.kill();
				});
			}),
	}),
);

export const make_node_installation_health_layer = (
	installation_root: string,
	command_interpreter = process.env.ComSpec ?? "C:\\Windows\\System32\\cmd.exe",
) =>
	Layer.effect(
		InstallationHealth,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const process_host = yield* HealthProcessHost;
			return InstallationHealth.of({
				Check: (version_input) =>
					Effect.gen(function* () {
						const version =
							yield* Schema.decodeUnknownEffect(SemanticVersion)(version_input);
						const version_path = path_service.join(
							installation_root,
							"versions",
							version,
						);
						const launcher = path_service.join(version_path, "bin", "ae.cmd");
						for (const path of [
							launcher,
							path_service.join(version_path, "forge", "ae.js"),
							path_service.join(version_path, "forge", "Artisan Forge.exe"),
							path_service.join(version_path, "editor", "Artisan Editor.exe"),
						]) {
							const metadata = yield* file_system.stat(path);
							if (metadata.type !== "File")
								return yield* Effect.fail(
									new Error(`Staged runtime entry is not a file: ${path}`),
								);
						}
						const result = yield* process_host.Run({
							argv: ["/d", "/s", "/c", `""${launcher}" --version"`],
							cwd: version_path,
							executable: command_interpreter,
							timeout_ms: 30_000,
							windows_verbatim_arguments: true,
						});
						if (result.exit_code !== 0)
							return yield* Effect.fail(new Error("Staged ae --version failed"));
						const match = /^ae v(.+)$/u.exec(result.stdout.trim());
						if (match?.[1] === undefined)
							return yield* Effect.fail(
								new Error("Staged ae --version output is not canonical"),
							);
						const reported_version = yield* Schema.decodeUnknownEffect(SemanticVersion)(
							match[1],
						);
						if (reported_version !== version)
							return yield* Effect.fail(
								new Error(
									`Staged ae reported ${reported_version}, expected ${version}`,
								),
							);
					}).pipe(
						Effect.mapError(
							(cause) =>
								new InstallationHealthFailure({
									cause,
									version: version_input,
								}),
						),
					),
			});
		}),
	);

const DecodeJsonOutput = (output: string) =>
	Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(output);

/** Coordinates the home's single Forge instance across an atomic version switch. */
export const make_node_forge_update_lifecycle_layer = (
	installation_root: string,
	command_interpreter = process.env.ComSpec ?? "C:\\Windows\\System32\\cmd.exe",
) =>
	Layer.effect(
		ForgeUpdateLifecycle,
		Effect.gen(function* () {
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const process_host = yield* HealthProcessHost;
			const config_path = path_service.join(installation_root, "config.json");

			const Configured = () => file_system.exists(config_path);

			const Run = (version: string, argv: ReadonlyArray<string>) =>
				Effect.gen(function* () {
					const version_path = path_service.join(installation_root, "versions", version);
					const launcher = path_service.join(version_path, "bin", "ae.cmd");
					const result = yield* process_host.Run({
						argv: ["/d", "/s", "/c", `""${launcher}" ${argv.join(" ")}"`],
						cwd: version_path,
						executable: command_interpreter,
						timeout_ms: 30_000,
						windows_verbatim_arguments: true,
					});
					if (result.exit_code !== 0)
						return yield* Effect.fail(
							new Error(`${argv[0] ?? "ae"} exited ${result.exit_code}`),
						);
					return result.stdout;
				});

			const Status = (version: string) =>
				Run(version, ["status", "--json"]).pipe(
					Effect.flatMap(DecodeJsonOutput),
					Effect.flatMap(Schema.decodeUnknownEffect(ForgeStatusJson)),
				);

			const ReadDoctor = (version: string) =>
				Run(version, ["doctor", "--json"]).pipe(
					Effect.flatMap(DecodeJsonOutput),
					Effect.flatMap(Schema.decodeUnknownEffect(ForgeDoctorJson)),
				);

			const RequireHealthyDistribution = (version: string) =>
				ReadDoctor(version).pipe(
					Effect.filterOrFail(
						(report) => report.distribution.healthy,
						(report) =>
							new Error(
								`Distribution doctor is unhealthy: ${JSON.stringify(report)}`,
							),
					),
					Effect.asVoid,
				);

			const StartAndVerify = (version: string, operation: "restore" | "resume") =>
				Effect.gen(function* () {
					yield* Run(version, ["start"]);
					const status = yield* Status(version);
					if (status.state !== "running")
						return yield* Effect.fail(new Error("Forge did not restart"));
					yield* RequireHealthyDistribution(version);
				}).pipe(
					Effect.mapError(
						(cause) =>
							new ForgeUpdateLifecycleFailure({
								cause,
								operation,
								version,
							}),
					),
				);

			return ForgeUpdateLifecycle.of({
				Quiesce: (version) => {
					let stopped = false;
					return Effect.gen(function* () {
						if (!(yield* Configured())) return { was_running: false };
						const status = yield* Status(version);
						if (status.state === "stale")
							return yield* Effect.fail(new Error("Forge is stale; refusing update"));
						if (status.state !== "running") return { was_running: false };
						yield* Run(version, ["stop"]);
						stopped = true;
						const verified = yield* Status(version);
						if (verified.state !== "missing")
							return yield* Effect.fail(new Error("Forge did not stop"));
						return { was_running: true };
					}).pipe(
						Effect.onError(() =>
							stopped
								? StartAndVerify(version, "restore").pipe(Effect.ignore)
								: Effect.void,
						),
						Effect.mapError(
							(cause) =>
								new ForgeUpdateLifecycleFailure({
									cause,
									operation: "quiesce",
									version,
								}),
						),
					);
				},
				Restore: (version, snapshot) =>
					snapshot.was_running ? StartAndVerify(version, "restore") : Effect.void,
				ResumeAndVerify: (version, snapshot) =>
					snapshot.was_running ? StartAndVerify(version, "resume") : Effect.void,
				VerifyCurrent: (version) =>
					Effect.gen(function* () {
						if (!(yield* Configured())) return;
						const status = yield* Status(version);
						if (status.state === "stale")
							return yield* Effect.fail(new Error("Forge is stale"));
						yield* RequireHealthyDistribution(version);
						if (status.state === "running") {
							const verified = yield* Status(version);
							if (verified.state !== "running")
								return yield* Effect.fail(new Error("Forge is not running"));
						}
					}).pipe(
						Effect.mapError(
							(cause) =>
								new ForgeUpdateLifecycleFailure({
									cause,
									operation: "verify_current",
									version,
								}),
						),
					),
			});
		}),
	);

/** Ready-to-compose Node filesystem dependencies for staging and health adapters. */
export const NodeDistributionPlatformLive = Layer.mergeAll(NodeFileSystem.layer, NodePath.layer);
