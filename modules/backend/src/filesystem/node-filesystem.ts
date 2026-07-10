import { randomUUID } from "node:crypto";
import { promises as fs, type Dirent } from "node:fs";
import { watch as watch_native } from "node:fs";
import { basename, isAbsolute, relative, resolve } from "node:path";

import { Cause, Effect, Layer, Option, Queue, Stream } from "effect";

import {
	Filesystem,
	FilesystemError,
	type FilesystemChange,
	type FilesystemEntry,
} from "./filesystem";

const trash_directory = ".artisan-trash";
const temporary_file_pattern = /\.artisan-[0-9a-f-]{36}\.tmp$/;

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

function inside(root: string, candidate: string) {
	const path = relative(root, candidate);

	return path === "" || (!path.startsWith("..") && !isAbsolute(path));
}

function is_missing_error(cause: unknown) {
	if (typeof cause !== "object" || cause === null || !("code" in cause)) {
		return false;
	}

	return cause.code === "ENOENT" || cause.code === "ENOTDIR";
}

async function canonical_existing_ancestor(root: string, candidate: string) {
	const segments = relative(root, candidate)
		.split(/[\\/]+/)
		.filter(Boolean);
	let canonical = root;
	let current = root;

	if (segments.length === 0) {
		return { canonical, target_exists: true };
	}

	for (const segment of segments) {
		current = resolve(current, segment);

		try {
			await fs.lstat(current);
		} catch (cause) {
			if (is_missing_error(cause)) {
				return { canonical, target_exists: false };
			}

			throw cause;
		}

		canonical = await fs.realpath(current);
	}

	return { canonical, target_exists: true };
}

function provider_path(root: string, path: string) {
	const normalized = relative(root, path).replaceAll("\\", "/");

	return normalized.length === 0 ? "." : normalized;
}

function entry_from(
	root: string,
	path: string,
	stat: Awaited<ReturnType<typeof fs.lstat>>,
): FilesystemEntry {
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
		path: provider_path(root, path),
		size: Number(stat.size),
	};
}

function try_filesystem<A>(
	operation: FilesystemError["operation"],
	path: string,
	thunk: (signal: AbortSignal) => PromiseLike<A>,
) {
	return Effect.tryPromise({
		try: thunk,
		catch: (cause) => make_error(operation, cause, path),
	});
}

/** Builds a root-confined Node filesystem layer with explicit watch overflow events. */
export function make_node_filesystem_layer(options: {
	readonly root: string;
	readonly watch_capacity?: number;
}) {
	const capacity = options.watch_capacity ?? 256;

	return Layer.effect(
		Filesystem,
		Effect.gen(function* () {
			if (!Number.isSafeInteger(capacity) || capacity <= 0) {
				return yield* Effect.fail(
					make_error(
						"watch",
						new Error("watch_capacity must be a positive safe integer"),
					),
				);
			}

			const root = yield* try_filesystem("resolve", options.root, () =>
				fs.realpath(options.root),
			);
			const trash = resolve(root, trash_directory);

			const resolve_path_info = (path: string, operation: FilesystemError["operation"]) =>
				Effect.gen(function* () {
					const lexical = resolve(root, path);

					if (!inside(root, lexical)) {
						return yield* Effect.fail(
							make_error(operation, new Error("path escapes project root"), path),
						);
					}

					const existing = yield* try_filesystem(operation, path, () =>
						canonical_existing_ancestor(root, lexical),
					);

					if (!inside(root, existing.canonical)) {
						return yield* Effect.fail(
							make_error(
								operation,
								new Error(
									"path resolves through a symlink outside the project root",
								),
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
				resolve_path_info(path, operation).pipe(
					Effect.map((resolution) => resolution.lexical),
				);

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
					inside(trash, resolution.lexical) ||
					inside(trash, resolution.canonical_ancestor)
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
					const stat = yield* try_filesystem(operation, path, () => fs.lstat(resolved));

					return entry_from(root, resolved, stat);
				});

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

									const absolute = resolve(watched, change.filename);

									if (
										!inside(root, absolute) ||
										temporary_file_pattern.test(change.filename)
									) {
										return Option.none<FilesystemChange>();
									}

									if (change.event === "change") {
										return Option.some<FilesystemChange>({
											kind: "modified",
											path: provider_path(root, absolute),
										});
									}

									const kind = yield* try_filesystem(
										"watch",
										change.filename,
										() => fs.lstat(absolute),
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
										path: provider_path(root, absolute),
									});
								});
							const watcher = yield* Effect.try({
								try: () =>
									watch_native(
										watched,
										{ recursive: true },
										(event, filename) => {
											if (filename === null) {
												return;
											}

											const change: RawFilesystemWatchEvent = {
												_tag: "RawFilesystemWatchEvent",
												event,
												filename: String(filename),
											};

											if (active && !Queue.offerUnsafe(queue, change)) {
												dropped_changes = Math.min(
													Number.MAX_SAFE_INTEGER,
													dropped_changes + 1,
												);
											}
										},
									),
								catch: (cause) => make_error("watch", cause, path ?? "."),
							});

							watcher.once("error", (cause) => {
								if (!active) {
									return;
								}

								Queue.failCauseUnsafe(
									queue,
									Cause.fail(make_error("watch", cause, path ?? ".")),
								);
							});

							yield* Effect.addFinalizer(() =>
								Effect.gen(function* () {
									active = false;
									watcher.close();
									yield* Queue.shutdown(queue);
								}),
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

			return {
				CreateDirectory: (path) =>
					resolve_mutable_path(path, "create", true).pipe(
						Effect.flatMap((resolved) =>
							try_filesystem("create", path, () =>
								fs.mkdir(resolved, { recursive: true }),
							),
						),
						Effect.asVoid,
					),
				CreateFile: (path, data = new Uint8Array()) =>
					Effect.gen(function* () {
						const resolved = yield* resolve_mutable_path(path, "create");

						yield* try_filesystem("create", path, (signal) =>
							fs.writeFile(resolved, data, { flag: "wx", signal }),
						);

						return yield* stat_entry(path, "create");
					}),
				DeleteToTrash: (path) =>
					Effect.gen(function* () {
						const source = yield* resolve_mutable_path(path, "delete");

						yield* resolve_path(trash_directory, "delete");
						yield* try_filesystem("delete", path, () =>
							fs.mkdir(trash, { recursive: true }),
						);

						const trash_stat = yield* try_filesystem("delete", path, () =>
							fs.lstat(trash),
						);

						if (!trash_stat.isDirectory() || trash_stat.isSymbolicLink()) {
							return yield* Effect.fail(
								make_error(
									"delete",
									new Error("managed trash path is not a directory"),
									path,
								),
							);
						}

						const destination = resolve(
							trash,
							`${Date.now()}-${randomUUID()}-${basename(source)}`,
						);

						yield* try_filesystem("delete", path, () => fs.rename(source, destination));

						return provider_path(root, destination);
					}),
				List: (path = ".") =>
					Effect.gen(function* () {
						const resolved = yield* resolve_path(path, "list");
						const entries = yield* try_filesystem("list", path, () =>
							fs.readdir(resolved, { withFileTypes: true }),
						);
						const metadata = yield* Effect.forEach(entries, (entry: Dirent) =>
							stat_entry(resolve(resolved, entry.name), "list"),
						);

						return metadata.toSorted((left, right) =>
							left.path.localeCompare(right.path),
						);
					}),
				Read: (path) =>
					resolve_path(path, "read").pipe(
						Effect.flatMap((resolved) =>
							try_filesystem("read", path, () => fs.readFile(resolved)),
						),
					),
				ReadText: (path) =>
					resolve_path(path, "read").pipe(
						Effect.flatMap((resolved) =>
							try_filesystem("read", path, () => fs.readFile(resolved, "utf8")),
						),
					),
				Rename: (from, to) =>
					Effect.gen(function* () {
						const source = yield* resolve_mutable_path(from, "rename");
						const destination = yield* resolve_mutable_path(to, "rename");

						yield* try_filesystem("rename", from, () => fs.rename(source, destination));

						return yield* stat_entry(to, "rename");
					}),
				Resolve: (path) => resolve_path(path, "resolve"),
				Stat: (path) => stat_entry(path),
				Watch: watch,
				WriteAtomic: (path, data) =>
					Effect.gen(function* () {
						const destination = yield* resolve_mutable_path(path, "write");
						const temporary = `${destination}.artisan-${randomUUID()}.tmp`;
						const existing_mode = yield* try_filesystem("write", path, () =>
							fs.lstat(destination),
						).pipe(
							Effect.map((stat) =>
								stat.isFile()
									? Option.some(Number(stat.mode) & 0o7777)
									: Option.none(),
							),
							Effect.catch((error) =>
								is_missing_error(error.cause)
									? Effect.succeed(Option.none<number>())
									: Effect.fail(error),
							),
						);
						const cleanup_temporary = try_filesystem("write", path, () =>
							fs.rm(temporary, { force: true }),
						).pipe(Effect.ignore);

						return yield* Effect.acquireUseRelease(
							Effect.succeed(temporary),
							() =>
								Effect.gen(function* () {
									yield* try_filesystem("write", path, (signal) =>
										fs.writeFile(temporary, data, { flag: "wx", signal }),
									);

									if (Option.isSome(existing_mode)) {
										yield* try_filesystem("write", path, () =>
											fs.chmod(temporary, existing_mode.value),
										);
									}

									yield* try_filesystem("write", path, () =>
										fs.rename(temporary, destination),
									);

									return yield* stat_entry(path, "write");
								}),
							() => cleanup_temporary,
						);
					}),
			};
		}),
	);
}
