import { Context, Data, Effect, Layer, Schema } from "effect";

import { StreamCursor, SurfaceItem, type SurfaceItem as SurfaceItemValue } from "@artisan/protocol";

import { JournalStore } from "../persistence/journal-store";
import { SurfaceProjector } from "./surface-projector";
import { SurfaceProjectionStore, type SurfaceProjectionSnapshot } from "./surface-projection-store";

export interface SurfaceProjectionDifference {
	readonly changed_stream_ids: ReadonlyArray<string>;
	readonly changed_surface_ids: ReadonlyArray<string>;
	readonly current_watermark: number;
	readonly equivalent: boolean;
	readonly missing_stream_ids: ReadonlyArray<string>;
	readonly missing_surface_ids: ReadonlyArray<string>;
	readonly projected_watermark: number;
	readonly unexpected_stream_ids: ReadonlyArray<string>;
	readonly unexpected_surface_ids: ReadonlyArray<string>;
}

export class SurfaceProjectionRebuildFailure extends Data.TaggedError(
	"SurfaceProjectionRebuildFailure",
)<{
	readonly cause: unknown;
	readonly stage: "journal" | "projection" | "store";
}> {}

/** Verifies and atomically replaces the canonical surface read model from one ledger snapshot. */
export class SurfaceProjectionRebuilder extends Context.Service<
	SurfaceProjectionRebuilder,
	{
		readonly Rebuild: Effect.Effect<SurfaceProjectionSnapshot, SurfaceProjectionRebuildFailure>;
		readonly Verify: Effect.Effect<
			SurfaceProjectionDifference,
			SurfaceProjectionRebuildFailure
		>;
	}
>()("Artisan/SurfaceProjectionRebuilder") {}

const DecodeItems = Schema.decodeUnknownEffect(Schema.Array(SurfaceItem), {
	onExcessProperty: "error",
});

const DecodeCursors = Schema.decodeUnknownEffect(Schema.Array(StreamCursor), {
	onExcessProperty: "error",
});

function sort_items(items: ReadonlyArray<SurfaceItemValue>) {
	return [...items].sort((left, right) => left.surface_id.localeCompare(right.surface_id));
}

function item_fingerprint(item: SurfaceItemValue) {
	return JSON.stringify(Schema.encodeSync(SurfaceItem)(item));
}

function compare_snapshots(
	current: SurfaceProjectionSnapshot,
	projected: SurfaceProjectionSnapshot,
): SurfaceProjectionDifference {
	const current_by_id = new Map(current.items.map((item) => [item.surface_id, item]));
	const projected_by_id = new Map(projected.items.map((item) => [item.surface_id, item]));
	const current_cursors = new Map(
		current.stream_cursors.map((cursor) => [cursor.stream_id, cursor.sequence]),
	);
	const projected_cursors = new Map(
		projected.stream_cursors.map((cursor) => [cursor.stream_id, cursor.sequence]),
	);
	const missing_surface_ids = [...projected_by_id.keys()]
		.filter((surface_id) => !current_by_id.has(surface_id))
		.sort();
	const unexpected_surface_ids = [...current_by_id.keys()]
		.filter((surface_id) => !projected_by_id.has(surface_id))
		.sort();
	const changed_surface_ids = [...projected_by_id.entries()]
		.filter(([surface_id, item]) => {
			const current_item = current_by_id.get(surface_id);

			return (
				current_item !== undefined &&
				item_fingerprint(current_item) !== item_fingerprint(item)
			);
		})
		.map(([surface_id]) => surface_id)
		.sort();
	const missing_stream_ids = [...projected_cursors.keys()]
		.filter((stream_id) => !current_cursors.has(stream_id))
		.sort();
	const unexpected_stream_ids = [...current_cursors.keys()]
		.filter((stream_id) => !projected_cursors.has(stream_id))
		.sort();
	const changed_stream_ids = [...projected_cursors.entries()]
		.filter(
			([stream_id, sequence]) =>
				current_cursors.has(stream_id) && current_cursors.get(stream_id) !== sequence,
		)
		.map(([stream_id]) => stream_id)
		.sort();

	return {
		changed_stream_ids,
		changed_surface_ids,
		current_watermark: current.watermark,
		equivalent:
			current.watermark === projected.watermark &&
			missing_surface_ids.length === 0 &&
			unexpected_surface_ids.length === 0 &&
			changed_surface_ids.length === 0 &&
			missing_stream_ids.length === 0 &&
			unexpected_stream_ids.length === 0 &&
			changed_stream_ids.length === 0,
		missing_stream_ids,
		missing_surface_ids,
		projected_watermark: projected.watermark,
		unexpected_stream_ids,
		unexpected_surface_ids,
	};
}

/** Provides deterministic projection verification and replacement over durable Services. */
export const SurfaceProjectionRebuilderLive = Layer.effect(
	SurfaceProjectionRebuilder,
	Effect.gen(function* () {
		const journal = yield* JournalStore;
		const projector = yield* SurfaceProjector;
		const store = yield* SurfaceProjectionStore;

		const Build = Effect.gen(function* () {
			const journal_snapshot = yield* journal
				.ReadSnapshot()
				.pipe(
					Effect.mapError(
						(cause) => new SurfaceProjectionRebuildFailure({ cause, stage: "journal" }),
					),
				);
			const projected_items = yield* projector
				.ProjectMany(journal_snapshot.events)
				.pipe(
					Effect.mapError(
						(cause) =>
							new SurfaceProjectionRebuildFailure({ cause, stage: "projection" }),
					),
				);
			const items = yield* DecodeItems(sort_items(projected_items)).pipe(
				Effect.mapError(
					(cause) => new SurfaceProjectionRebuildFailure({ cause, stage: "projection" }),
				),
			);
			const stream_cursors = yield* DecodeCursors(journal_snapshot.stream_cursors).pipe(
				Effect.mapError(
					(cause) => new SurfaceProjectionRebuildFailure({ cause, stage: "projection" }),
				),
			);

			return {
				items,
				stream_cursors,
				watermark: journal_snapshot.watermark,
			} satisfies SurfaceProjectionSnapshot;
		});

		const Rebuild = Build.pipe(
			Effect.flatMap(store.Replace),
			Effect.mapError((cause) =>
				cause instanceof SurfaceProjectionRebuildFailure
					? cause
					: new SurfaceProjectionRebuildFailure({ cause, stage: "store" }),
			),
		);
		const Verify = Effect.all({
			current: store.Read,
			projected: Build,
		}).pipe(
			Effect.map(({ current, projected }) => compare_snapshots(current, projected)),
			Effect.mapError((cause) =>
				cause instanceof SurfaceProjectionRebuildFailure
					? cause
					: new SurfaceProjectionRebuildFailure({ cause, stage: "store" }),
			),
		);

		return { Rebuild, Verify };
	}),
);
