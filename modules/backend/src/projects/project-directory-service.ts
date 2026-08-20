import {
	Context,
	Data,
	Deferred,
	Effect,
	Exit,
	FileSystem,
	Layer,
	Option,
	Path,
	Ref,
	Scope,
	Semaphore,
} from "effect";

import type {
	ProjectDirectoryCreateInput,
	ProjectDirectoryEntry,
	ProjectDirectoryList,
	ProjectDirectoryListInput,
	ProjectDirectoryPickResult,
	ProjectDirectoryPlace,
	ProjectDirectoryPlaceKind,
	ProjectDirectorySelectInput,
	ProjectRef,
} from "@artisan/protocol";
import { SnowflakeId } from "@artisan/protocol";
import { ProjectLocator } from "../threads/project-locator";
import { NativeDirectoryPicker } from "./native-directory-picker";

const maximum_directories = 256;
const maximum_files = 256;
const child_probe_concurrency = 16;
/** Bounds independent configured-root and shortcut filesystem probes at startup. */
const startup_probe_concurrency = 8;

/** The standard user folders offered as picker shortcuts, in presentation order. */
const known_places: ReadonlyArray<{
	readonly folder_name: string | undefined;
	readonly place: ProjectDirectoryPlaceKind;
}> = [
	{ folder_name: undefined, place: "home" },
	{ folder_name: "Desktop", place: "desktop" },
	{ folder_name: "Documents", place: "documents" },
	{ folder_name: "Downloads", place: "downloads" },
	{ folder_name: "Pictures", place: "pictures" },
	{ folder_name: "Music", place: "music" },
	{ folder_name: "Videos", place: "videos" },
];

export class ProjectDirectoryError extends Data.TaggedError("ProjectDirectoryError")<{
	readonly cause?: unknown;
	readonly code:
		| "create_failed"
		| "invalid_directory"
		| "invalid_name"
		| "list_failed"
		| "pick_failed"
		| "project_not_found";
}> {}

export class ProjectDirectoryService extends Context.Service<
	ProjectDirectoryService,
	{
		readonly Create: (
			input: ProjectDirectoryCreateInput,
		) => Effect.Effect<ProjectDirectoryEntry, ProjectDirectoryError>;
		readonly List: (
			input: ProjectDirectoryListInput,
		) => Effect.Effect<ProjectDirectoryList, ProjectDirectoryError>;
		readonly Pick: Effect.Effect<ProjectDirectoryPickResult, ProjectDirectoryError>;
		readonly Select: (
			input: ProjectDirectorySelectInput,
		) => Effect.Effect<ProjectRef, ProjectDirectoryError>;
	}
>()("Artisan/ProjectDirectoryService") {}

/**
 * One plain path segment: no separators, no traversal, no control characters,
 * and none of the characters Windows refuses in file names.
 */
const valid_directory_name = (name: string) =>
	name === name.trim() &&
	name.length <= 128 &&
	name !== "." &&
	name !== ".." &&
	// eslint-disable-next-line no-control-regex -- rejects raw control characters in names
	!/[\\/:*?"<>|\u0000-\u001f]/.test(name);

interface KnownDirectory {
	readonly canonical_path: string;
	readonly root_path: string;
}

interface DirectoryRegistry {
	readonly by_id: ReadonlyMap<string, KnownDirectory>;
	readonly by_path: ReadonlyMap<string, string>;
}

/**
 * Optional construction observation used by deterministic lifecycle coverage.
 * It runs after an opaque id is allocated but before the registry publishes it,
 * so interruption there must leave no externally resolvable registration.
 */
export interface ProjectDirectoryServiceOptions {
	readonly OnRegisterBeforePublish?: (input: {
		readonly canonical_path: string;
		readonly directory_id: string;
	}) => Effect.Effect<void>;
}

function inside(path_service: Path.Path, root: string, candidate: string) {
	const relative = path_service.relative(root, candidate);
	return relative === "" || (!relative.startsWith("..") && !path_service.isAbsolute(relative));
}

/**
 * Builds a stateful browser-safe directory service over explicitly allowed
 * server roots. When the home directory is named, its standard user folders
 * (Desktop, Documents, Downloads, …) surface as picker shortcuts — still
 * bounded by the allowed roots like every other listing.
 */
export function make_project_directory_service_layer(
	allowed_roots: ReadonlyArray<string>,
	home_directory?: string,
	options: ProjectDirectoryServiceOptions = {},
) {
	return Layer.effect(
		ProjectDirectoryService,
		Effect.gen(function* () {
			const snowflake_id = yield* SnowflakeId;
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const locator = yield* ProjectLocator;
			const native_picker = yield* NativeDirectoryPicker;
			const service_scope = yield* Scope.Scope;
			const registry = yield* Ref.make<DirectoryRegistry>({
				by_id: new Map(),
				by_path: new Map(),
			});
			const registration_lock = yield* Semaphore.make(1);
			const initialization_lock = yield* Semaphore.make(1);

			const Register = (known: KnownDirectory) =>
				Semaphore.withPermit(registration_lock)(
					Effect.gen(function* () {
						const existing = (yield* Ref.get(registry)).by_path.get(
							known.canonical_path,
						);
						if (existing !== undefined) return existing;
						const directory_id = yield* snowflake_id.Make("directory");
						yield* (
							options.OnRegisterBeforePublish?.({
								canonical_path: known.canonical_path,
								directory_id,
							}) ?? Effect.void
						);
						return yield* Ref.modify(registry, (current) => {
							const registered = current.by_path.get(known.canonical_path);
							if (registered !== undefined) return [registered, current] as const;
							return [
								directory_id,
								{
									by_id: new Map(current.by_id).set(directory_id, known),
									by_path: new Map(current.by_path).set(
										known.canonical_path,
										directory_id,
									),
								},
							] as const;
						});
					}),
				);

			interface InitializedDirectoryState {
				readonly places: ReadonlyArray<ProjectDirectoryPlace>;
				readonly root_entries: ReadonlyArray<ProjectDirectoryEntry>;
			}
			const Initialize = Effect.gen(function* () {
				const startup_candidates = [
					...allowed_roots.map((target) => ({ kind: "root" as const, target })),
					...(home_directory === undefined
						? []
						: known_places.map((candidate) => ({
								candidate,
								kind: "place" as const,
								target:
									candidate.folder_name === undefined
										? home_directory
										: path_service.join(home_directory, candidate.folder_name),
							}))),
				];
				/**
				 * Root validation and shortcut discovery do not depend on one another.
				 * Probe them together, but retain input order so configured roots and
				 * presentation-order places stay deterministic after concurrent I/O.
				 */
				const startup_results = yield* Effect.forEach(
					startup_candidates,
					(candidate) =>
						Effect.gen(function* () {
							const canonical_path = yield* file_system.realPath(candidate.target);
							const metadata = yield* file_system.stat(canonical_path);
							return { canonical_path, metadata };
						}).pipe(Effect.exit),
					{ concurrency: startup_probe_concurrency },
				);
				const root_paths = startup_results.flatMap((result, index) => {
					const candidate = startup_candidates[index]!;
					if (candidate.kind !== "root") return [];
					if (Exit.isFailure(result) || result.value.metadata.type !== "Directory") {
						return [undefined];
					}
					return [result.value.canonical_path];
				});
				if (root_paths.some((root) => root === undefined)) {
					return yield* Effect.fail(
						new ProjectDirectoryError({
							code: "list_failed",
							cause: new Error("Allowed project root is not a directory"),
						}),
					);
				}
				const resolved_roots = [
					...new Set(root_paths.filter((root): root is string => root !== undefined)),
				];
				const root_entries = yield* Effect.forEach(resolved_roots, (root_path) =>
					Effect.gen(function* () {
						const directory_id = yield* Register({
							canonical_path: root_path,
							root_path,
						});
						return {
							directory_id,
							display_name: path_service.basename(root_path) || root_path,
							has_children: true,
							kind: "root",
						} satisfies ProjectDirectoryEntry;
					}),
				);
				if (home_directory === undefined) {
					return { places: [], root_entries } satisfies InitializedDirectoryState;
				}
				const resolved_places = yield* Effect.forEach(
					startup_results.flatMap((result, index) => {
						const candidate = startup_candidates[index]!;
						if (candidate.kind !== "place" || Exit.isFailure(result)) return [];
						return [{ candidate: candidate.candidate, ...result.value }];
					}),
					({ candidate, canonical_path, metadata }) =>
						Effect.gen(function* () {
							const root_path = resolved_roots.find((root) =>
								inside(path_service, root, canonical_path),
							);
							if (metadata.type !== "Directory" || root_path === undefined) {
								return Option.none<ProjectDirectoryPlace>();
							}
							const directory_id = yield* Register({ canonical_path, root_path });
							return Option.some({
								directory_id,
								display_name: candidate.folder_name ?? "Home",
								place: candidate.place,
							} satisfies ProjectDirectoryPlace);
						}),
				);
				return {
					places: resolved_places.flatMap((place) =>
						Option.isSome(place) ? [place.value] : [],
					),
					root_entries,
				} satisfies InitializedDirectoryState;
			});
			type InitializationFlight = Deferred.Deferred<
				Exit.Exit<InitializedDirectoryState, ProjectDirectoryError>
			>;
			type InitializationState =
				| { readonly _tag: "Closed" }
				| { readonly _tag: "Open"; readonly flight: InitializationFlight | undefined };
			const initialization = yield* Ref.make<InitializationState>({
				_tag: "Open",
				flight: undefined,
			});
			/**
			 * A service-scope close can interrupt a just-admitted owner before its
			 * completion handler starts. Atomically close admission and capture the
			 * exact active flight before settling external waiters; their waits remain
			 * interruptible in their own caller scopes.
			 */
			yield* Effect.addFinalizer(() =>
				Ref.modify(initialization, (current) => [
					current._tag === "Open" ? current.flight : undefined,
					{ _tag: "Closed" } as const,
				]).pipe(
					Effect.flatMap((flight) =>
						flight === undefined ? Effect.void : Deferred.interrupt(flight),
					),
					Effect.uninterruptible,
				),
			);
			/**
			 * Only a completed success remains cached. A typed failure or interruption
			 * clears the slot before releasing waiters, so the next caller starts a
			 * fresh scoped flight instead of permanently poisoning the directory API.
			 */
			const StartInitialization = Semaphore.withPermit(initialization_lock)(
				Effect.uninterruptibleMask((restore) =>
					Effect.gen(function* () {
						const flight =
							yield* Deferred.make<
								Exit.Exit<InitializedDirectoryState, ProjectDirectoryError>
							>();
						const admitted = yield* Ref.modify(initialization, (current) => {
							if (current._tag === "Closed") return [undefined, current] as const;
							return [
								current.flight ?? flight,
								{
									_tag: "Open",
									flight: current.flight ?? flight,
								},
							] as const;
						});
						if (admitted === undefined) return yield* Effect.interrupt;
						if (admitted !== flight) return admitted;
						yield* Effect.forkIn(
							restore(Initialize).pipe(
								Effect.exit,
								Effect.flatMap((result) =>
									(Exit.isFailure(result)
										? Ref.update(initialization, (current) =>
												current._tag === "Open" && current.flight === flight
													? ({ _tag: "Open", flight: undefined } as const)
													: current,
											)
										: Effect.void
									).pipe(
										Effect.andThen(Deferred.succeed(flight, result)),
										Effect.asVoid,
										Effect.uninterruptible,
									),
								),
							),
							service_scope,
						);
						return flight;
					}),
				),
			);
			const AwaitInitialization = StartInitialization.pipe(
				Effect.flatMap(Deferred.await),
				Effect.flatMap((result) =>
					Exit.isSuccess(result)
						? Effect.succeed(result.value)
						: Effect.failCause(result.cause),
				),
			);
			/** Start the scoped initialization flight after the service is publishable. */
			yield* StartInitialization;

			const ResolveKnown = (directory_id: string) =>
				Effect.gen(function* () {
					const known = (yield* Ref.get(registry)).by_id.get(directory_id);
					if (known === undefined) {
						return yield* Effect.fail(
							new ProjectDirectoryError({ code: "invalid_directory" }),
						);
					}
					const canonical_path = yield* file_system.realPath(known.canonical_path);
					const metadata = yield* file_system.stat(canonical_path);
					if (
						metadata.type !== "Directory" ||
						!inside(path_service, known.root_path, canonical_path)
					) {
						return yield* Effect.fail(
							new ProjectDirectoryError({ code: "invalid_directory" }),
						);
					}
					return { canonical_path, root_path: known.root_path };
				}).pipe(
					Effect.mapError((cause) =>
						cause instanceof ProjectDirectoryError
							? cause
							: new ProjectDirectoryError({ cause, code: "invalid_directory" }),
					),
				);

			type ChildListing = {
				readonly directories: ReadonlyArray<ProjectDirectoryEntry>;
				readonly files: ReadonlyArray<string>;
			};

			const ChildEntries = (
				parent_directory_id: string,
			): Effect.Effect<ChildListing, unknown> =>
				Effect.gen(function* () {
					const parent = yield* ResolveKnown(parent_directory_id);
					/**
					 * The scan is bounded before any stat runs: each name costs a
					 * filesystem probe, and a pathological directory must not turn
					 * one listing into thousands of them.
					 */
					const names = (yield* file_system.readDirectory(parent.canonical_path))
						.toSorted((left, right) => left.localeCompare(right))
						.slice(0, maximum_directories + maximum_files);
					const entries = yield* Effect.forEach(
						names,
						(name) =>
							Effect.gen(function* () {
								const candidate = path_service.join(parent.canonical_path, name);
								const canonical_path = yield* file_system.realPath(candidate);
								const metadata = yield* file_system.stat(canonical_path);
								if (metadata.type === "File") {
									return Option.some({ name, type: "file" as const });
								}
								if (
									metadata.type !== "Directory" ||
									!inside(path_service, parent.root_path, canonical_path)
								) {
									return Option.none<
										| { readonly name: string; readonly type: "file" }
										| {
												readonly entry: ProjectDirectoryEntry;
												readonly type: "directory";
										  }
									>();
								}
								const directory_id = yield* Register({
									canonical_path,
									root_path: parent.root_path,
								});
								return Option.some({
									entry: {
										directory_id,
										display_name: name,
										/** The browser may enter every returned directory without a preflight. */
										has_children: true,
										kind: "directory",
									} satisfies ProjectDirectoryEntry,
									type: "directory" as const,
								});
							}).pipe(Effect.catch(() => Effect.succeed(Option.none()))),
						{
							concurrency: child_probe_concurrency,
						},
					);
					const listed = entries.flatMap((entry) =>
						Option.isSome(entry) ? [entry.value] : [],
					);
					return {
						directories: listed
							.flatMap((item) => (item.type === "directory" ? [item.entry] : []))
							.slice(0, maximum_directories),
						files: listed
							.flatMap((item) => (item.type === "file" ? [item.name] : []))
							.slice(0, maximum_files),
					};
				});

			return {
				Create: (input) =>
					AwaitInitialization.pipe(
						Effect.andThen(
							Effect.gen(function* () {
								if (!valid_directory_name(input.name)) {
									return yield* Effect.fail(
										new ProjectDirectoryError({ code: "invalid_name" }),
									);
								}
								const parent = yield* ResolveKnown(input.parent_directory_id);
								const target = path_service.join(parent.canonical_path, input.name);
								/** Non-recursive on purpose: an existing folder is a caller error, not a no-op. */
								yield* file_system.makeDirectory(target);
								const canonical_path = yield* file_system.realPath(target);
								if (!inside(path_service, parent.root_path, canonical_path)) {
									return yield* Effect.fail(
										new ProjectDirectoryError({ code: "invalid_directory" }),
									);
								}
								const directory_id = yield* Register({
									canonical_path,
									root_path: parent.root_path,
								});
								return {
									directory_id,
									display_name: input.name,
									has_children: false,
									kind: "directory",
								} satisfies ProjectDirectoryEntry;
							}),
						),
						Effect.mapError((cause) =>
							cause instanceof ProjectDirectoryError
								? cause
								: new ProjectDirectoryError({ cause, code: "create_failed" }),
						),
					),
				List: (input) =>
					AwaitInitialization.pipe(
						Effect.flatMap(({ places, root_entries }) =>
							(input.parent_directory_id === undefined
								? Effect.succeed<ChildListing>({
										directories: root_entries,
										files: [],
									})
								: ChildEntries(input.parent_directory_id)
							).pipe(
								Effect.map((listing) => ({
									directories: listing.directories,
									files: listing.files,
									places,
									...(input.parent_directory_id === undefined
										? {}
										: { parent_directory_id: input.parent_directory_id }),
								})),
							),
						),
						Effect.mapError((cause) =>
							cause instanceof ProjectDirectoryError
								? cause
								: new ProjectDirectoryError({ cause, code: "list_failed" }),
						),
					),
				Pick: AwaitInitialization.pipe(
					Effect.andThen(
						Effect.gen(function* () {
							const picked = yield* native_picker.Pick();
							if (picked.kind === "cancelled") {
								return { status: "cancelled" } as const;
							}

							const canonical_path = yield* file_system.realPath(picked.path);
							const metadata = yield* file_system.stat(canonical_path);
							if (metadata.type !== "Directory") {
								return yield* Effect.fail(
									new ProjectDirectoryError({ code: "invalid_directory" }),
								);
							}

							const directory_id = yield* Register({
								canonical_path,
								root_path: canonical_path,
							});
							return {
								directory: {
									directory_id,
									display_name:
										path_service.basename(canonical_path) || "Selected folder",
									has_children: true,
									kind: "root",
								},
								status: "selected",
							} as const;
						}),
					),
					Effect.mapError((cause) =>
						cause instanceof ProjectDirectoryError
							? cause
							: new ProjectDirectoryError({ cause, code: "pick_failed" }),
					),
				),
				Select: (input) =>
					AwaitInitialization.pipe(
						Effect.andThen(
							Effect.gen(function* () {
								const selected = yield* ResolveKnown(input.directory_id);
								const located = yield* locator.Locate(selected.canonical_path);
								if (Option.isNone(located)) {
									return yield* Effect.fail(
										new ProjectDirectoryError({ code: "project_not_found" }),
									);
								}
								return located.value.project;
							}),
						),
						Effect.mapError((cause) =>
							cause instanceof ProjectDirectoryError
								? cause
								: new ProjectDirectoryError({ cause, code: "project_not_found" }),
						),
					),
			};
		}),
	);
}
