import { asc, eq, ne } from "drizzle-orm";
import { Effect, Layer, Schema } from "effect";

import { JournalSequence, StreamCursor, SurfaceItem } from "@artisan/protocol";

import { Database } from "../persistence/database";
import {
	SurfaceProjectionGenerations,
	SurfaceProjectionItems,
	SurfaceProjectionState,
} from "../persistence/schema";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import {
	SurfaceProjectionStore,
	SurfaceProjectionStoreConflict,
	SurfaceProjectionStoreFailure,
	type SurfaceProjectionSnapshot,
} from "./surface-projection-store";

const DecodeItems = Schema.decodeUnknownEffect(Schema.Array(SurfaceItem), {
	onExcessProperty: "error",
});

const DecodeCursors = Schema.decodeUnknownEffect(Schema.Array(StreamCursor), {
	onExcessProperty: "error",
});

const DecodeWatermark = Schema.decodeUnknownEffect(JournalSequence);

function failure(cause: unknown) {
	return new SurfaceProjectionStoreFailure({ cause });
}

function conflict(message: string) {
	return new SurfaceProjectionStoreConflict({ message });
}

const ParseJson = (value: string, context: string) =>
	Effect.try({
		try: () => JSON.parse(value) as unknown,
		catch: (cause) => failure(new Error(`${context} contains invalid JSON`, { cause })),
	});

const ValidateSnapshot = (snapshot: SurfaceProjectionSnapshot) =>
	Effect.gen(function* () {
		const watermark = yield* DecodeWatermark(snapshot.watermark).pipe(Effect.mapError(failure));
		const items = yield* DecodeItems(snapshot.items).pipe(Effect.mapError(failure));
		const stream_cursors = yield* DecodeCursors(snapshot.stream_cursors).pipe(
			Effect.mapError(failure),
		);
		const surface_ids = items.map(({ surface_id }) => surface_id);
		const stream_ids = stream_cursors.map(({ stream_id }) => stream_id);

		if (new Set(surface_ids).size !== surface_ids.length) {
			return yield* conflict("Surface projection contains duplicate item ids");
		}

		if (new Set(stream_ids).size !== stream_ids.length) {
			return yield* conflict("Surface projection contains duplicate stream cursors");
		}

		if (stream_cursors.some(({ sequence }) => sequence > watermark)) {
			return yield* conflict("Surface projection cursor exceeds its journal watermark");
		}

		if (
			(watermark === 0 && (items.length > 0 || stream_cursors.length > 0)) ||
			(watermark > 0 && stream_cursors.length === 0)
		) {
			return yield* conflict("Surface projection watermark does not match its contents");
		}

		return {
			items: [...items].sort((left, right) =>
				left.surface_id.localeCompare(right.surface_id),
			),
			stream_cursors: [...stream_cursors].sort((left, right) =>
				left.stream_id.localeCompare(right.stream_id),
			),
			watermark,
		} satisfies SurfaceProjectionSnapshot;
	});

/** Persists complete immutable surface generations and swaps readers atomically. */
export const SQLiteSurfaceProjectionStoreLive = Layer.effect(
	SurfaceProjectionStore,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;

		const Read = database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const [active] = yield* transaction
						.select({
							generation_id: SurfaceProjectionGenerations.generation_id,
							item_count: SurfaceProjectionGenerations.item_count,
							stream_cursors_json: SurfaceProjectionGenerations.stream_cursors_json,
							watermark: SurfaceProjectionGenerations.watermark,
						})
						.from(SurfaceProjectionState)
						.innerJoin(
							SurfaceProjectionGenerations,
							eq(
								SurfaceProjectionState.generation_id,
								SurfaceProjectionGenerations.generation_id,
							),
						)
						.where(eq(SurfaceProjectionState.state_id, 1))
						.limit(1);

					if (!active) {
						const [orphan] = yield* transaction
							.select({ generation_id: SurfaceProjectionGenerations.generation_id })
							.from(SurfaceProjectionGenerations)
							.limit(1);

						if (orphan) {
							return yield* conflict("Surface projection has no active generation");
						}

						return { items: [], stream_cursors: [], watermark: 0 };
					}

					const item_rows = yield* transaction
						.select({
							item_json: SurfaceProjectionItems.item_json,
							surface_id: SurfaceProjectionItems.surface_id,
						})
						.from(SurfaceProjectionItems)
						.where(eq(SurfaceProjectionItems.generation_id, active.generation_id))
						.orderBy(asc(SurfaceProjectionItems.surface_id));
					const parsed_items = yield* Effect.forEach(
						item_rows,
						({ item_json, surface_id }) =>
							ParseJson(item_json, `Surface projection ${surface_id}`).pipe(
								Effect.flatMap((item) =>
									Schema.decodeUnknownEffect(SurfaceItem, {
										onExcessProperty: "error",
									})(item),
								),
								Effect.mapError(failure),
								Effect.flatMap((item) =>
									item.surface_id === surface_id
										? Effect.succeed(item)
										: Effect.fail(
												conflict(
													`Surface projection row ${surface_id} is misbound`,
												),
											),
								),
							),
					);
					const parsed_cursors = yield* ParseJson(
						active.stream_cursors_json,
						`Surface generation ${active.generation_id}`,
					).pipe(Effect.flatMap(DecodeCursors), Effect.mapError(failure));

					if (parsed_items.length !== active.item_count) {
						return yield* conflict("Surface projection item count is corrupt");
					}

					return yield* ValidateSnapshot({
						items: parsed_items,
						stream_cursors: parsed_cursors,
						watermark: active.watermark,
					});
				}),
			)
			.pipe(
				Effect.mapError((cause) =>
					cause instanceof SurfaceProjectionStoreConflict ? cause : failure(cause),
				),
			);

		const Replace = (input: SurfaceProjectionSnapshot) =>
			Effect.gen(function* () {
				const snapshot = yield* ValidateSnapshot(input);
				const generation_id = yield* metadata.MakeId("projection");
				const created_at = yield* metadata.Now;
				const write = database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [current] = yield* transaction
							.select({ watermark: SurfaceProjectionGenerations.watermark })
							.from(SurfaceProjectionState)
							.innerJoin(
								SurfaceProjectionGenerations,
								eq(
									SurfaceProjectionState.generation_id,
									SurfaceProjectionGenerations.generation_id,
								),
							)
							.where(eq(SurfaceProjectionState.state_id, 1))
							.limit(1);

						if (current && current.watermark > snapshot.watermark) {
							return yield* conflict(
								"Surface projection cannot move to an older watermark",
							);
						}

						yield* transaction.insert(SurfaceProjectionGenerations).values({
							created_at,
							generation_id,
							item_count: snapshot.items.length,
							stream_cursors_json: JSON.stringify(snapshot.stream_cursors),
							watermark: snapshot.watermark,
						});

						if (snapshot.items.length > 0) {
							yield* transaction.insert(SurfaceProjectionItems).values(
								snapshot.items.map((item) => ({
									generation_id,
									item_json: JSON.stringify(Schema.encodeSync(SurfaceItem)(item)),
									surface_id: item.surface_id,
								})),
							);
						}

						yield* transaction
							.insert(SurfaceProjectionState)
							.values({ generation_id, state_id: 1 })
							.onConflictDoUpdate({
								set: { generation_id },
								target: SurfaceProjectionState.state_id,
							});
						yield* transaction
							.delete(SurfaceProjectionGenerations)
							.where(ne(SurfaceProjectionGenerations.generation_id, generation_id));
					}),
				);

				yield* RetrySqliteWrite(write);

				return snapshot;
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof SurfaceProjectionStoreConflict ? cause : failure(cause),
				),
			);

		return { Read, Replace };
	}),
);
