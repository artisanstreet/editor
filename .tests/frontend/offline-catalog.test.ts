import { describe, expect, it } from "@effect/vitest";
import { Effect, Ref, Schema, Stream } from "effect";
import { ArtisanClient } from "@artisan/transport/client";

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

	it.effect("queries once from the replayed ready connection state", () =>
		Effect.gen(function* () {
			const attempts = yield* Ref.make(0);
			const connected: RuntimeCatalog = {
				manifest: OfflineRuntimeCatalog.manifest,
				runnable_harness_ids: ["codex"],
			};
			const client = {
				...FixtureArtisanClientService,
				ConnectionChanges: Stream.fromIterable([
					{ phase: "ready" as const },
					{ phase: "ready" as const },
				]),
				GetRuntimeCatalog: Ref.update(attempts, (count) => count + 1).pipe(
					Effect.as(connected),
				),
			} satisfies typeof ArtisanClient.Service;

			const catalogs = yield* RuntimeCatalogChanges.pipe(
				Stream.runCollect,
				Effect.provideService(ArtisanClient, client),
			);

			expect(catalogs).toEqual([connected]);
			expect(yield* Ref.get(attempts)).toBe(1);
		}),
	);

	it.effect("refreshes once per real reconnect without duplicating phase work", () =>
		Effect.gen(function* () {
			const attempts = yield* Ref.make(0);
			const connected: RuntimeCatalog = {
				manifest: OfflineRuntimeCatalog.manifest,
				runnable_harness_ids: ["codex"],
			};
			const client = {
				...FixtureArtisanClientService,
				ConnectionChanges: Stream.fromIterable([
					{ phase: "ready" as const },
					{ phase: "reconnecting" as const },
					{ phase: "reconnecting" as const },
					{ phase: "ready" as const },
				]),
				GetRuntimeCatalog: Ref.update(attempts, (count) => count + 1).pipe(
					Effect.as(connected),
				),
			} satisfies typeof ArtisanClient.Service;

			const catalogs = yield* RuntimeCatalogChanges.pipe(
				Stream.runCollect,
				Effect.provideService(ArtisanClient, client),
			);

			expect(catalogs).toEqual([connected, OfflineRuntimeCatalog, connected]);
			expect(yield* Ref.get(attempts)).toBe(2);
		}),
	);

	it.effect("stays offline without querying until the connection becomes ready", () =>
		Effect.gen(function* () {
			const attempts = yield* Ref.make(0);
			const connected: RuntimeCatalog = {
				manifest: OfflineRuntimeCatalog.manifest,
				runnable_harness_ids: ["codex"],
			};
			const client = {
				...FixtureArtisanClientService,
				ConnectionChanges: Stream.fromIterable([
					{ phase: "connecting" as const },
					{ phase: "reconnecting" as const },
					{ phase: "ready" as const },
				]),
				GetRuntimeCatalog: Ref.update(attempts, (count) => count + 1).pipe(
					Effect.as(connected),
				),
			} satisfies typeof ArtisanClient.Service;

			const catalogs = yield* RuntimeCatalogChanges.pipe(
				Stream.runCollect,
				Effect.provideService(ArtisanClient, client),
			);

			expect(catalogs).toEqual([OfflineRuntimeCatalog, connected]);
			expect(yield* Ref.get(attempts)).toBe(1);
		}),
	);
});
