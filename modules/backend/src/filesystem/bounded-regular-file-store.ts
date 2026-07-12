import { Context, Data, Effect } from "effect";

/** Names one operation owned by the bounded regular-file store. */
export type BoundedRegularFileStoreOperation = "finalize" | "read" | "replace";

/** Reports an opaque bounded regular-file operation failure. */
export class BoundedRegularFileStoreError extends Data.TaggedError("BoundedRegularFileStoreError")<{
	readonly cause: unknown;
	readonly operation: BoundedRegularFileStoreOperation;
	readonly path: string;
}> {}

/** Describes one conditional regular-file replacement and its durable receipt identity. */
export interface ReplaceRegularFileOptions {
	readonly expected: Uint8Array;
	readonly maximum_bytes: number;
	readonly operation_id: string;
	readonly path: string;
	readonly replacement: Uint8Array;
}

/** Describes the outcome of one conditional regular-file replacement attempt. */
export type ReplaceRegularFileResult =
	| { readonly _tag: "Replaced" }
	| { readonly _tag: "AlreadyReplaced" }
	| { readonly _tag: "Changed" };

/** Owns bounded reads and recoverable conditional publication for regular files. */
export class BoundedRegularFileStore extends Context.Service<
	BoundedRegularFileStore,
	{
		/** Removes a publication receipt only after the caller durably records `applied`. */
		readonly FinalizeRegularFileReplacement: (
			options: ReplaceRegularFileOptions,
		) => Effect.Effect<void, BoundedRegularFileStoreError>;
		readonly ReadRegularFile: (
			path: string,
			maximum_bytes: number,
		) => Effect.Effect<Uint8Array, BoundedRegularFileStoreError>;
		readonly ReplaceRegularFile: (
			options: ReplaceRegularFileOptions,
		) => Effect.Effect<ReplaceRegularFileResult, BoundedRegularFileStoreError>;
	}
>()("Artisan/BoundedRegularFileStore") {}

/** Exposes bounded regular-file reads without granting mutation or receipt cleanup. */
export type BoundedRegularFileReader = Pick<
	typeof BoundedRegularFileStore.Service,
	"ReadRegularFile"
>;
