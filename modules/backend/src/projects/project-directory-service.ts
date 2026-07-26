import { Context, Data, Effect, FileSystem, Layer, Option, Path, Ref } from "effect";

import type {
	ProjectDirectoryEntry,
	ProjectDirectoryList,
	ProjectDirectoryListInput,
	ProjectDirectorySelectInput,
	ProjectRef,
} from "@artisan/protocol";
import { SnowflakeId } from "@artisan/protocol";
import { ProjectLocator } from "../threads/project-locator";

const maximum_directories = 256;

export class ProjectDirectoryError extends Data.TaggedError("ProjectDirectoryError")<{
	readonly cause?: unknown;
	readonly code: "invalid_directory" | "list_failed" | "project_not_found";
}> {}

export class ProjectDirectoryService extends Context.Service<
	ProjectDirectoryService,
	{
		readonly List: (
			input: ProjectDirectoryListInput,
		) => Effect.Effect<ProjectDirectoryList, ProjectDirectoryError>;
		readonly Select: (
			input: ProjectDirectorySelectInput,
		) => Effect.Effect<ProjectRef, ProjectDirectoryError>;
	}
>()("Artisan/ProjectDirectoryService") {}

interface KnownDirectory {
	readonly canonical_path: string;
	readonly root_path: string;
}

function inside(path_service: Path.Path, root: string, candidate: string) {
	const relative = path_service.relative(root, candidate);
	return relative === "" || (!relative.startsWith("..") && !path_service.isAbsolute(relative));
}

/** Builds a stateful browser-safe directory service over explicitly allowed server roots. */
export function make_project_directory_service_layer(allowed_roots: ReadonlyArray<string>) {
	return Layer.effect(
		ProjectDirectoryService,
		Effect.gen(function* () {
			const snowflake_id = yield* SnowflakeId;
			const file_system = yield* FileSystem.FileSystem;
			const path_service = yield* Path.Path;
			const locator = yield* ProjectLocator;
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

			const ChildEntries = (parent_directory_id: string) =>
				Effect.gen(function* () {
					const parent = yield* ResolveKnown(parent_directory_id);
					const names = (yield* file_system.readDirectory(parent.canonical_path))
						.toSorted((left, right) => left.localeCompare(right))
						.slice(0, maximum_directories);
					const entries = yield* Effect.forEach(names, (name) =>
						Effect.gen(function* () {
							const candidate = path_service.join(parent.canonical_path, name);
							const canonical_path = yield* file_system.realPath(candidate);
							const metadata = yield* file_system.stat(canonical_path);
							if (
								metadata.type !== "Directory" ||
								!inside(path_service, parent.root_path, canonical_path)
							) {
								return Option.none<ProjectDirectoryEntry>();
							}
							const directory_id = yield* Register({
								canonical_path,
								root_path: parent.root_path,
							});
							const children = yield* file_system.readDirectory(canonical_path);
							return Option.some({
								directory_id,
								display_name: name,
								has_children: children.length > 0,
								kind: "directory",
							} satisfies ProjectDirectoryEntry);
						}).pipe(Effect.catch(() => Effect.succeed(Option.none()))),
					);
					return entries.flatMap((entry) => (Option.isSome(entry) ? [entry.value] : []));
				});

			return {
				List: (input) =>
					(input.parent_directory_id === undefined
						? RootEntries
						: ChildEntries(input.parent_directory_id)
					).pipe(
						Effect.map((directories) => ({
							directories,
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
