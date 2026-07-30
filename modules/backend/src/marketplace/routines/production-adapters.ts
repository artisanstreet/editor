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
import { Effect, Layer, Schema, Stream } from "effect";

import {
	NpxSkillsAdapter,
	RoutineInspectionSchema,
	type RoutineInspection,
	RoutineInspectorError,
	RoutineInstaller,
	RoutineInstallerError,
	RoutineSourceInspector,
} from "./adapters";

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

const InstallReceipt = Schema.Struct({
	artifact_refs: Schema.Array(Schema.String),
	fingerprint: Schema.String,
	operation_id: Schema.String,
	rollback_id: Schema.String,
	target_name: Schema.String,
});

const RollbackMarker = Schema.Struct({
	rollback_id: Schema.String,
});

const ErrnoError = Schema.Struct({
	code: Schema.String,
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
		.flatMap((match) => {
			const key = match[1];
			const value = match[2];
			return key === undefined || value === undefined
				? []
				: [[key, value.replace(/^['"]|['"]$/g, "")] as const];
		});
	return Object.fromEntries(entries);
};

const ReadBounded = (root: string, path: string, max_file_bytes: number) =>
	Effect.gen(function* () {
		const before = yield* Effect.tryPromise(() => lstat(path));
		if (!before.isFile() || before.isSymbolicLink()) {
			return yield* Effect.fail(new Error("invalid file"));
		}
		return yield* Effect.acquireUseRelease(
			Effect.tryPromise(() => open(path, constants.O_RDONLY | constants.O_NOFOLLOW)),
			(handle) =>
				Effect.gen(function* () {
					const [opened, canonical, after] = yield* Effect.all([
						Effect.tryPromise(() => handle.stat()),
						Effect.tryPromise(() => realpath(path)),
						Effect.tryPromise(() => lstat(path)),
					]);
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
					) {
						return yield* Effect.fail(new Error("file identity changed"));
					}
					const bytes = new Uint8Array(yield* Effect.tryPromise(() => handle.readFile()));
					if (bytes.byteLength > max_file_bytes) {
						return yield* Effect.fail(new Error("file exceeded bound"));
					}
					return bytes;
				}),
			(handle) => Effect.promise(() => handle.close()).pipe(Effect.ignore),
		);
	}).pipe(Effect.mapError(() => new RoutineInspectorError({ code: "read_failed" })));

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
const IsErrorCode = (cause: unknown, code: string) =>
	Schema.is(ErrnoError)(cause) && cause.code === code;
const InstallerFs = <A>(
	try_: () => Promise<A>,
	code: RoutineInstallerError["code"] = "install_failed",
) =>
	Effect.tryPromise({
		try: try_,
		catch: (cause) => (cause instanceof RoutineInstallerError ? cause : InstallerError(code)),
	});
const InstallerFsOptional = <A>(try_: () => Promise<A>) =>
	Effect.tryPromise({
		try: try_,
		catch: (cause) => (IsErrorCode(cause, "ENOENT") ? undefined : InstallerError("conflict")),
	}).pipe(
		Effect.matchEffect({
			onFailure: (failure) =>
				failure === undefined ? Effect.succeed(undefined) : Effect.fail(failure),
			onSuccess: (value) => Effect.succeed(value),
		}),
	);

/** Atomic local materialization. Acquisition is inert; all filesystem effects are explicit. */
export const make_local_routine_installer_layer = (options: LocalRoutineInstallerOptions) =>
	Layer.succeed(RoutineInstaller, {
		Install: ({ inspection, operation_id, scope }) =>
			Effect.gen(function* () {
				if (inspection.source.kind !== "local" || !isAbsolute(options.install_root))
					return yield* InstallerError("install_failed");
				const paths = Object.keys(inspection.content_hashes).sort();
				const max_files = options.max_files ?? 256;
				const max_file_bytes = options.max_file_bytes ?? 1_048_576;
				const max_total_bytes = options.max_total_bytes ?? 8_388_608;
				if (
					paths.length === 0 ||
					paths.length > max_files ||
					paths.some((path) => !SafeRelativePath(path))
				)
					return yield* InstallerError("conflict");
				const source_locator = resolve(inspection.source.locator);
				const source_stat = yield* InstallerFs(() => lstat(source_locator));
				if (source_stat.isSymbolicLink()) return yield* InstallerError("conflict");
				const source_root = yield* InstallerFs(() =>
					realpath(source_stat.isDirectory() ? source_locator : dirname(source_locator)),
				);
				const materialized = new Map<string, Uint8Array>();
				let total = 0;
				for (const path of paths) {
					const bytes = yield* ReadBounded(
						source_root,
						join(source_root, ...path.split("/")),
						max_file_bytes,
					).pipe(Effect.mapError(() => InstallerError("conflict")));
					total += bytes.byteLength;
					if (total > max_total_bytes || Hash(bytes) !== inspection.content_hashes[path])
						return yield* InstallerError("conflict");
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
				yield* InstallerFs(() => mkdir(root, { recursive: true }));
				const root_stat = yield* InstallerFs(() => lstat(root));
				if (!root_stat.isDirectory() || root_stat.isSymbolicLink())
					return yield* InstallerError("install_failed");
				yield* InstallerFs(() => rm(stage, { force: true, recursive: true }));
				const existing_stat = yield* InstallerFsOptional(() => lstat(target));
				const existing =
					existing_stat === undefined
						? undefined
						: yield* Effect.gen(function* () {
								if (!existing_stat.isDirectory() || existing_stat.isSymbolicLink())
									return yield* InstallerError("conflict");
								const existing_root = yield* InstallerFs(
									() => realpath(target),
									"conflict",
								);
								const receipt_bytes = yield* ReadBounded(
									existing_root,
									join(target, ".artisan-install.json"),
									65_536,
								).pipe(Effect.mapError(() => InstallerError("conflict")));
								const existing = yield* Schema.decodeUnknownEffect(
									Schema.UnknownFromJsonString,
								)(new TextDecoder().decode(receipt_bytes)).pipe(
									Effect.flatMap(
										Schema.decodeUnknownEffect(InstallReceipt, {
											onExcessProperty: "error",
										}),
									),
									Effect.mapError(() => InstallerError("conflict")),
								);
								if (
									existing.operation_id !== operation_id ||
									existing.fingerprint !== fingerprint ||
									existing.rollback_id !== rollback_id
								)
									return yield* InstallerError("conflict");
								for (const path of paths) {
									const bytes = yield* ReadBounded(
										existing_root,
										join(target, ...path.split("/")),
										max_file_bytes,
									).pipe(Effect.mapError(() => InstallerError("conflict")));
									if (Hash(bytes) !== inspection.content_hashes[path])
										return yield* InstallerError("conflict");
								}
								return existing;
							});
				if (existing !== undefined)
					return { artifact_refs: existing.artifact_refs, rollback_id };
				yield* InstallerFs(() => mkdir(stage));
				yield* Effect.gen(function* () {
					for (const [path, bytes] of materialized) {
						const destination = join(stage, ...path.split("/"));
						yield* InstallerFs(() => mkdir(dirname(destination), { recursive: true }));
						yield* InstallerFs(() => writeFile(destination, bytes, { flag: "wx" }));
					}
					yield* InstallerFs(() =>
						writeFile(join(stage, ".artisan-install.json"), JSON.stringify(receipt), {
							flag: "wx",
						}),
					);
					yield* InstallerFs(() => rename(stage, target));
				}).pipe(
					Effect.onError(() =>
						InstallerFs(() => rm(stage, { force: true, recursive: true })).pipe(
							Effect.ignore,
						),
					),
				);
				return { artifact_refs: inspection.artifact_refs, rollback_id };
			}),
		Rollback: ({ rollback_id }) =>
			Effect.gen(function* () {
				const match = rollback_id.match(/^rollback_([a-f0-9]{32})_([a-f0-9]{64})$/);
				if (!match) return yield* InstallerError("rollback_failed");
				const installed_id = match.at(1);
				if (installed_id === undefined) return yield* InstallerError("rollback_failed");
				const root = resolve(options.install_root);
				const target = join(root, installed_id);
				const marker_root = join(root, ".rollbacks");
				const marker = join(marker_root, `${Hash(rollback_id)}.json`);
				const trash = join(root, `.rollback-${Hash(rollback_id).slice(0, 32)}`);
				yield* InstallerFs(
					() => mkdir(marker_root, { recursive: true }),
					"rollback_failed",
				);
				const root_stat = yield* InstallerFs(() => lstat(root), "rollback_failed");
				const marker_root_stat = yield* InstallerFs(
					() => lstat(marker_root),
					"rollback_failed",
				);
				if (!root_stat.isDirectory() || root_stat.isSymbolicLink())
					return yield* InstallerError("rollback_failed");
				if (!marker_root_stat.isDirectory() || marker_root_stat.isSymbolicLink())
					return yield* InstallerError("rollback_failed");

				const marker_exists = yield* InstallerFsOptional(() => lstat(marker));
				if (marker_exists !== undefined) {
					const canonical_marker_root = yield* InstallerFs(
						() => realpath(marker_root),
						"rollback_failed",
					);
					const marker_bytes = yield* ReadBounded(canonical_marker_root, marker, 4_096);
					const marker_payload = yield* Schema.decodeUnknownEffect(
						Schema.UnknownFromJsonString,
					)(new TextDecoder().decode(marker_bytes)).pipe(
						Effect.flatMap(
							Schema.decodeUnknownEffect(RollbackMarker, {
								onExcessProperty: "error",
							}),
						),
						Effect.mapError(() => InstallerError("rollback_failed")),
					);
					if (marker_payload.rollback_id !== rollback_id)
						return yield* InstallerError("rollback_failed");
					yield* InstallerFs(
						() => rm(trash, { force: true, recursive: true }),
						"rollback_failed",
					);
					return;
				}

				const target_root = yield* InstallerFsOptional(() => realpath(target));
				if (target_root !== undefined) {
					const target_stat = yield* InstallerFs(() => lstat(target), "rollback_failed");
					if (
						!target_stat.isDirectory() ||
						target_stat.isSymbolicLink() ||
						!IsWithin(root, target_root)
					)
						return yield* InstallerError("rollback_failed");
					const receipt_bytes = yield* ReadBounded(
						target_root,
						join(target, ".artisan-install.json"),
						65_536,
					);
					const receipt = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
						new TextDecoder().decode(receipt_bytes),
					).pipe(
						Effect.flatMap(
							Schema.decodeUnknownEffect(InstallReceipt, {
								onExcessProperty: "error",
							}),
						),
						Effect.mapError(() => InstallerError("rollback_failed")),
					);
					if (receipt.rollback_id !== rollback_id)
						return yield* InstallerError("rollback_failed");
					yield* InstallerFs(() => rename(target, trash), "rollback_failed");
				} else if ((yield* InstallerFsOptional(() => lstat(trash))) === undefined) {
					return yield* InstallerError("rollback_failed");
				}
				yield* InstallerFs(
					() => writeFile(marker, JSON.stringify({ rollback_id }), { flag: "wx" }),
					"rollback_failed",
				);
				yield* InstallerFs(() => rm(trash, { recursive: true }), "rollback_failed");
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof RoutineInstallerError
						? cause
						: InstallerError("rollback_failed"),
				),
			),
	});

const ReadProcessStream = (stream: AsyncIterable<Uint8Array>, max_bytes: number) =>
	Stream.fromAsyncIterable(stream, () => new RoutineInspectorError({ code: "read_failed" })).pipe(
		Stream.runFoldEffect(
			() => ({ chunks: [] as Array<Uint8Array>, total: 0 }),
			(state, chunk) => {
				const total = state.total + chunk.byteLength;
				return total > max_bytes
					? Effect.fail(new RoutineInspectorError({ code: "read_failed" }))
					: Effect.succeed({ chunks: [...state.chunks, chunk], total });
			},
		),
		Effect.map(({ chunks, total }) => {
			const output = new Uint8Array(total);
			let offset = 0;
			for (const chunk of chunks) {
				output.set(chunk, offset);
				offset += chunk.byteLength;
			}
			return output;
		}),
	);

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
