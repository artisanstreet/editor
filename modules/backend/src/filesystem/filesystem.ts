import { Context, Data, Effect, Stream } from "effect";

/** Describes a filesystem operation that failed. */
export type FilesystemOperation =
	| "create"
	| "delete"
	| "list"
	| "read"
	| "rename"
	| "resolve"
	| "stat"
	| "watch"
	| "write";

/** Reports a filesystem failure without exposing platform-specific errors. */
export class FilesystemError extends Data.TaggedError("FilesystemError")<{
	readonly cause: unknown;
	readonly operation: FilesystemOperation;
	readonly path?: string;
}> {}

/** Identifies the kind of filesystem entry. */
export type FilesystemEntryKind = "file" | "directory" | "symlink" | "other";

/** Describes one filesystem entry and its metadata. */
export interface FilesystemEntry {
	readonly created_at: string;
	readonly kind: FilesystemEntryKind;
	readonly mode: number;
	readonly modified_at: string;
	readonly path: string;
	readonly size: number;
}

/** Describes one normalized path change from a filesystem watch. */
export interface FilesystemPathChange {
	readonly kind: "created" | "modified" | "deleted" | "renamed";
	readonly path: string;
}

export type FilesystemChange = FilesystemPathChange;

/** Provides project-root-confined filesystem operations. */
export class Filesystem extends Context.Service<
	Filesystem,
	{
		readonly CreateDirectory: (path: string) => Effect.Effect<void, FilesystemError>;
		readonly CreateFile: (
			path: string,
			data?: Uint8Array,
		) => Effect.Effect<FilesystemEntry, FilesystemError>;
		readonly DeleteToTrash: (path: string) => Effect.Effect<string, FilesystemError>;
		readonly List: (
			path?: string,
		) => Effect.Effect<ReadonlyArray<FilesystemEntry>, FilesystemError>;
		readonly Read: (path: string) => Effect.Effect<Uint8Array, FilesystemError>;
		readonly ReadText: (path: string) => Effect.Effect<string, FilesystemError>;
		readonly Rename: (
			from: string,
			to: string,
		) => Effect.Effect<FilesystemEntry, FilesystemError>;
		readonly Resolve: (path: string) => Effect.Effect<string, FilesystemError>;
		readonly Stat: (path: string) => Effect.Effect<FilesystemEntry, FilesystemError>;
		readonly Watch: (path?: string) => Stream.Stream<FilesystemChange, FilesystemError>;
		readonly WriteAtomic: (
			path: string,
			data: Uint8Array,
		) => Effect.Effect<FilesystemEntry, FilesystemError>;
	}
>()("Artisan/Filesystem") {}
