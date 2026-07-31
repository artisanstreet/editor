import { describe, expect, it } from "@effect/vitest";
import { Effect, Ref, Schema, Stream } from "effect";
import { ArtisanClient, ArtisanClientError } from "@artisan/transport/client";

import { RuntimeCatalog } from "../../modules/protocol/src/runtime-catalog";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";
import {
	IsOfflineRuntimeCatalog,
	OfflineRuntimeCatalog,
	RuntimeCatalogChanges,
	WithOfflineRuntimeCatalog,
} from "../../modules/frontend/src/lib/runtime/offline-catalog";

describe("offline runtime catalog", () => {
	it("is a valid catalog carrying the whole manifest with nothing runnable", () => {
		const decoded = Schema.decodeUnknownSync(RuntimeCatalog)(OfflineRuntimeCatalog);

		expect(decoded.runnable_harness_ids).toEqual([]);
		expect(decoded.default_model_id).toBeUndefined();
		expect(decoded.manifest.models.length).toBeGreaterThan(0);
		expect(decoded.manifest.harnesses.length).toBeGreaterThan(0);
	});

	it("substitutes only for a failing catalog query", () => {
		const connected = {
			manifest: OfflineRuntimeCatalog.manifest,
			runnable_harness_ids: ["codex"],
		};

		expect(Effect.runSync(WithOfflineRuntimeCatalog(Effect.succeed(connected)))).toBe(
			connected,
		);
		expect(
			Effect.runSync(WithOfflineRuntimeCatalog(Effect.fail(new Error("socket closed")))),
		).toBe(OfflineRuntimeCatalog);
	});

	it("distinguishes the offline catalog from a connected one", () => {
		expect(IsOfflineRuntimeCatalog(OfflineRuntimeCatalog)).toBe(true);
		/** A connected Forge with no registered engine is not the offline case. */
		expect(
			IsOfflineRuntimeCatalog({
				manifest: OfflineRuntimeCatalog.manifest,
				runnable_harness_ids: [],
			}),
		).toBe(false);
	});

	it.effect("replaces an offline mount snapshot after Forge reconnects", () =>
		Effect.gen(function* () {
			const attempts = yield* Ref.make(0);
			const connected: RuntimeCatalog = {
				manifest: OfflineRuntimeCatalog.manifest,
				runnable_harness_ids: ["codex"],
			};
			const disconnected = new ArtisanClientError({
				cause: new Error("socket closed"),
				code: "connection",
				message: "Transport bootstrap failed.",
				protocol_code: "transport.connection",
				retryable: true,
			});
			const client = {
				...FixtureArtisanClientService,
				ConnectionChanges: Stream.succeed({ phase: "ready" as const }),
				ConnectionState: Effect.succeed({ phase: "reconnecting" as const }),
				GetRuntimeCatalog: Ref.updateAndGet(attempts, (count) => count + 1).pipe(
					Effect.flatMap((attempt) =>
						attempt === 1 ? Effect.fail(disconnected) : Effect.succeed(connected),
					),
				),
			} satisfies typeof ArtisanClient.Service;

			const catalogs = yield* RuntimeCatalogChanges.pipe(
				Stream.take(2),
				Stream.runCollect,
				Effect.provideService(ArtisanClient, client),
			);

			expect(catalogs).toEqual([OfflineRuntimeCatalog, connected]);
		}),
	);

	it.effect("returns to the offline sentinel when a live Forge disconnects", () =>
		Effect.gen(function* () {
			const connected: RuntimeCatalog = {
				manifest: OfflineRuntimeCatalog.manifest,
				runnable_harness_ids: ["codex"],
			};
			const client = {
				...FixtureArtisanClientService,
				ConnectionChanges: Stream.succeed({ phase: "reconnecting" as const }),
				ConnectionState: Effect.succeed({ phase: "ready" as const }),
				GetRuntimeCatalog: Effect.succeed(connected),
			} satisfies typeof ArtisanClient.Service;

			const catalogs = yield* RuntimeCatalogChanges.pipe(
				Stream.take(2),
				Stream.runCollect,
				Effect.provideService(ArtisanClient, client),
			);

			expect(catalogs).toEqual([connected, OfflineRuntimeCatalog]);
		}),
	);
});
