import { promises as fs, type BigIntStats } from "node:fs";
import { watch as watch_native } from "node:fs";

import { NodeCrypto, NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import {
	Cause,
	Clock,
	Crypto,
	Effect,
	FileSystem,
	Layer,
	Option,
	Path,
	Queue,
	Stream,
} from "effect";

import {
	BoundedRegularFileStore,
	BoundedRegularFileStoreError,
} from "../bounded-regular-file-store";
import {
	Filesystem,
	FilesystemError,
	type FilesystemChange,
	type FilesystemEntry,
} from "../filesystem";
import { ReadFileIdentity, same_file_identity, type FileIdentity } from "../file-identity";
import { type NodeBoundedRegularFileStoreHooks, type RegularFileSnapshot } from "./context";
import { MakeNodeReplacementService, make_node_replacement_context_layer } from "./replacement";

export type { NodeBoundedRegularFileStoreHooks } from "./context";

const trash_directory = ".artisan-trash";
const temporary_file_pattern = /\.artisan-[0-9a-f-]{36}\.tmp$/;
const conditional_artifact_pattern =
	/^\.artisan-conditional-[0-9a-f]{64}\.(?:stage|backup-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/;

interface PathResolution {
	readonly canonical_ancestor: string;
	readonly lexical: string;
	readonly target_exists: boolean;
}

interface RawFilesystemWatchEvent {
	readonly _tag: "RawFilesystemWatchEvent";
	readonly event: "change" | "rename";
	readonly filename: string;
}

interface RawFilesystemWatchOverflow {
	readonly _tag: "RawFilesystemWatchOverflow";
	readonly dropped: number;
}

type RawFilesystemWatchChange = RawFilesystemWatchEvent | RawFilesystemWatchOverflow;

function make_error(operation: FilesystemError["operation"], cause: unknown, path?: string) {
	return new FilesystemError(
		path === undefined ? { cause, operation } : { cause, operation, path },
	);
}

function make_store_error(
	operation: BoundedRegularFileStoreError["operation"],
	path: string,
	cause: unknown,
) {
	return new BoundedRegularFileStoreError({ cause, operation, path });
}

function inside(path_service: Path.Path, root: string, candidate: string) {
	const relative = path_service.relative(root, candidate);

	return relative === "" || (!relative.startsWith("..") && !path_service.isAbsolute(relative));
}

function is_missing_error(cause: unknown) {
	return (
		typeof cause === "object" &&
		cause !== null &&
		(("code" in cause && (cause.code === "ENOENT" || cause.code === "ENOTDIR")) ||
			("reason" in cause &&
				typeof cause.reason === "object" &&
				cause.reason !== null &&
				"_tag" in cause.reason &&
				cause.reason._tag === "NotFound"))
	);
}

function is_reserved_path(path: string) {
	return path.split(/[\\/]+/).some((segment) => conditional_artifact_pattern.test(segment));
}

/** Native lstat remains necessary because Effect 4 beta 97 has no lstat for symlink identity. */
function Lstat(path: string) {
	return Effect.tryPromise({
		try: () => fs.lstat(path),
		catch: (cause) => cause,
	});
}

/** Native bigint lstat closes Effect beta's no-follow exact-path identity gap. */
function LstatBigint(path: string) {
	return Effect.tryPromise({
		try: () => fs.lstat(path, { bigint: true }),
		catch: (cause) => cause,
	});
}

function normalize_uint64(value: bigint) {
	const modulus = 1n << 64n;

	return value < 0n ? value + modulus : value;
}

function identity_from_lstat(stat: BigIntStats) {
	return {
		device: normalize_uint64(stat.dev),
		inode: normalize_uint64(stat.ino),
	} satisfies FileIdentity;
}

function entry_from(
	root: string,
	path: string,
	stat: Awaited<ReturnType<typeof fs.lstat>>,
	path_service: Path.Path,
): FilesystemEntry {
	const relative = path_service.relative(root, path).replaceAll("\\", "/");
	const kind = stat.isFile()
		? "file"
		: stat.isDirectory()
			? "directory"
			: stat.isSymbolicLink()
				? "symlink"
				: "other";

	return {
		created_at: stat.birthtime.toISOString(),
		kind,
		mode: Number(stat.mode),
		modified_at: stat.mtime.toISOString(),
		path: relative.length === 0 ? "." : relative,
		size: Number(stat.size),
	};
}

/** Builds the shared Node filesystem capabilities before exposing either seam. */
function BuildNodeFilesystemCapabilities(options: {
	readonly hooks?: NodeBoundedRegularFileStoreHooks;
	readonly root: string;
	readonly watch_capacity?: number;
}) {
	const capacity = options.watch_capacity ?? 256;

	return Effect.gen(function* () {
		const file_system = yield* FileSystem.FileSystem;
		const path_service = yield* Path.Path;
		const crypto = yield* Crypto.Crypto;
		const map_error = (operation: FilesystemError["operation"], path: string) =>
			Effect.mapError((cause: unknown) => make_error(operation, cause, path));
		const map_store_error = (
			operation: BoundedRegularFileStoreError["operation"],
			path: string,
		) => Effect.mapError((cause: unknown) => make_store_error(operation, path, cause));
		const ReadLstat = (
			operation: FilesystemError["operation"],
			native_path: string,
			reported_path: string,
		) => Lstat(native_path).pipe(map_error(operation, reported_path));
		const RealPath = (operation: FilesystemError["operation"], path: string) =>
			file_system.realPath(path).pipe(map_error(operation, path));
		const RandomUuid = (operation: FilesystemError["operation"], path: string) =>
			crypto.randomUUIDv4.pipe(map_error(operation, path));
		const ReadRegularSnapshot = (resolved: string, path: string, maximum_bytes: number) =>
			Effect.scoped(
				Effect.gen(function* () {
					if (!Number.isSafeInteger(maximum_bytes) || maximum_bytes <= 0) {
						return yield* Effect.fail(
							new Error("maximum_bytes must be a positive safe integer"),
						);
					}

					const before = yield* LstatBigint(resolved);

					if (!before.isFile() || before.isSymbolicLink()) {
						return yield* Effect.fail(new Error("path is not a regular file"));
					}

					if (before.size > BigInt(maximum_bytes)) {
						return yield* Effect.fail(new Error("regular file exceeds maximum_bytes"));
					}

					const file = yield* file_system.open(resolved, { flag: "r" });
					const identity = yield* ReadFileIdentity(file.fd);
					const descriptor_stat = yield* file.stat;
					const mode = descriptor_stat.mode & 0o7777;

					if (
						descriptor_stat.type !== "File" ||
						descriptor_stat.size > BigInt(maximum_bytes) ||
						mode !== (Number(before.mode) & 0o7777) ||
						!same_file_identity(identity, identity_from_lstat(before))
					) {
						return yield* Effect.fail(new Error("regular file changed before opening"));
					}

					const expected_size = descriptor_stat.size;
					const bytes =
						expected_size === 0n
							? new Uint8Array()
							: Option.getOrElse(
									yield* file.readAlloc(expected_size),
									() => new Uint8Array(),
								);
					const after = yield* LstatBigint(resolved);
					const after_stat = yield* file.stat;

					if (
						bytes.length !== Number(expected_size) ||
						!after.isFile() ||
						after.isSymbolicLink() ||
						!same_file_identity(identity, identity_from_lstat(after)) ||
						after.size !== before.size ||
						after.mode !== before.mode ||
						after.mtimeNs !== before.mtimeNs ||
						after.ctimeNs !== before.ctimeNs ||
						after_stat.type !== "File" ||
						after_stat.size !== expected_size ||
						(after_stat.mode & 0o7777) !== mode ||
						after_stat.size > BigInt(maximum_bytes)
					) {
						return yield* Effect.fail(new Error("regular file changed while reading"));
					}

					return {
						bytes,
						identity,
						mode,
					} satisfies RegularFileSnapshot;
				}),
			);
		const ReadOptionalRegularSnapshot = (
			resolved: string,
			path: string,
			maximum_bytes: number,
		) =>
			ReadRegularSnapshot(resolved, path, maximum_bytes).pipe(
				Effect.map(Option.some),
				Effect.catch((cause) =>
					is_missing_error(cause)
						? Effect.succeed(Option.none<RegularFileSnapshot>())
						: Effect.fail(cause),
				),
			);
		if (!Number.isSafeInteger(capacity) || capacity <= 0) {
			return yield* Effect.fail(
				make_error("watch", new Error("watch_capacity must be a positive safe integer")),
			);
		}

		const root = yield* RealPath("resolve", options.root);
		const trash = path_service.resolve(root, trash_directory);
		const canonical_existing_ancestor = (candidate: string) =>
			Effect.gen(function* () {
				const segments = path_service
					.relative(root, candidate)
					.split(/[\\/]+/)
					.filter(Boolean);
				let canonical = root;
				let current = root;

				for (const segment of segments) {
					current = path_service.resolve(current, segment);
					const exists = yield* Lstat(current).pipe(
						Effect.as(true),
						Effect.catch((cause) =>
							is_missing_error(cause) ? Effect.succeed(false) : Effect.fail(cause),
						),
					);

					if (!exists) {
						return { canonical, target_exists: false };
					}

					canonical = yield* RealPath("resolve", current);
				}

				return { canonical, target_exists: true };
			});
		const resolve_path_info = (path: string, operation: FilesystemError["operation"]) =>
			Effect.gen(function* () {
				if (is_reserved_path(path)) {
					return yield* Effect.fail(
						make_error(
							operation,
							new Error("internal filesystem artifacts are reserved"),
							path,
						),
					);
				}

				const lexical = path_service.resolve(root, path);

				if (!inside(path_service, root, lexical)) {
					return yield* Effect.fail(
						make_error(operation, new Error("path escapes project root"), path),
					);
				}

				const existing = yield* canonical_existing_ancestor(lexical).pipe(
					map_error(operation, path),
				);

				if (!inside(path_service, root, existing.canonical)) {
					return yield* Effect.fail(
						make_error(
							operation,
							new Error("path resolves through a symlink outside the project root"),
							path,
						),
					);
				}

				return {
					canonical_ancestor: existing.canonical,
					lexical,
					target_exists: existing.target_exists,
				} satisfies PathResolution;
			});
		const resolve_path = (path: string, operation: FilesystemError["operation"]) =>
			resolve_path_info(path, operation).pipe(Effect.map((resolution) => resolution.lexical));
		const assert_mutable_path = (
			path: string,
			resolution: PathResolution,
			operation: FilesystemError["operation"],
			allow_root: boolean,
		) => {
			if (
				!allow_root &&
				(resolution.lexical === root ||
					(resolution.target_exists && resolution.canonical_ancestor === root))
			) {
				return Effect.fail(
					make_error(operation, new Error("the project root is protected"), path),
				);
			}

			if (
				inside(path_service, trash, resolution.lexical) ||
				inside(path_service, trash, resolution.canonical_ancestor)
			) {
				return Effect.fail(
					make_error(
						operation,
						new Error("the managed trash directory is protected"),
						path,
					),
				);
			}

			return Effect.void;
		};
		const resolve_mutable_path = (
			path: string,
			operation: FilesystemError["operation"],
			allow_root = false,
		) =>
			resolve_path_info(path, operation).pipe(
				Effect.tap((resolution) =>
					assert_mutable_path(path, resolution, operation, allow_root),
				),
				Effect.map((resolution) => resolution.lexical),
			);
		const stat_entry = (path: string, operation: FilesystemError["operation"] = "stat") =>
			Effect.gen(function* () {
				const resolved = yield* resolve_path(path, operation);

				return entry_from(
					root,
					resolved,
					yield* ReadLstat(operation, resolved, path),
					path_service,
				);
			});
		/** Native watch remains necessary because Effect FileSystem.watch has no dropped-event accounting. */
		const watch = (path?: string) =>
			Stream.scoped(
				Stream.unwrap(
					Effect.gen(function* () {
						const watched = yield* resolve_path(path ?? ".", "watch");
						const queue = yield* Queue.dropping<
							RawFilesystemWatchEvent,
							FilesystemError
						>(capacity);
						let active = true;
						let dropped_changes = 0;
						const normalize_raw_change = (change: RawFilesystemWatchChange) =>
							Effect.gen(function* () {
								if (change._tag === "RawFilesystemWatchOverflow") {
									return Option.some<FilesystemChange>({
										dropped: change.dropped,
										kind: "overflow",
									});
								}

								const absolute = path_service.resolve(watched, change.filename);

								if (
									!inside(path_service, root, absolute) ||
									temporary_file_pattern.test(change.filename) ||
									is_reserved_path(change.filename)
								) {
									return Option.none<FilesystemChange>();
								}

								if (change.event === "change") {
									return Option.some<FilesystemChange>({
										kind: "modified",
										path: path_service
											.relative(root, absolute)
											.replaceAll("\\", "/"),
									});
								}

								const kind = yield* ReadLstat(
									"watch",
									absolute,
									change.filename,
								).pipe(
									Effect.as("created" as const),
									Effect.catch((error) =>
										is_missing_error(error.cause)
											? Effect.succeed("deleted" as const)
											: Effect.fail(error),
									),
								);

								return Option.some<FilesystemChange>({
									kind,
									path: path_service
										.relative(root, absolute)
										.replaceAll("\\", "/"),
								});
							});
						const watcher = yield* Effect.try({
							try: () =>
								watch_native(watched, { recursive: true }, (event, filename) => {
									if (filename === null) return;
									const change: RawFilesystemWatchEvent = {
										_tag: "RawFilesystemWatchEvent",
										event,
										filename: String(filename),
									};
									if (active && !Queue.offerUnsafe(queue, change))
										dropped_changes = Math.min(
											Number.MAX_SAFE_INTEGER,
											dropped_changes + 1,
										);
								}),
							catch: (cause) => make_error("watch", cause, path ?? "."),
						});

						watcher.once("error", (cause) => {
							if (active)
								Queue.failCauseUnsafe(
									queue,
									Cause.fail(make_error("watch", cause, path ?? ".")),
								);
						});
						yield* Effect.addFinalizer(() =>
							Effect.sync(() => {
								active = false;
								watcher.close();
							}).pipe(Effect.andThen(Queue.shutdown(queue))),
						);
						const take_raw_change = Effect.suspend<
							RawFilesystemWatchChange,
							FilesystemError,
							never
						>(() => {
							if (dropped_changes > 0) {
								const dropped = dropped_changes;
								dropped_changes = 0;
								return Effect.succeed<RawFilesystemWatchOverflow>({
									_tag: "RawFilesystemWatchOverflow",
									dropped,
								});
							}
							return Queue.take(queue);
						});

						return Stream.fromEffectRepeat(take_raw_change).pipe(
							Stream.mapEffect(normalize_raw_change, { concurrency: 1 }),
							Stream.filter(Option.isSome),
							Stream.map((change) => change.value),
						);
					}),
				),
			);
		const filesystem = {
			CreateDirectory: (path) =>
				resolve_mutable_path(path, "create", true).pipe(
					Effect.flatMap((resolved) =>
						file_system
							.makeDirectory(resolved, { recursive: true })
							.pipe(map_error("create", path)),
					),
					Effect.asVoid,
				),
			CreateFile: (path, data = new Uint8Array()) =>
				Effect.gen(function* () {
					const resolved = yield* resolve_mutable_path(path, "create");
					yield* file_system
						.writeFile(resolved, data, { flag: "wx" })
						.pipe(map_error("create", path));
					return yield* stat_entry(path, "create");
				}),
			DeleteToTrash: (path) =>
				Effect.gen(function* () {
					const source = yield* resolve_mutable_path(path, "delete");
					yield* resolve_path(trash_directory, "delete");
					yield* file_system
						.makeDirectory(trash, { recursive: true })
						.pipe(map_error("delete", path));
					const trash_stat = yield* ReadLstat("delete", trash, path);
					if (!trash_stat.isDirectory() || trash_stat.isSymbolicLink())
						return yield* Effect.fail(
							make_error(
								"delete",
								new Error("managed trash path is not a directory"),
								path,
							),
						);
					const destination = path_service.resolve(
						trash,
						`${yield* Clock.currentTimeMillis}-${yield* RandomUuid("delete", path)}-${path_service.basename(source)}`,
					);
					yield* file_system.rename(source, destination).pipe(map_error("delete", path));
					return path_service.relative(root, destination).replaceAll("\\", "/");
				}),
			List: (path = ".") =>
				Effect.gen(function* () {
					const resolved = yield* resolve_path(path, "list");
					const entries = (yield* file_system
						.readDirectory(resolved)
						.pipe(map_error("list", path))).filter(
						(entry) => !conditional_artifact_pattern.test(entry),
					);
					const metadata = yield* Effect.forEach(entries, (entry) =>
						stat_entry(path_service.resolve(resolved, entry), "list"),
					);
					return metadata.toSorted((left, right) => left.path.localeCompare(right.path));
				}),
			Read: (path) =>
				resolve_path(path, "read").pipe(
					Effect.flatMap((resolved) =>
						file_system.readFile(resolved).pipe(map_error("read", path)),
					),
				),
			ReadText: (path) =>
				resolve_path(path, "read").pipe(
					Effect.flatMap((resolved) =>
						file_system.readFileString(resolved).pipe(map_error("read", path)),
					),
				),
			Rename: (from, to) =>
				Effect.gen(function* () {
					const source = yield* resolve_mutable_path(from, "rename");
					const destination = yield* resolve_mutable_path(to, "rename");
					yield* file_system.rename(source, destination).pipe(map_error("rename", from));
					return yield* stat_entry(to, "rename");
				}),
			Resolve: (path) => resolve_path(path, "resolve"),
			Stat: (path) => stat_entry(path),
			Watch: watch,
			WriteAtomic: (path, data) =>
				Effect.gen(function* () {
					const destination = yield* resolve_mutable_path(path, "write");
					const temporary = `${destination}.artisan-${yield* RandomUuid("write", path)}.tmp`;
					const existing_mode = yield* ReadLstat("write", destination, path).pipe(
						Effect.map((stat) =>
							stat.isFile() ? Option.some(Number(stat.mode) & 0o7777) : Option.none(),
						),
						Effect.catch((error) =>
							is_missing_error(error.cause)
								? Effect.succeed(Option.none<number>())
								: Effect.fail(error),
						),
					);
					const cleanup_temporary = file_system
						.remove(temporary, { force: true })
						.pipe(Effect.ignore);
					return yield* Effect.acquireUseRelease(
						Effect.succeed(temporary),
						() =>
							Effect.gen(function* () {
								yield* file_system
									.writeFile(temporary, data, { flag: "wx" })
									.pipe(map_error("write", path));
								if (Option.isSome(existing_mode))
									yield* file_system
										.chmod(temporary, existing_mode.value)
										.pipe(map_error("write", path));
								yield* file_system
									.rename(temporary, destination)
									.pipe(map_error("write", path));
								return yield* stat_entry(path, "write");
							}),
						() => cleanup_temporary,
					);
				}),
		} satisfies typeof Filesystem.Service;
		const replacement = yield* MakeNodeReplacementService.pipe(
			Effect.provide(
				make_node_replacement_context_layer({
					crypto,
					file_system,
					hooks: options.hooks,
					path_service,
					ReadOptionalRegularSnapshot,
					ReadRegularSnapshot,
					resolve_mutable_path: (path, operation) =>
						resolve_mutable_path(path, operation),
					root,
				}),
			),
		);
		const bounded_regular_file_store = {
			FinalizeRegularFileReplacement: replacement.FinalizeRegularFileReplacement,
			ReadRegularFile: (path: string, maximum_bytes: number) =>
				resolve_path(path, "read").pipe(
					Effect.flatMap((resolved) =>
						ReadRegularSnapshot(resolved, path, maximum_bytes),
					),
					Effect.map((snapshot) => snapshot.bytes),
					map_store_error("read", path),
				),
			ReplaceRegularFile: replacement.ReplaceRegularFile,
		} satisfies typeof BoundedRegularFileStore.Service;

		return { bounded_regular_file_store, filesystem };
	});
}

/** Builds the ordinary Node filesystem adapter without exposing conditional mutation. */
export function make_node_filesystem(options: {
	readonly root: string;
	readonly watch_capacity?: number;
}): Effect.Effect<
	typeof Filesystem.Service,
	FilesystemError,
	FileSystem.FileSystem | Path.Path | Crypto.Crypto
> {
	return BuildNodeFilesystemCapabilities(options).pipe(
		Effect.map((capabilities) => capabilities.filesystem),
	);
}

/** Builds the path-checked algorithm adapter used until native handle confinement exists. */
export function make_node_non_adversarial_bounded_regular_file_store(options: {
	readonly hooks?: NodeBoundedRegularFileStoreHooks;
	readonly root: string;
}) {
	return BuildNodeFilesystemCapabilities(options).pipe(
		Effect.map((capabilities) => capabilities.bounded_regular_file_store),
	);
}

/** Builds a self-contained Node filesystem layer with explicit watch overflow events. */
export function make_node_filesystem_layer(options: {
	readonly root: string;
	readonly watch_capacity?: number;
}) {
	return Layer.effect(Filesystem, make_node_filesystem(options)).pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(NodePath.layer),
		Layer.provideMerge(NodeCrypto.layer),
	);
}

/** Builds the explicitly non-adversarial bounded regular-file algorithm layer. */
export function make_node_non_adversarial_bounded_regular_file_store_layer(options: {
	readonly hooks?: NodeBoundedRegularFileStoreHooks;
	readonly root: string;
}) {
	return Layer.effect(
		BoundedRegularFileStore,
		make_node_non_adversarial_bounded_regular_file_store(options),
	).pipe(
		Layer.provideMerge(NodeFileSystem.layer),
		Layer.provideMerge(NodePath.layer),
		Layer.provideMerge(NodeCrypto.layer),
	);
}
