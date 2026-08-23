import { readFileSync } from "node:fs";

import { Deferred, Effect, Fiber, Layer, Ref } from "effect";
import { describe, expect, it } from "vitest";

import { model_manifest } from "@artisan/catalog";
import type { SessionDefaultsUpdateInput, ThreadSessionPolicy } from "@artisan/protocol";
import { ArtisanClient, ArtisanClientError } from "@artisan/transport/client";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";
import {
	CompactionSelectionFromDefaults,
	SessionDefaultsController,
	SessionDefaultsControllerLive,
} from "../../modules/frontend/src/lib/settings/session-defaults-controller";

describe("session defaults controller", () => {
	it("atomically admits hydration flights while leaving only deferred waits interruptible", () => {
		const source = readFileSync(
			"modules/frontend/src/lib/settings/session-defaults-controller.ts",
			"utf8",
		);
		expect(source).toContain("Effect.uninterruptibleMask((restore)");
		expect(source).toContain("Effect.forkIn(");
		expect(source).toContain("restore(Deferred.await(claim.deferred))");
	});

	it("is the single application owner for catalog and session-default hydration", () => {
		const paths = [
			"modules/frontend/src/routes/+layout.svelte",
			"modules/frontend/src/routes/components/new-thread-route.svelte",
			"modules/frontend/src/routes/components/thread-composer.svelte",
			"modules/frontend/src/routes/components/settings/engine.svelte",
			"modules/frontend/src/routes/components/settings/nav.svelte",
			"modules/frontend/src/routes/components/settings/models.svelte",
			"modules/frontend/src/routes/components/settings/compaction-model.svelte",
			"modules/frontend/src/routes/components/settings/agent-names.svelte",
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

	it("uses the shell's cached defaults for new-thread and selector mounts", () => {
		const new_thread_route = readFileSync(
			"modules/frontend/src/routes/components/new-thread-route.svelte",
			"utf8",
		);
		const selector = readFileSync(
			"modules/frontend/src/routes/components/model-selector/view.svelte",
			"utf8",
		);
		const layout = readFileSync("modules/frontend/src/routes/+layout.svelte", "utf8");

		expect(new_thread_route).toContain("const snapshot = yield* session_defaults.Current;");
		expect(new_thread_route).not.toContain("session_defaults.Refresh");
		expect(selector).toContain("yield* defaults_controller.Current");
		expect(selector).toContain("defaults_controller.Changes");
		expect(selector).not.toContain("ArtisanClient");
		expect(selector).not.toContain("client.ConnectionChanges");
		expect(layout).toContain("session_defaults.Refresh.pipe(");
	});

	it("starts one concurrent hydration triple and shares it with concurrent refreshes", async () => {
		const calls = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const catalog_started = yield* Deferred.make<void>();
					const defaults_started = yield* Deferred.make<void>();
					const favorites_started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const counts = yield* Ref.make({ catalog: 0, defaults: 0, favorites: 0 });
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetRuntimeCatalog: Effect.gen(function* () {
							yield* Ref.update(counts, (current) => ({
								...current,
								catalog: current.catalog + 1,
							}));
							yield* Deferred.succeed(catalog_started, undefined);
							yield* Deferred.await(release);
							return yield* FixtureArtisanClientService.GetRuntimeCatalog;
						}),
						GetSessionDefaults: Effect.gen(function* () {
							yield* Ref.update(counts, (current) => ({
								...current,
								defaults: current.defaults + 1,
							}));
							yield* Deferred.succeed(defaults_started, undefined);
							yield* Deferred.await(release);
							return yield* FixtureArtisanClientService.GetSessionDefaults;
						}),
						GetModelFavorites: Effect.gen(function* () {
							yield* Ref.update(counts, (current) => ({
								...current,
								favorites: current.favorites + 1,
							}));
							yield* Deferred.succeed(favorites_started, undefined);
							yield* Deferred.await(release);
							return yield* FixtureArtisanClientService.GetModelFavorites;
						}),
					});
					const services = yield* Layer.build(
						Layer.provide(SessionDefaultsControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						const first = yield* controller.Refresh.pipe(Effect.forkChild);
						yield* Effect.all(
							[
								Deferred.await(catalog_started),
								Deferred.await(defaults_started),
								Deferred.await(favorites_started),
							],
							{ concurrency: "unbounded", discard: true },
						);
						const second = yield* controller.Refresh.pipe(Effect.forkChild);
						yield* Effect.sleep("1 millis");
						expect(yield* Ref.get(counts)).toEqual({
							catalog: 1,
							defaults: 1,
							favorites: 1,
						});
						yield* Deferred.succeed(release, undefined);
						yield* Fiber.join(first);
						yield* Fiber.join(second);
						return yield* Ref.get(counts);
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(calls).toEqual({ catalog: 1, defaults: 1, favorites: 1 });
	});

	it("keeps shared hydration alive when the caller that started it is interrupted", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const catalog_reads = yield* Ref.make(0);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetRuntimeCatalog: Effect.gen(function* () {
							yield* Ref.update(catalog_reads, (count) => count + 1);
							yield* Deferred.succeed(started, undefined);
							yield* Deferred.await(release);
							return yield* FixtureArtisanClientService.GetRuntimeCatalog;
						}),
					});
					const services = yield* Layer.build(
						Layer.provide(SessionDefaultsControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						const leader = yield* controller.Refresh.pipe(Effect.forkChild);
						yield* Deferred.await(started);
						const follower = yield* controller.Refresh.pipe(Effect.forkChild);
						yield* Effect.yieldNow;
						yield* Fiber.interrupt(leader);
						yield* Deferred.succeed(release, undefined);
						const snapshot = yield* Fiber.join(follower);
						return { reads: yield* Ref.get(catalog_reads), snapshot };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.reads).toBe(1);
		expect(result.snapshot.available).toBe(true);
	});

	it("persists the selected name dataset as a defaults patch", async () => {
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
						yield* controller.SetAgentNameDataset("british");
					}).pipe(Effect.provide(services));
					return yield* Ref.get(captured);
				}),
			),
		);

		expect(updates).toEqual([{ agent_name_dataset: "british" }]);
	});

	it("patches accepted defaults writes locally without a read-back", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetSessionDefaults: Effect.gen(function* () {
							yield* Ref.update(reads, (count) => count + 1);
							return yield* FixtureArtisanClientService.GetSessionDefaults;
						}),
					});
					const services = yield* Layer.build(
						Layer.provide(SessionDefaultsControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						const next = yield* controller.SaveCompactionDefaults({
							model: {
								model_id: "codex:gpt-5.6-codex",
								reasoning_effort: "high",
							},
							permission: "unrestricted",
							selection: { _tag: "Explicit", model_id: "codex:gpt-5.6-codex" },
						});
						return { reads: yield* Ref.get(reads), next };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.reads).toBe(0);
		expect(result.next.defaults).toMatchObject({
			compaction_model: "codex:gpt-5.6-codex",
			permission: "unrestricted",
		});
		expect(result.next.defaults.models).toEqual([
			{ model_id: "codex:gpt-5.6-codex", reasoning_effort: "high" },
		]);
	});

	it("patches accepted favorite writes locally without a read-back", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetModelFavorites: Effect.gen(function* () {
							yield* Ref.update(reads, (count) => count + 1);
							return yield* FixtureArtisanClientService.GetModelFavorites;
						}),
					});
					const services = yield* Layer.build(
						Layer.provide(SessionDefaultsControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						yield* controller.SetFavorite("codex:gpt-5.6-codex", true);
						yield* controller.SetFavorite("codex:gpt-5.6-codex", false);
						return { reads: yield* Ref.get(reads), state: yield* controller.Current };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.reads).toBe(0);
		expect(result.state.favorite_ids).toEqual([]);
	});

	it("returns rejected defaults receipts before one app-owned correction worker and fences a ready stale read behind a later save", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const correction_started = yield* Deferred.make<void>();
					const release_correction = yield* Deferred.make<void>();
					const success_started = yield* Deferred.make<void>();
					const release_success = yield* Deferred.make<void>();
					const reads = yield* Ref.make(0);
					let writes = 0;
					const services = yield* Layer.build(
						Layer.provide(
							SessionDefaultsControllerLive,
							Layer.succeed(ArtisanClient, {
								...FixtureArtisanClientService,
								GetSessionDefaults: Effect.gen(function* () {
									yield* Ref.update(reads, (count) => count + 1);
									yield* Deferred.succeed(correction_started, undefined);
									yield* Deferred.await(release_correction);
									return { models: [], permission: "supervised" as const };
								}),
								UpdateSessionDefaults: () => {
									writes += 1;
									return writes < 3
										? Effect.fail(
												new ArtisanClientError({
													cause: undefined,
													code: "connection",
													message: "offline",
													protocol_code: "offline",
													retryable: true,
												}),
											)
										: Effect.gen(function* () {
												yield* Deferred.succeed(success_started, undefined);
												yield* Deferred.await(release_success);
												return yield* FixtureArtisanClientService.UpdateSessionDefaults();
											});
								},
							}),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						expect(
							(yield* controller.SetAutoContinueUsageLimits(false).pipe(Effect.exit))
								._tag,
						).toBe("Failure");
						yield* Deferred.await(correction_started);
						expect(
							(yield* controller.SetAutoContinueUsageLimits(false).pipe(Effect.exit))
								._tag,
						).toBe("Failure");
						yield* Effect.yieldNow;
						expect(yield* Ref.get(reads)).toBe(1);
						const success = yield* controller
							.SetAutoContinueUsageLimits(false)
							.pipe(Effect.forkChild);
						yield* Deferred.await(success_started);
						yield* Deferred.succeed(release_correction, undefined);
						yield* Deferred.succeed(release_success, undefined);
						yield* Fiber.join(success);
						yield* Effect.yieldNow;
						return { reads: yield* Ref.get(reads), state: yield* controller.Current };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.reads).toBe(2);
		expect(result.state.defaults.auto_continue_usage_limits).toBe(false);
	});

	it("returns rejected favorite receipts before one app-owned correction worker and fences it behind a later save", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const correction_started = yield* Deferred.make<void>();
					const release_correction = yield* Deferred.make<void>();
					const reads = yield* Ref.make(0);
					let writes = 0;
					const services = yield* Layer.build(
						Layer.provide(
							SessionDefaultsControllerLive,
							Layer.succeed(ArtisanClient, {
								...FixtureArtisanClientService,
								GetModelFavorites: Effect.gen(function* () {
									yield* Ref.update(reads, (count) => count + 1);
									yield* Deferred.succeed(correction_started, undefined);
									yield* Deferred.await(release_correction);
									return { model_ids: [] };
								}),
								UpdateModelFavorite: () => {
									writes += 1;
									return writes < 3
										? Effect.fail(
												new ArtisanClientError({
													cause: undefined,
													code: "connection",
													message: "offline",
													protocol_code: "offline",
													retryable: true,
												}),
											)
										: FixtureArtisanClientService.UpdateModelFavorite({
												favorite: true,
												model_id: "codex:gpt-5.6-codex",
											});
								},
							}),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						expect(
							(yield* controller
								.SetFavorite("codex:gpt-5.6-codex", true)
								.pipe(Effect.exit))._tag,
						).toBe("Failure");
						yield* Deferred.await(correction_started);
						expect(
							(yield* controller
								.SetFavorite("codex:gpt-5.6-codex", true)
								.pipe(Effect.exit))._tag,
						).toBe("Failure");
						yield* Effect.yieldNow;
						expect(yield* Ref.get(reads)).toBe(1);
						yield* controller.SetFavorite("codex:gpt-5.6-codex", true);
						yield* Deferred.succeed(release_correction, undefined);
						yield* Effect.yieldNow;
						return { reads: yield* Ref.get(reads), state: yield* controller.Current };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.reads).toBe(2);
		expect(result.state.favorite_ids).toEqual(["codex:gpt-5.6-codex"]);
	});

	it("keeps failed defaults correction admitted when the initiating caller is interrupted", async () => {
		const state = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const correction_started = yield* Deferred.make<void>();
					const release_correction = yield* Deferred.make<void>();
					const services = yield* Layer.build(
						Layer.provide(
							SessionDefaultsControllerLive,
							Layer.succeed(ArtisanClient, {
								...FixtureArtisanClientService,
								GetSessionDefaults: Effect.gen(function* () {
									yield* Deferred.succeed(correction_started, undefined);
									yield* Deferred.await(release_correction);
									return {
										agent_name_dataset: "british" as const,
										models: [],
										permission: "supervised" as const,
									};
								}),
								UpdateSessionDefaults: () =>
									Effect.fail(
										new ArtisanClientError({
											cause: undefined,
											code: "connection",
											message: "offline",
											protocol_code: "offline",
											retryable: true,
										}),
									),
							}),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						const caller = yield* controller
							.SetAgentNameDataset("british")
							.pipe(Effect.forkChild);
						yield* Deferred.await(correction_started);
						yield* Fiber.interrupt(caller);
						yield* Deferred.succeed(release_correction, undefined);
						for (let attempt = 0; attempt < 10; attempt += 1) {
							const current = yield* controller.Current;
							if (current.defaults.agent_name_dataset === "british") return current;
							yield* Effect.yieldNow;
						}
						return yield* controller.Current;
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(state.defaults.agent_name_dataset).toBe("british");
	});

	it("removes the disabled-engine field when the final engine is re-enabled", async () => {
		const state = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const services = yield* Layer.build(
						Layer.provide(
							SessionDefaultsControllerLive,
							Layer.succeed(ArtisanClient, FixtureArtisanClientService),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						yield* controller.SetEngineEnabled("codex", false);
						return yield* controller.SetEngineEnabled("codex", true);
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(state.defaults).not.toHaveProperty("disabled_engines");
	});

	it("remembers the exact catalog model with its effort and speed", async () => {
		const updates = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const captured = yield* Ref.make<ReadonlyArray<SessionDefaultsUpdateInput>>([]);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetRuntimeCatalog: Effect.succeed({
							default_model_id: "codex-sol",
							manifest: model_manifest,
							runnable_harness_ids: ["codex", "cursor"],
						}),
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
						yield* controller.Refresh;
						yield* controller.RememberPolicyDefaults({
							engine_id: "cursor",
							model: "gpt-5.6-sol",
							permission: "autonomous",
							permission_mode: "on_request",
							reasoning_effort: "xhigh",
							sandbox_mode: "workspace_write",
							service_tier: "fast",
							strict_clarification: false,
							web_search_enabled: false,
						} satisfies ThreadSessionPolicy);
					}).pipe(Effect.provide(services));
					return yield* Ref.get(captured);
				}),
			),
		);

		expect(updates).toEqual([
			{
				last_model_id: "cursor-gpt-5-6-sol",
				model: {
					model_id: "cursor-gpt-5-6-sol",
					reasoning_effort: "xhigh",
					service_tier: "fast",
				},
				permission: "autonomous",
			},
		]);
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

	it("starts in summary-title mode and patches an explicit latest-message choice", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const captured = yield* Ref.make<ReadonlyArray<SessionDefaultsUpdateInput>>([]);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						UpdateSessionDefaults: (input) =>
							Ref.update(captured, (current) => [...current, input]).pipe(
								Effect.andThen(FixtureArtisanClientService.UpdateSessionDefaults()),
							),
					});
					const services = yield* Layer.build(
						Layer.provide(SessionDefaultsControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* SessionDefaultsController;
						const initial = yield* controller.Current;
						const updated = yield* controller.SetThreadTitleMode("latest_message");
						return {
							captured: yield* Ref.get(captured),
							initial: initial.defaults.thread_title_mode,
							updated: updated.defaults.thread_title_mode,
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result).toEqual({
			captured: [{ thread_title_mode: "latest_message" }],
			initial: "summary",
			updated: "latest_message",
		});
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
