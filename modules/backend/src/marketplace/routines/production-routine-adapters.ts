import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { lstat, mkdir, open, realpath, readdir, rename, rm, writeFile } from "node:fs/promises";
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from "node:path";

import { EngineProcessFactory } from "@artisan/engines";
import {
	EngineCompatibility,
	MarketplacePermission,
	NpxSkillsDiscoveryResult,
	RoutineCommand,
} from "@artisan/protocol";
import { Effect, Layer, Schema } from "effect";

import {
	NpxSkillsAdapter,
	RoutineInspectionSchema,
	type RoutineInspection,
	RoutineInspectorError,
	RoutineInstaller,
	RoutineInstallerError,
	RoutineSourceInspector,
} from "./routine-adapters";

const Manifest = Schema.Struct({
	author: Schema.optional(Schema.NonEmptyString),
	compatibility: Schema.optional(Schema.Array(EngineCompatibility)),
	description: Schema.optional(Schema.NonEmptyString),
	display_name: Schema.optional(Schema.NonEmptyString),
	exported_commands: Schema.optional(Schema.Array(RoutineCommand)),
	permissions: Schema.optional(Schema.Array(MarketplacePermission)),
	version: Schema.optional(Schema.NonEmptyString),
});

const NpxOutput = Schema.Struct({
	candidates: NpxSkillsDiscoveryResult.fields.candidates,
});

export interface LocalRoutineInspectorOptions {
	readonly delegate?: typeof RoutineSourceInspector.Service;
	readonly max_file_bytes?: number;
	readonly max_files?: number;
	readonly max_total_bytes?: number;
}

export interface NpxSkillsProcessOptions {
	/** Explicit executable. It is invoked directly, never through a shell. */
	readonly command: string;
	/** Fixed arguments placed before the package specifier. Include `--no-install`. */
	readonly args: ReadonlyArray<string>;
	readonly max_output_bytes?: number;
	readonly timeout_ms?: number;
}

const Hash = (value: Uint8Array | string) => createHash("sha256").update(value).digest("hex");

const IsWithin = (root: string, candidate: string) => {
	const path = relative(root, candidate);
	return path === "" || (!path.startsWith(`..${sep}`) && path !== "..");
};

const Frontmatter = (content: string) => {
	if (!content.startsWith("---\n")) return {};
	const end = content.indexOf("\n---\n", 4);
	if (end < 0) return {};
	const entries = content
		.slice(4, end)
		.split("\n")
		.map((line) => line.match(/^([a-zA-Z_][\w-]*):\s*(.+)$/))
		.filter((match): match is RegExpMatchArray => match !== null)
		.map((match) => [match[1]!, match[2]!.replace(/^['"]|['"]$/g, "")] as const);
	return Object.fromEntries(entries) as Readonly<Record<string, string>>;
};

const ReadBounded = (root: string, path: string, max_file_bytes: number) =>
	Effect.tryPromise({
		try: async () => {
			const before = await lstat(path);
			if (!before.isFile() || before.isSymbolicLink()) throw new Error("invalid file");
			const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
			try {
				const opened = await handle.stat();
				const canonical = await realpath(path);
				const after = await lstat(path);
				if (
					!opened.isFile() ||
					after.isSymbolicLink() ||
					!after.isFile() ||
					opened.dev !== before.dev ||
					opened.ino !== before.ino ||
					opened.dev !== after.dev ||
					opened.ino !== after.ino ||
					!IsWithin(root, canonical) ||
					opened.size > max_file_bytes
				)
					throw new Error("file identity changed");
				const bytes = new Uint8Array(await handle.readFile());
				if (bytes.byteLength > max_file_bytes) throw new Error("file exceeded bound");
				return bytes;
			} finally {
				await handle.close();
			}
		},
		catch: () => new RoutineInspectorError({ code: "read_failed" }),
	});

/**
 * Inspects an explicit local file or directory without writing. Every traversed entry must be a
 * regular file/directory and its real path must remain below the reviewed root.
 */
export const make_local_routine_source_inspector_layer = (
	options: LocalRoutineInspectorOptions = {},
) =>
	Layer.succeed(RoutineSourceInspector, {
		Inspect: ({ scope, source }) =>
			Effect.gen(function* () {
				if (source.kind !== "local")
					return options.delegate === undefined
						? yield* new RoutineInspectorError({ code: "unsupported" })
						: yield* options.delegate.Inspect({ scope, source });
				if (!isAbsolute(source.locator))
					return yield* new RoutineInspectorError({ code: "invalid_source" });
				const max_file_bytes = options.max_file_bytes ?? 1_048_576;
				const max_files = options.max_files ?? 256;
				const max_total_bytes = options.max_total_bytes ?? 8_388_608;
				if (max_file_bytes <= 0 || max_files <= 0 || max_total_bytes <= 0)
					return yield* new RoutineInspectorError({ code: "invalid_source" });
				const locator = resolve(source.locator);
				const root_stat = yield* Effect.tryPromise({
					try: () => lstat(locator),
					catch: () => new RoutineInspectorError({ code: "not_found" }),
				});
				if (root_stat.isSymbolicLink() || (!root_stat.isDirectory() && !root_stat.isFile()))
					return yield* new RoutineInspectorError({ code: "invalid_source" });
				const root = yield* Effect.tryPromise({
					try: () => realpath(root_stat.isDirectory() ? locator : dirname(locator)),
					catch: () => new RoutineInspectorError({ code: "read_failed" }),
				});
				const files: Array<string> = [];
				const Visit = (path: string): Effect.Effect<void, RoutineInspectorError> =>
					Effect.gen(function* () {
						const stat = yield* Effect.tryPromise({
							try: () => lstat(path),
							catch: () => new RoutineInspectorError({ code: "read_failed" }),
						});
						if (stat.isSymbolicLink())
							return yield* new RoutineInspectorError({ code: "invalid_source" });
						const canonical = yield* Effect.tryPromise({
							try: () => realpath(path),
							catch: () => new RoutineInspectorError({ code: "read_failed" }),
						});
						if (!IsWithin(root, canonical))
							return yield* new RoutineInspectorError({ code: "invalid_source" });
						if (stat.isFile()) {
							files.push(path);
							if (files.length > max_files)
								return yield* new RoutineInspectorError({ code: "read_failed" });
							return;
						}
						if (!stat.isDirectory())
							return yield* new RoutineInspectorError({ code: "invalid_source" });
						const children = yield* Effect.tryPromise({
							try: () => readdir(path),
							catch: () => new RoutineInspectorError({ code: "read_failed" }),
						});
						for (const child of children.sort()) yield* Visit(join(path, child));
					});
				yield* Visit(locator);
				const skill_path = root_stat.isFile() ? locator : join(locator, "SKILL.md");
				if (!files.includes(skill_path))
					return yield* new RoutineInspectorError({ code: "invalid_source" });
				let total = 0;
				const content_hashes: Record<string, string> = {};
				for (const path of files) {
					const bytes = yield* ReadBounded(root, path, max_file_bytes);
					total += bytes.byteLength;
					if (total > max_total_bytes)
						return yield* new RoutineInspectorError({ code: "read_failed" });
					content_hashes[relative(root, path).replaceAll("\\", "/")] = Hash(bytes);
				}
				const instructions = yield* ReadBounded(root, skill_path, max_file_bytes).pipe(
					Effect.flatMap((bytes) =>
						Effect.try({
							try: () => new TextDecoder("utf-8", { fatal: true }).decode(bytes),
							catch: () => new RoutineInspectorError({ code: "invalid_source" }),
						}),
					),
				);
				const frontmatter = Frontmatter(instructions);
				const manifest_path = root_stat.isDirectory()
					? join(locator, "artisan-routine.json")
					: join(dirname(locator), "artisan-routine.json");
				const manifest = files.includes(manifest_path)
					? yield* ReadBounded(root, manifest_path, max_file_bytes).pipe(
							Effect.flatMap((bytes) =>
								Effect.try({
									try: () =>
										new TextDecoder("utf-8", { fatal: true }).decode(bytes),
									catch: () =>
										new RoutineInspectorError({ code: "invalid_source" }),
								}),
							),
							Effect.flatMap(
								Schema.decodeUnknownEffect(Schema.UnknownFromJsonString),
							),
							Effect.flatMap(
								Schema.decodeUnknownEffect(Manifest, { onExcessProperty: "error" }),
							),
							Effect.mapError(
								() => new RoutineInspectorError({ code: "invalid_source" }),
							),
						)
					: {};
				const display_name = manifest.display_name ?? frontmatter.name ?? basename(root);
				const result = {
					artifact_refs: Object.entries(content_hashes).map(
						([path, hash]) => `sha256:${hash}:${path}`,
					),
					...(manifest.author === undefined ? {} : { author: manifest.author }),
					candidate_id: `routine_${Hash(`${root}\0${JSON.stringify(content_hashes)}`).slice(0, 24)}`,
					compatibility: manifest.compatibility ?? [],
					content_hashes,
					description: manifest.description ?? frontmatter.description ?? display_name,
					display_name,
					exported_commands: manifest.exported_commands ?? [],
					files: Object.keys(content_hashes).map((path) => ({
						path,
						required: path === "SKILL.md",
					})),
					instructions,
					permissions: manifest.permissions ?? [],
					rollback_available: true,
					source,
					trust: "local" as const,
					version: manifest.version ?? frontmatter.version ?? "0.0.0-local",
				};
				return yield* Schema.decodeUnknownEffect(RoutineInspectionSchema)(result).pipe(
					Effect.mapError(() => new RoutineInspectorError({ code: "invalid_source" })),
					Effect.map((decoded): RoutineInspection => {
						const { author, ...required } = decoded;
						return author === undefined ? required : { ...required, author };
					}),
				);
			}),
	});

/** Test-only receipt adapter. Production desktop composition uses the atomic local installer. */
export const DeterministicRoutineInstallerTestLive = Layer.succeed(RoutineInstaller, {
	Install: ({ inspection, operation_id }) =>
		Effect.succeed({
			artifact_refs: inspection.artifact_refs,
			rollback_id: `rollback_${Hash(`${operation_id}\0${inspection.candidate_id}`).slice(0, 24)}`,
		}),
	Rollback: ({ rollback_id }) =>
		rollback_id.startsWith("rollback_")
			? Effect.void
			: Effect.fail(new RoutineInstallerError({ code: "rollback_failed" })),
});

export interface LocalRoutineInstallerOptions {
	readonly install_root: string;
	readonly max_file_bytes?: number;
	readonly max_files?: number;
	readonly max_total_bytes?: number;
}

const SafeRelativePath = (path: string) => {
	if (path.length === 0 || isAbsolute(path) || path.includes("\0")) return false;
	const normalized = path.replaceAll("\\", "/");
	const segments = normalized.split("/");
	return segments.every((segment) => segment.length > 0 && segment !== "." && segment !== "..");
};

const InstallerError = (code: RoutineInstallerError["code"]) => new RoutineInstallerError({ code });

/** Atomic local materialization. Acquisition is inert; all filesystem effects are explicit. */
export const make_local_routine_installer_layer = (options: LocalRoutineInstallerOptions) =>
	Layer.succeed(RoutineInstaller, {
		Install: ({ inspection, operation_id, scope }) =>
			Effect.tryPromise({
				try: async () => {
					if (inspection.source.kind !== "local" || !isAbsolute(options.install_root))
						throw InstallerError("install_failed");
					const paths = Object.keys(inspection.content_hashes).sort();
					const max_files = options.max_files ?? 256;
					const max_file_bytes = options.max_file_bytes ?? 1_048_576;
					const max_total_bytes = options.max_total_bytes ?? 8_388_608;
					if (
						paths.length === 0 ||
						paths.length > max_files ||
						paths.some((path) => !SafeRelativePath(path))
					)
						throw InstallerError("conflict");
					const source_locator = resolve(inspection.source.locator);
					const source_stat = await lstat(source_locator);
					if (source_stat.isSymbolicLink()) throw InstallerError("conflict");
					const source_root = await realpath(
						source_stat.isDirectory() ? source_locator : dirname(source_locator),
					);
					const materialized = new Map<string, Uint8Array>();
					let total = 0;
					for (const path of paths) {
						const bytes = await Effect.runPromise(
							ReadBounded(
								source_root,
								join(source_root, ...path.split("/")),
								max_file_bytes,
							),
						);
						total += bytes.byteLength;
						if (
							total > max_total_bytes ||
							Hash(bytes) !== inspection.content_hashes[path]
						)
							throw InstallerError("conflict");
						materialized.set(path, bytes);
					}
					const fingerprint = Hash(
						JSON.stringify({ content_hashes: inspection.content_hashes, scope }),
					);
					const target_name = Hash(
						`${inspection.candidate_id}\0${JSON.stringify(scope)}`,
					).slice(0, 32);
					const rollback_id = `rollback_${target_name}_${fingerprint}`;
					const receipt = {
						artifact_refs: inspection.artifact_refs,
						fingerprint,
						operation_id,
						rollback_id,
						target_name,
					};
					const root = resolve(options.install_root);
					const target = join(root, target_name);
					const stage = join(root, `.stage-${Hash(operation_id).slice(0, 32)}`);
					await mkdir(root, { recursive: true });
					const root_stat = await lstat(root);
					if (!root_stat.isDirectory() || root_stat.isSymbolicLink())
						throw InstallerError("install_failed");
					await rm(stage, { force: true, recursive: true });
					try {
						const existing_stat = await lstat(target);
						if (!existing_stat.isDirectory() || existing_stat.isSymbolicLink())
							throw InstallerError("conflict");
						const existing = JSON.parse(
							new TextDecoder().decode(
								await Effect.runPromise(
									ReadBounded(
										await realpath(target),
										join(target, ".artisan-install.json"),
										65_536,
									),
								),
							),
						) as typeof receipt;
						if (
							existing.operation_id !== operation_id ||
							existing.fingerprint !== fingerprint ||
							existing.rollback_id !== rollback_id
						)
							throw InstallerError("conflict");
						const existing_root = await realpath(target);
						for (const path of paths) {
							const bytes = await Effect.runPromise(
								ReadBounded(
									existing_root,
									join(target, ...path.split("/")),
									max_file_bytes,
								),
							);
							if (Hash(bytes) !== inspection.content_hashes[path])
								throw InstallerError("conflict");
						}
						return { artifact_refs: existing.artifact_refs, rollback_id };
					} catch (cause) {
						if (
							!(cause instanceof Error && "code" in cause && cause.code === "ENOENT")
						) {
							if (cause instanceof RoutineInstallerError) throw cause;
							throw InstallerError("conflict");
						}
					}
					await mkdir(stage);
					try {
						for (const [path, bytes] of materialized) {
							const destination = join(stage, ...path.split("/"));
							await mkdir(dirname(destination), { recursive: true });
							await writeFile(destination, bytes, { flag: "wx" });
						}
						await writeFile(
							join(stage, ".artisan-install.json"),
							JSON.stringify(receipt),
							{
								flag: "wx",
							},
						);
						await rename(stage, target);
					} catch (cause) {
						await rm(stage, { force: true, recursive: true });
						throw cause;
					}
					return { artifact_refs: inspection.artifact_refs, rollback_id };
				},
				catch: (cause) =>
					cause instanceof RoutineInstallerError
						? cause
						: InstallerError("install_failed"),
			}),
		Rollback: ({ rollback_id }) =>
			Effect.tryPromise({
				try: async () => {
					const match = rollback_id.match(/^rollback_([a-f0-9]{32})_([a-f0-9]{64})$/);
					if (!match) throw InstallerError("rollback_failed");
					const root = resolve(options.install_root);
					const target = join(root, match[1]!);
					const marker_root = join(root, ".rollbacks");
					const marker = join(marker_root, `${Hash(rollback_id)}.json`);
					const trash = join(root, `.rollback-${Hash(rollback_id).slice(0, 32)}`);
					await mkdir(marker_root, { recursive: true });
					const root_stat = await lstat(root);
					const marker_root_stat = await lstat(marker_root);
					if (!root_stat.isDirectory() || root_stat.isSymbolicLink())
						throw InstallerError("rollback_failed");
					if (!marker_root_stat.isDirectory() || marker_root_stat.isSymbolicLink())
						throw InstallerError("rollback_failed");
					try {
						await lstat(marker);
						const marker_payload = JSON.parse(
							new TextDecoder().decode(
								await Effect.runPromise(
									ReadBounded(await realpath(marker_root), marker, 4_096),
								),
							),
						) as { readonly rollback_id?: string };
						if (marker_payload.rollback_id !== rollback_id)
							throw InstallerError("rollback_failed");
						await rm(trash, { force: true, recursive: true });
						return;
					} catch (cause) {
						if (!(cause instanceof Error && "code" in cause && cause.code === "ENOENT"))
							throw cause;
					}
					try {
						const target_root = await realpath(target);
						const target_stat = await lstat(target);
						if (
							!target_stat.isDirectory() ||
							target_stat.isSymbolicLink() ||
							!IsWithin(root, target_root)
						)
							throw InstallerError("rollback_failed");
						const receipt = JSON.parse(
							new TextDecoder().decode(
								await Effect.runPromise(
									ReadBounded(
										target_root,
										join(target, ".artisan-install.json"),
										65_536,
									),
								),
							),
						) as { readonly rollback_id?: string };
						if (receipt.rollback_id !== rollback_id)
							throw InstallerError("rollback_failed");
						await rename(target, trash);
					} catch (cause) {
						if (!(cause instanceof Error && "code" in cause && cause.code === "ENOENT"))
							throw cause;
						try {
							await lstat(trash);
						} catch {
							throw InstallerError("rollback_failed");
						}
					}
					await writeFile(marker, JSON.stringify({ rollback_id }), { flag: "wx" });
					await rm(trash, { recursive: true });
				},
				catch: (cause) =>
					cause instanceof RoutineInstallerError
						? cause
						: InstallerError("rollback_failed"),
			}),
	});

const ReadProcessStream = (stream: AsyncIterable<Uint8Array>, max_bytes: number) =>
	Effect.tryPromise({
		try: async () => {
			const chunks: Array<Uint8Array> = [];
			let total = 0;
			for await (const chunk of stream) {
				total += chunk.byteLength;
				if (total > max_bytes) throw new Error("output bound exceeded");
				chunks.push(chunk);
			}
			const output = new Uint8Array(total);
			let offset = 0;
			for (const chunk of chunks) {
				output.set(chunk, offset);
				offset += chunk.byteLength;
			}
			return output;
		},
		catch: () => new RoutineInspectorError({ code: "read_failed" }),
	});

/** Explicit, bounded argv-only `npx skills` discovery. Layer acquisition never spawns. */
export const make_npx_skills_process_adapter_layer = (options: NpxSkillsProcessOptions) =>
	Layer.effect(
		NpxSkillsAdapter,
		Effect.gen(function* () {
			const factory = yield* EngineProcessFactory;
			return {
				Discover: (input) =>
					Effect.scoped(
						Effect.gen(function* () {
							if (
								!options.args.includes("--no-install") ||
								options.command.length === 0
							)
								return yield* new RoutineInspectorError({ code: "invalid_source" });
							const process = yield* factory
								.Spawn({
									args: [...options.args, input.package_spec],
									command: options.command,
								})
								.pipe(
									Effect.mapError(
										() => new RoutineInspectorError({ code: "read_failed" }),
									),
								);
							yield* Effect.addFinalizer(() => process.Close.pipe(Effect.ignore));
							const max = options.max_output_bytes ?? 1_048_576;
							const [stdout, _stderr, exit] = yield* Effect.all(
								[
									ReadProcessStream(process.Stdout, max),
									ReadProcessStream(process.Stderr, max),
									process.Exit.pipe(
										Effect.mapError(
											() =>
												new RoutineInspectorError({ code: "read_failed" }),
										),
									),
								],
								{ concurrency: "unbounded" },
							).pipe(
								Effect.timeout(options.timeout_ms ?? 15_000),
								Effect.mapError(
									() => new RoutineInspectorError({ code: "read_failed" }),
								),
							);
							if (exit.code !== 0)
								return yield* new RoutineInspectorError({ code: "read_failed" });
							const decoded = yield* Effect.try({
								try: () => new TextDecoder("utf-8", { fatal: true }).decode(stdout),
								catch: () => new RoutineInspectorError({ code: "invalid_source" }),
							}).pipe(
								Effect.flatMap(
									Schema.decodeUnknownEffect(Schema.UnknownFromJsonString),
								),
								Effect.flatMap(
									Schema.decodeUnknownEffect(NpxOutput, {
										onExcessProperty: "error",
									}),
								),
								Effect.mapError(
									() => new RoutineInspectorError({ code: "invalid_source" }),
								),
							);
							return yield* Schema.decodeUnknownEffect(NpxSkillsDiscoveryResult)({
								candidates: decoded.candidates,
								package_spec: input.package_spec,
							}).pipe(
								Effect.mapError(
									() => new RoutineInspectorError({ code: "invalid_source" }),
								),
							);
						}),
					),
			};
		}),
	);
