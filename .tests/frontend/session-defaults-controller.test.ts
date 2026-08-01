import { readFileSync } from "node:fs";

import { Deferred, Effect, Fiber, Layer, Ref } from "effect";
import { describe, expect, it } from "vitest";

import type { SessionDefaultsUpdateInput } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";
import {
	CompactionSelectionFromDefaults,
	SessionDefaultsController,
	SessionDefaultsControllerLive,
} from "../../modules/frontend/src/lib/settings/session-defaults-controller";

describe("session defaults controller", () => {
	it("is the single application owner for catalog and session-default hydration", () => {
		const paths = [
			"modules/frontend/src/routes/+layout.sv",
			"modules/frontend/src/routes/+page.sv",
			"modules/frontend/src/routes/components/thread-composer.sv",
			"modules/frontend/src/routes/components/settings/engine.sv",
			"modules/frontend/src/routes/components/settings/nav.sv",
			"modules/frontend/src/routes/components/settings/models.sv",
			"modules/frontend/src/routes/components/settings/compaction-model.sv",
		];

		for (const path of paths) {
			const source = readFileSync(path, "utf8");
			expect(source, path).toContain("yield* SessionDefaultsController");
			expect(source, path).not.toContain("client.GetSessionDefaults");
			expect(source, path).not.toContain("client.GetRuntimeCatalog");
			expect(source, path).not.toContain("RuntimeCatalogChanges");
		}

		const offline_catalog = readFileSync(
			"modules/frontend/src/lib/runtime/offline-catalog.ts",
			"utf8",
		);
		expect(offline_catalog).not.toContain("RuntimeCatalogChanges");
	});

	it("models curated, inherited, and explicit compaction selections as tagged values", () => {
		expect(CompactionSelectionFromDefaults({ models: [], permission: "supervised" })).toEqual({
			_tag: "Curated",
		});
		expect(
			CompactionSelectionFromDefaults({
				compaction_model: "inherited",
				models: [],
				permission: "supervised",
			}),
		).toEqual({ _tag: "Inherited" });
		expect(
			CompactionSelectionFromDefaults({
				compaction_model: "codex:gpt-5.6-codex",
				models: [],
				permission: "supervised",
			}),
		).toEqual({ _tag: "Explicit", model_id: "codex:gpt-5.6-codex" });
	});

	it("persists an explicit compaction model and its controls in one atomic patch", async () => {
		const updates = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const captured = yield* Ref.make<ReadonlyArray<SessionDefaultsUpdateInput>>([]);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						UpdateSessionDefaults: (input) =>
							Effect.gen(function* () {
								yield* Ref.update(captured, (current) => [...current, input]);
								return yield* FixtureArtisanClientService.UpdateSessionDefaults();
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(SessionDefaultsControllerLive, client_layer),
					);
					yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						yield* controller.SaveCompactionDefaults({
							model: {
								context_window: "1m",
								model_id: "codex:gpt-5.6-codex",
								reasoning_effort: "high",
							},
							permission: "unrestricted",
							selection: { _tag: "Explicit", model_id: "codex:gpt-5.6-codex" },
						});
					}).pipe(Effect.provide(services));
					return yield* Ref.get(captured);
				}),
			),
		);

		expect(updates).toEqual([
			{
				compaction_model: "codex:gpt-5.6-codex",
				model: {
					context_window: "1m",
					model_id: "codex:gpt-5.6-codex",
					reasoning_effort: "high",
				},
				permission: "unrestricted",
			},
		]);
	});

	it("keeps rapid context and reasoning changes as independent model patches", async () => {
		const updates = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const captured = yield* Ref.make<ReadonlyArray<SessionDefaultsUpdateInput>>([]);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						UpdateSessionDefaults: (input) =>
							Effect.gen(function* () {
								yield* Ref.update(captured, (current) => [...current, input]);
								return yield* FixtureArtisanClientService.UpdateSessionDefaults();
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(SessionDefaultsControllerLive, client_layer),
					);
					yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						yield* Effect.all(
							[
								controller.SaveCompactionDefaults({
									model: {
										context_window: "1m",
										model_id: "codex:gpt-5.6-codex",
									},
									selection: {
										_tag: "Explicit",
										model_id: "codex:gpt-5.6-codex",
									},
								}),
								controller.SaveCompactionDefaults({
									model: {
										model_id: "codex:gpt-5.6-codex",
										reasoning_effort: "high",
									},
									selection: {
										_tag: "Explicit",
										model_id: "codex:gpt-5.6-codex",
									},
								}),
							],
							{ concurrency: "unbounded", discard: true },
						);
					}).pipe(Effect.provide(services));
					return yield* Ref.get(captured);
				}),
			),
		);

		expect(updates).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					model: { context_window: "1m", model_id: "codex:gpt-5.6-codex" },
				}),
				expect.objectContaining({
					model: { model_id: "codex:gpt-5.6-codex", reasoning_effort: "high" },
				}),
			]),
		);
	});

	it("serializes a stale refresh behind a newer defaults save", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const refresh_started = yield* Deferred.make<void>();
					const release_refresh = yield* Deferred.make<void>();
					const saved = yield* Ref.make(false);
					const favorite_reads = yield* Ref.make(0);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetModelFavorites: Effect.gen(function* () {
							const read = yield* Ref.updateAndGet(
								favorite_reads,
								(count) => count + 1,
							);
							if (read === 1) {
								yield* Deferred.succeed(refresh_started, undefined);
								yield* Deferred.await(release_refresh);
							}
							return { model_ids: [] };
						}),
						GetSessionDefaults: Effect.gen(function* () {
							return (yield* Ref.get(saved))
								? {
										compaction_model: "codex:gpt-5.6-codex",
										models: [],
										permission: "supervised" as const,
									}
								: { models: [], permission: "supervised" as const };
						}),
						UpdateSessionDefaults: () =>
							Effect.gen(function* () {
								yield* Ref.set(saved, true);
								return yield* FixtureArtisanClientService.UpdateSessionDefaults();
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(SessionDefaultsControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						const refresh = yield* controller.Refresh.pipe(Effect.forkChild);
						yield* Deferred.await(refresh_started);
						const save = yield* controller
							.SaveCompactionDefaults({
								selection: { _tag: "Explicit", model_id: "codex:gpt-5.6-codex" },
							})
							.pipe(Effect.forkChild);
						yield* Deferred.succeed(release_refresh, undefined);
						yield* Fiber.join(refresh);
						yield* Fiber.join(save);
						return yield* controller.Current;
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.defaults.compaction_model).toBe("codex:gpt-5.6-codex");
	});
});
