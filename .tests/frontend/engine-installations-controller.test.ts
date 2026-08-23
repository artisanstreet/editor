import { Deferred, Effect, Exit, Fiber, Layer, Ref } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";

import type { EngineInstallationReport } from "@artisan/protocol";
import { ArtisanClient } from "@artisan/transport/client";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";
import {
	EngineInstallationsController,
	EngineInstallationsControllerLive,
} from "../../modules/frontend/src/lib/settings/engine-installations-controller";
import {
	EngineUsageController,
	EngineUsageControllerLive,
} from "../../modules/frontend/src/lib/identity/engine-usage-controller";

const Report = (
	activity: EngineInstallationReport["activity"],
	engine_id = "codex",
): EngineInstallationReport => ({
	activity,
	credentials_present: false,
	display_name: "Codex",
	engine_id,
	managed: true,
});

const Snapshot = (activity: EngineInstallationReport["activity"]) => ({
	engines: [Report(activity)],
	fetched_at: "2026-08-14T00:00:00.000Z",
});

describe("engine installations controller", () => {
	it("force-refreshes shared usage after a first install becomes ready", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const usage_refreshed = yield* Deferred.make<void>();
					const usage_requests = yield* Ref.make<ReadonlyArray<boolean>>([]);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () =>
							Effect.succeed({
								engines: [
									{
										...Report("idle"),
										active_version: "1.0.0",
									},
								],
								fetched_at: "2026-08-21T10:00:00.000Z",
							}),
						GetEngineUsage: (input) =>
							Effect.gen(function* () {
								yield* Ref.update(usage_requests, (current) => [
									...current,
									input?.force === true,
								]);
								yield* Deferred.succeed(usage_refreshed, undefined);
								return {
									engines: [
										{
											authentication: "unauthenticated" as const,
											display_name: "Codex",
											engine_id: "codex",
											windows: [],
										},
									],
									fetched_at: "2026-08-21T10:00:01.000Z",
								};
							}),
						InstallEngine: () =>
							Effect.succeed({
								report: { ...Report("installing"), managed: false },
								status: "accepted" as const,
							}),
					});
					const services = yield* Layer.build(
						EngineInstallationsControllerLive.pipe(
							Layer.provideMerge(EngineUsageControllerLive),
							Layer.provide(client_layer),
						),
					);

					return yield* Effect.gen(function* () {
						const installations = yield* EngineInstallationsController;
						const usage = yield* EngineUsageController;
						yield* installations.Install("codex");
						yield* Deferred.await(usage_refreshed);
						return {
							entry: yield* usage.Load("codex"),
							requests: yield* Ref.get(usage_requests),
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.requests).toEqual([true]);
		expect(result.entry?.report?.authentication).toBe("unauthenticated");
	});

	it("shares an interrupted route refresh with its surviving caller", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const reads = yield* Ref.make(0);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () =>
							Effect.gen(function* () {
								yield* Ref.update(reads, (count) => count + 1);
								yield* Deferred.succeed(started, undefined);
								yield* Deferred.await(release);
								return Snapshot("idle");
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(EngineInstallationsControllerLive, client_layer),
					);

					return yield* Effect.gen(function* () {
						const controller = yield* EngineInstallationsController;
						const route_mount = yield* controller
							.Refresh({ engine_id: "codex" })
							.pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						const action = yield* controller
							.Refresh({ engine_id: "codex" })
							.pipe(Effect.forkScoped);
						yield* Effect.yieldNow;
						expect(yield* Ref.get(reads)).toBe(1);
						yield* Fiber.interrupt(route_mount);
						yield* Deferred.succeed(release, undefined);
						const state = yield* Fiber.join(action);
						return { reads: yield* Ref.get(reads), state };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.reads).toBe(1);
		expect(result.state.reports.codex?.activity).toBe("idle");
	});

	it("keeps distinct refresh intents separate while coalescing each intent", async () => {
		const reads = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const count = yield* Ref.make(0);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () =>
							Ref.updateAndGet(count, (current) => current + 1).pipe(
								Effect.as(Snapshot("idle")),
							),
					});
					const services = yield* Layer.build(
						Layer.provide(EngineInstallationsControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* EngineInstallationsController;
						yield* Effect.all(
							[
								controller.Refresh({ engine_id: "codex" }),
								controller.Refresh({ engine_id: "codex" }),
								controller.Refresh({ check_updates: true, engine_id: "codex" }),
							],
							{ concurrency: "unbounded", discard: true },
						);
						return yield* Ref.get(count);
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(reads).toBe(2);
	});

	it("settles an accepted action before its monitor poll replies", async () => {
		const state = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const poll_started = yield* Deferred.make<void>();
					const release_poll = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () =>
							Effect.gen(function* () {
								yield* Deferred.succeed(poll_started, undefined);
								yield* Deferred.await(release_poll);
								return Snapshot("idle");
							}),
						InstallEngine: () =>
							Effect.succeed({
								report: Report("installing"),
								status: "accepted" as const,
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(EngineInstallationsControllerLive, client_layer),
					);

					return yield* Effect.gen(function* () {
						const controller = yield* EngineInstallationsController;
						const install = yield* controller.Install("codex").pipe(Effect.forkChild);
						yield* Effect.yieldNow;
						yield* Deferred.await(poll_started);
						expect((yield* controller.Current).pending_engine_ids.has("codex")).toBe(
							true,
						);
						yield* Fiber.join(install);
						yield* Deferred.succeed(release_poll, undefined);
						return yield* controller.Current;
					}).pipe(Effect.provide(services));
				}),
			).pipe(Effect.provide(TestClock.layer())),
		);

		expect(state.pending_engine_ids.has("codex")).toBe(true);
		expect(state.reports.codex?.activity).toBe("installing");
	});

	it("keeps one replacement-safe monitor per engine", async () => {
		const maximum_active_polls = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const active = yield* Ref.make(0);
					const maximum = yield* Ref.make(0);
					const first_poll = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () =>
							Effect.gen(function* () {
								const count = yield* Ref.updateAndGet(active, (value) => value + 1);
								yield* Ref.update(maximum, (value) => Math.max(value, count));
								yield* Deferred.succeed(first_poll, undefined);
								return Snapshot("installing");
							}).pipe(Effect.ensuring(Ref.update(active, (value) => value - 1))),
						InstallEngine: () =>
							Effect.succeed({
								report: Report("installing"),
								status: "accepted" as const,
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(EngineInstallationsControllerLive, client_layer),
					);

					return yield* Effect.gen(function* () {
						const controller = yield* EngineInstallationsController;
						yield* controller.Install("codex");
						yield* Deferred.await(first_poll);
						yield* controller.Install("codex");
						yield* Effect.yieldNow;
						return yield* Ref.get(maximum);
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(maximum_active_polls).toBe(1);
	});

	it("does not let a superseded monitor overwrite an accepted terminal command", async () => {
		const state = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const monitor_started = yield* Deferred.make<void>();
					const release_monitor = yield* Deferred.make<void>();
					const commands = yield* Ref.make(0);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () =>
							Effect.gen(function* () {
								yield* Deferred.succeed(monitor_started, undefined);
								yield* Deferred.await(release_monitor);
								return Snapshot("installing");
							}),
						InstallEngine: () =>
							Ref.updateAndGet(commands, (count) => count + 1).pipe(
								Effect.map((count) => ({
									report: Report(count === 1 ? "installing" : "idle"),
									status: "accepted" as const,
								})),
							),
					});
					const services = yield* Layer.build(
						Layer.provide(EngineInstallationsControllerLive, client_layer),
					);

					return yield* Effect.gen(function* () {
						const controller = yield* EngineInstallationsController;
						yield* controller.Install("codex");
						yield* Deferred.await(monitor_started);
						yield* controller.Install("codex");
						yield* Deferred.succeed(release_monitor, undefined);
						yield* Effect.yieldNow;
						return yield* controller.Current;
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(state.reports.codex?.activity).toBe("idle");
		expect(state.pending_engine_ids.has("codex")).toBe(false);
	});

	it("does not let a pre-command refresh overwrite its accepted receipt", async () => {
		const state = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const refresh_started = yield* Deferred.make<void>();
					const release_refresh = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () =>
							Effect.gen(function* () {
								yield* Deferred.succeed(refresh_started, undefined);
								yield* Deferred.await(release_refresh);
								return Snapshot("installing");
							}),
						InstallEngine: () =>
							Effect.succeed({
								report: Report("idle"),
								status: "accepted" as const,
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(EngineInstallationsControllerLive, client_layer),
					);

					return yield* Effect.gen(function* () {
						const controller = yield* EngineInstallationsController;
						const refresh = yield* controller.Refresh().pipe(Effect.forkChild);
						yield* Deferred.await(refresh_started);
						yield* controller.Install("codex");
						yield* Deferred.succeed(release_refresh, undefined);
						yield* Fiber.join(refresh);
						return yield* controller.Current;
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(state.reports.codex?.activity).toBe("idle");
	});

	it("does not let one engine command block another engine", async () => {
		const calls = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const codex_started = yield* Deferred.make<void>();
					const release_codex = yield* Deferred.make<void>();
					const command_calls = yield* Ref.make<ReadonlyArray<string>>([]);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () => Effect.succeed(Snapshot("idle")),
						InstallEngine: ({ engine_id }) =>
							Effect.gen(function* () {
								yield* Ref.update(command_calls, (current) => [
									...current,
									engine_id,
								]);
								if (engine_id === "codex") {
									yield* Deferred.succeed(codex_started, undefined);
									yield* Deferred.await(release_codex);
								}
								return {
									report: Report("idle", engine_id),
									status: "accepted" as const,
								};
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(EngineInstallationsControllerLive, client_layer),
					);

					return yield* Effect.gen(function* () {
						const controller = yield* EngineInstallationsController;
						const codex = yield* controller.Install("codex").pipe(Effect.forkChild);
						yield* Deferred.await(codex_started);
						yield* controller.Install("claude");
						yield* Deferred.succeed(release_codex, undefined);
						yield* Fiber.join(codex);
						return yield* Ref.get(command_calls);
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(calls).toEqual(["codex", "claude"]);
	});

	it("serializes mutations and records a rejected request truthfully", async () => {
		const outcome = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const calls = yield* Ref.make(0);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () => Effect.succeed(Snapshot("idle")),
						InstallEngine: () =>
							Effect.gen(function* () {
								const call = yield* Ref.updateAndGet(calls, (count) => count + 1);
								if (call === 1) {
									yield* Deferred.succeed(started, undefined);
									yield* Deferred.await(release);
									return {
										report: Report("installing"),
										status: "accepted" as const,
									};
								}
								return {
									message: "Release channel unavailable",
									status: "rejected" as const,
								};
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(EngineInstallationsControllerLive, client_layer),
					);

					return yield* Effect.gen(function* () {
						const controller = yield* EngineInstallationsController;
						const first = yield* controller.Install("codex").pipe(Effect.forkChild);
						yield* Deferred.await(started);
						const second = yield* controller.Install("codex").pipe(Effect.forkChild);
						expect(yield* Ref.get(calls)).toBe(1);
						yield* Deferred.succeed(release, undefined);
						yield* Fiber.join(first);
						const rejected = yield* Fiber.await(second);
						return { rejected, state: yield* controller.Current };
					}).pipe(Effect.provide(services));
				}),
			).pipe(Effect.provide(TestClock.layer())),
		);

		expect(Exit.isFailure(outcome.rejected)).toBe(true);
		expect(outcome.state.errors.codex).toBe("Release channel unavailable");
		expect(outcome.state.pending_engine_ids.has("codex")).toBe(false);
	});

	it("bounds a stalled install and clears its local pending marker", async () => {
		const state = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const poll_started = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () =>
							Effect.gen(function* () {
								yield* Deferred.succeed(poll_started, undefined);
								return Snapshot("installing");
							}),
						InstallEngine: () =>
							Effect.succeed({
								report: Report("installing"),
								status: "accepted" as const,
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(EngineInstallationsControllerLive, client_layer),
					);

					return yield* Effect.gen(function* () {
						const controller = yield* EngineInstallationsController;
						const install = yield* controller
							.Install("codex")
							.pipe(Effect.exit, Effect.forkChild);
						yield* Deferred.await(poll_started);
						yield* Fiber.join(install);
						yield* Effect.yieldNow;
						yield* TestClock.adjust("90 seconds");
						yield* Effect.yieldNow;
						return yield* controller.Current;
					}).pipe(Effect.provide(services));
				}),
			).pipe(Effect.provide(TestClock.layer())),
		);

		expect(state.pending_engine_ids.has("codex")).toBe(false);
		expect(state.errors.codex).toContain("still running");
	});

	it("clears a timed-out local error when a later authoritative idle report arrives", async () => {
		const state = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const terminal = yield* Ref.make(false);
					const poll_started = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () =>
							Effect.gen(function* () {
								yield* Deferred.succeed(poll_started, undefined);
								return Snapshot((yield* Ref.get(terminal)) ? "idle" : "installing");
							}),
						InstallEngine: () =>
							Effect.succeed({
								report: Report("installing"),
								status: "accepted" as const,
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(EngineInstallationsControllerLive, client_layer),
					);

					return yield* Effect.gen(function* () {
						const controller = yield* EngineInstallationsController;
						const install = yield* controller
							.Install("codex")
							.pipe(Effect.exit, Effect.forkChild);
						yield* Deferred.await(poll_started);
						yield* Fiber.join(install);
						yield* Effect.yieldNow;
						yield* TestClock.adjust("90 seconds");
						yield* Effect.yieldNow;
						expect((yield* controller.Current).errors.codex).toContain("still running");
						yield* Ref.set(terminal, true);
						return yield* controller.Refresh({ engine_id: "codex" });
					}).pipe(Effect.provide(services));
				}),
			).pipe(Effect.provide(TestClock.layer())),
		);

		expect(state.reports.codex?.activity).toBe("idle");
		expect(state.errors.codex).toBeUndefined();
	});

	it("settles authentication and rollback against a failed terminal report", async () => {
		const state = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetEngineInstallations: () =>
							Effect.gen(function* () {
								const read = yield* Ref.updateAndGet(reads, (count) => count + 1);
								return read % 2 === 1
									? Snapshot("authenticating")
									: {
											...Snapshot("failed"),
											engines: [
												{ ...Report("failed"), failure: "Sign-in closed" },
											],
										};
							}),
						AuthenticateEngine: () =>
							Effect.succeed({
								report: Report("authenticating"),
								status: "accepted" as const,
							}),
						RollbackEngine: () =>
							Effect.succeed({
								report: Report("authenticating"),
								status: "accepted" as const,
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(EngineInstallationsControllerLive, client_layer),
					);

					return yield* Effect.gen(function* () {
						const controller = yield* EngineInstallationsController;
						const authenticate = yield* controller
							.Authenticate("codex")
							.pipe(Effect.forkChild);
						yield* Fiber.join(authenticate);
						yield* Effect.yieldNow;
						yield* Effect.yieldNow;
						yield* TestClock.adjust("750 millis");
						const rollback = yield* controller.Rollback("codex").pipe(Effect.forkChild);
						yield* Fiber.join(rollback);
						yield* Effect.yieldNow;
						yield* Effect.yieldNow;
						yield* TestClock.adjust("750 millis");
						return yield* controller.Current;
					}).pipe(Effect.provide(services));
				}),
			).pipe(Effect.provide(TestClock.layer())),
		);

		expect(state.pending_engine_ids.has("codex")).toBe(false);
		expect(state.reports.codex?.activity).toBe("failed");
		expect(state.errors.codex).toBe("Sign-in closed");
	});
});
