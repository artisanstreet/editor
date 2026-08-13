import { Context, Data, Effect, FileSystem, Layer, Option, Path, Ref } from "effect";

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
) {
	return Layer.effect(
		ProjectDirectoryService,
		Effect.gen(function* () {
			const snowflake_id = yield* SnowflakeId;
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const locator = yield* ProjectLocator;
			const native_picker = yield* NativeDirectoryPicker;
			const by_id = yield* Ref.make(new Map<string, KnownDirectory>());
			const by_path = yield* Ref.make(new Map<string, string>());

			const Register = (known: KnownDirectory) =>
				Effect.gen(function* () {
					const existing = (yield* Ref.get(by_path)).get(known.canonical_path);
					if (existing !== undefined) return existing;
					const directory_id = yield* snowflake_id.Make("directory");
					yield* Ref.update(by_id, (current) =>
						new Map(current).set(directory_id, known),
					);
					yield* Ref.update(by_path, (current) =>
						new Map(current).set(known.canonical_path, directory_id),
					);
					return directory_id;
				});

			const roots = yield* Effect.forEach(allowed_roots, (root) =>
				Effect.gen(function* () {
					const canonical_path = yield* file_system.realPath(root);
					const metadata = yield* file_system.stat(canonical_path);
					if (metadata.type !== "Directory") {
						return yield* Effect.fail(
							new Error("Allowed project root is not a directory"),
						);
					}
					return canonical_path;
				}),
			).pipe(
				Effect.map((items) => [...new Set(items)]),
				Effect.mapError(
					(cause) => new ProjectDirectoryError({ cause, code: "list_failed" }),
				),
			);

			const RootEntries = Effect.forEach(roots, (root_path) =>
				Effect.gen(function* () {
					const directory_id = yield* Register({ canonical_path: root_path, root_path });
					return {
						directory_id,
						display_name: path_service.basename(root_path) || root_path,
						// Configured roots remain browsable even when probing a mounted
						// directory would be slow. The bounded child request owns that I/O.
						has_children: true,
						kind: "root",
					} satisfies ProjectDirectoryEntry;
				}),
			);

			/**
			 * Resolved once at construction: the shortcuts are stable for the
			 * process lifetime, and every one of them must land inside an
			 * allowed root to be offered at all.
			 */
			const places = yield* Effect.gen(function* () {
				if (home_directory === undefined) return [];
				const resolved = yield* Effect.forEach(known_places, (candidate) =>
					Effect.gen(function* () {
						const target =
							candidate.folder_name === undefined
								? home_directory
								: path_service.join(home_directory, candidate.folder_name);
						const canonical_path = yield* file_system.realPath(target);
						const metadata = yield* file_system.stat(canonical_path);
						const root_path = roots.find((root) =>
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
					}).pipe(Effect.catch(() => Effect.succeed(Option.none()))),
				);
				return resolved.flatMap((place) => (Option.isSome(place) ? [place.value] : []));
			});

			const ResolveKnown = (directory_id: string) =>
				Effect.gen(function* () {
					const known = (yield* Ref.get(by_id)).get(directory_id);
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
					const entries = yield* Effect.forEach(names, (name) =>
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
							const children = yield* file_system.readDirectory(canonical_path);
							return Option.some({
								entry: {
									directory_id,
									display_name: name,
									has_children: children.length > 0,
									kind: "directory",
								} satisfies ProjectDirectoryEntry,
								type: "directory" as const,
							});
						}).pipe(Effect.catch(() => Effect.succeed(Option.none()))),
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
					}).pipe(
						Effect.mapError((cause) =>
							cause instanceof ProjectDirectoryError
								? cause
								: new ProjectDirectoryError({ cause, code: "create_failed" }),
						),
					),
				List: (input) =>
					(input.parent_directory_id === undefined
						? RootEntries.pipe(
								Effect.map(
									(directories): ChildListing => ({ directories, files: [] }),
								),
							)
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
						Effect.mapError((cause) =>
							cause instanceof ProjectDirectoryError
								? cause
								: new ProjectDirectoryError({ cause, code: "list_failed" }),
						),
					),
				Pick: Effect.gen(function* () {
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
				}).pipe(
					Effect.mapError((cause) =>
						cause instanceof ProjectDirectoryError
							? cause
							: new ProjectDirectoryError({ cause, code: "pick_failed" }),
					),
				),
				Select: (input) =>
					Effect.gen(function* () {
						const selected = yield* ResolveKnown(input.directory_id);
						const located = yield* locator.Locate(selected.canonical_path);
						if (Option.isNone(located)) {
							return yield* Effect.fail(
								new ProjectDirectoryError({ code: "project_not_found" }),
							);
						}
						return located.value.project;
					}).pipe(
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
