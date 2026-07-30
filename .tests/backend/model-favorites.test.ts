import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	type HelloEnvelope,
	type ModelFavoritesQueryEnvelope,
	type ModelFavoriteUpdateEnvelope,
} from "@artisan/protocol";
import { make_backend_runtime, ProtocolServer } from "@artisan/backend";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-model-favorites-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer(now: { value: string } = { value: "2026-07-29T12:00:00.000Z" }) {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "backend_model_favorites_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.sync(() => now.value),
	});
}

function make_hello(): HelloEnvelope {
	return {
		kind: "hello",
		message_id: "hello_1",
		origin: "frontend",
		payload: {
			event_cursors: [],
			last_journal_sequence: 0,
			supported_protocol_versions: [1],
		},
		schema_version: 1,
		sent_at: "2026-07-29T12:00:00.000Z",
	};
}

function make_query(message_id: string): ModelFavoritesQueryEnvelope {
	return {
		kind: "model.favorites.query",
		message_id,
		origin: "frontend",
		payload: {},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-29T12:00:00.000Z",
	};
}

function make_update(
	message_id: string,
	model_id: string,
	favorite: boolean,
): ModelFavoriteUpdateEnvelope {
	return {
		kind: "model.favorite.update",
		message_id,
		origin: "frontend",
		payload: { favorite, model_id },
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-29T12:00:00.000Z",
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("model favorites", () => {
	/**
	 * The harness clock is fixed, so both stars share a timestamp and the order
	 * below is the deterministic model-id tiebreak rather than insertion order.
	 * That tiebreak is the point: two favorites recorded in the same instant
	 * must not reshuffle between reads.
	 */
	it("starts empty, durably stars models, and orders them deterministically", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const result = await runtime.runPromise(
				Effect.scoped(
					Effect.gen(function* () {
						const server = yield* ProtocolServer;
						const connection = yield* server.Open;

						yield* connection.Receive(make_hello());
						yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);

						yield* connection.Receive(make_query("query_empty"));
						const [initial] = yield* connection.Outbound.pipe(
							Stream.take(1),
							Stream.runCollect,
						);

						const star = make_update("favorite_1", "codex-sol", true);
						yield* connection.Receive(star);
						const first = yield* connection.Outbound.pipe(
							Stream.take(2),
							Stream.runCollect,
						);

						/** The same command id must never star a second time. */
						yield* connection.Receive(star);
						const retry = yield* connection.Outbound.pipe(
							Stream.take(1),
							Stream.runCollect,
						);

						yield* connection.Receive(make_update("favorite_2", "claude-opus-5", true));
						yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);

						yield* connection.Receive(make_query("query_starred"));
						const [starred] = yield* connection.Outbound.pipe(
							Stream.take(1),
							Stream.runCollect,
						);

						yield* connection.Receive(make_update("favorite_3", "codex-sol", false));
						yield* connection.Outbound.pipe(Stream.take(2), Stream.runCollect);

						yield* connection.Receive(make_query("query_unstarred"));
						const [unstarred] = yield* connection.Outbound.pipe(
							Stream.take(1),
							Stream.runCollect,
						);

						return { first, initial, retry, starred, unstarred };
					}),
				),
			);

			expect(result.initial).toMatchObject({
				kind: "model.favorites.query.result",
				payload: { model_ids: [] },
			});
			expect(result.first[0]).toMatchObject({ payload: { status: "accepted" } });
			expect(result.retry[0]).toMatchObject({ payload: { status: "duplicate" } });
			expect(result.starred).toMatchObject({
				kind: "model.favorites.query.result",
				payload: { model_ids: ["claude-opus-5", "codex-sol"] },
			});
			expect(result.unstarred).toMatchObject({
				kind: "model.favorites.query.result",
				payload: { model_ids: ["claude-opus-5"] },
			});
		} finally {
			await runtime.dispose();
		}
	});
});
