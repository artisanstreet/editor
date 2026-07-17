import { Context, Data, Effect } from "effect";

import type { StreamCursor, SurfaceItem } from "@artisan/protocol";

export interface SurfaceProjectionSnapshot {
	readonly items: ReadonlyArray<SurfaceItem>;
	readonly stream_cursors: ReadonlyArray<StreamCursor>;
	readonly watermark: number;
}

export class SurfaceProjectionStoreConflict extends Data.TaggedError(
	"SurfaceProjectionStoreConflict",
)<{
	readonly message: string;
}> {}

export class SurfaceProjectionStoreFailure extends Data.TaggedError(
	"SurfaceProjectionStoreFailure",
)<{
	readonly cause: unknown;
}> {}

export type SurfaceProjectionStoreError =
	| SurfaceProjectionStoreConflict
	| SurfaceProjectionStoreFailure;

/** Owns atomic generations of the canonical source-safe activity projection. */
export class SurfaceProjectionStore extends Context.Service<
	SurfaceProjectionStore,
	{
		readonly Read: Effect.Effect<SurfaceProjectionSnapshot, SurfaceProjectionStoreError>;
		readonly Replace: (
			snapshot: SurfaceProjectionSnapshot,
		) => Effect.Effect<SurfaceProjectionSnapshot, SurfaceProjectionStoreError>;
	}
>()("Artisan/SurfaceProjectionStore") {}
