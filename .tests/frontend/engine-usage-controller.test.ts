import { Deferred, Effect, Fiber, Layer } from "effect";
import { describe, expect, it } from "vitest";

import type { EngineUsageSnapshot } from "@artisan/protocol";
import { ArtisanClient, ArtisanClientError } from "@artisan/transport/client";
import {
	EngineUsageController,
	EngineUsageControllerLive,
} from "../../modules/frontend/src/lib/identity/engine-usage-controller";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";

const Snapshot = (
	fetched_at = new Date().toISOString(),
	display_name = "Codex",
): EngineUsageSnapshot => ({
	engines: [
		{
			authentication: "authenticated",
			display_name,
			engine_id: "codex",
			windows: [],
		},
	],
	fetched_at,
});

describe("engine usage controller", () => {
	it("shares cold callers, retains the admitted request, and bypasses fresh data for force", async () => {
		const calls: Array<boolean> = [];
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const services = yield* Layer.build(
						EngineUsageControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									GetEngineUsage: (input) =>
										Effect.gen(function* () {
											calls.push(input?.force === true);
											if (calls.length === 1) {
												yield* Deferred.succeed(started, undefined);
												yield* Deferred.await(release);
											}
											return Snapshot();
										}),
								}),
							),
						),
					);
					yield* Effect.gen(function* () {
						const controller = yield* EngineUsageController;
						const starter = yield* controller.Load("codex").pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						expect(calls).toEqual([false]);
						yield* Fiber.interrupt(starter);
						const follower = yield* controller.Load("codex").pipe(Effect.forkScoped);
						yield* Deferred.succeed(release, undefined);
						yield* Fiber.join(follower);
						yield* controller.Load("codex");
						yield* controller.Load("codex", { force: true });
						expect(calls).toEqual([false, true]);
					}).pipe(Effect.provide(services));
				}),
			),
		);
	});

	it("uses a fresh seeded sidebar report and evicts failed flights for a retry", async () => {
		const calls: Array<number> = [];
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const services = yield* Layer.build(
						EngineUsageControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									GetEngineUsage: () =>
										Effect.gen(function* () {
											calls.push(calls.length);
											if (calls.length === 2) {
												return yield* Effect.fail(
													new ArtisanClientError({
														cause: undefined,
														code: "connection",
														message: "offline",
														protocol_code: "offline",
														retryable: true,
													}),
												);
											}
											return Snapshot();
										}),
								}),
							),
						),
					);
					yield* Effect.gen(function* () {
						const controller = yield* EngineUsageController;
						yield* controller.Seed(Snapshot());
						yield* controller.Load("codex");
						expect(calls).toEqual([]);

						yield* controller.Load("codex", { force: true });
						expect(calls).toEqual([0]);
						const failed = yield* controller
							.Load("codex", { force: true })
							.pipe(Effect.exit);
						expect(failed._tag).toBe("Failure");
						yield* controller.Load("codex", { force: true });
						expect(calls).toEqual([0, 1, 2]);
					}).pipe(Effect.provide(services));
				}),
			),
		);
	});

	it("admits a requested forced refresh after a joined automatic flight fails", async () => {
		const calls: Array<boolean> = [];
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const services = yield* Layer.build(
						EngineUsageControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									GetEngineUsage: (input) =>
										Effect.gen(function* () {
											calls.push(input?.force === true);
											if (calls.length === 1) {
												yield* Deferred.succeed(started, undefined);
												yield* Deferred.await(release);
												return yield* Effect.fail(
													new ArtisanClientError({
														cause: undefined,
														code: "connection",
														message: "automatic refresh failed",
														protocol_code: "offline",
														retryable: true,
													}),
												);
											}
											return Snapshot();
										}),
								}),
							),
						),
					);
					yield* Effect.gen(function* () {
						const controller = yield* EngineUsageController;
						const automatic = yield* controller.Load("codex").pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						const forced = yield* controller
							.Load("codex", { force: true })
							.pipe(Effect.forkScoped);
						yield* Deferred.succeed(release, undefined);
						const automatic_exit = yield* Fiber.await(automatic);
						const forced_entry = yield* Fiber.join(forced);
						expect(automatic_exit._tag).toBe("Failure");
						expect(forced_entry.report?.engine_id).toBe("codex");
						expect(calls).toEqual([false, true]);
					}).pipe(Effect.provide(services));
				}),
			),
		);
	});

	it("does not let an older admitted response replace a newer seeded snapshot", async () => {
		const older_at = "2026-08-15T08:00:00.000Z";
		const newer_at = "2026-08-15T09:00:00.000Z";
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const services = yield* Layer.build(
						EngineUsageControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									GetEngineUsage: () =>
										Effect.gen(function* () {
											yield* Deferred.succeed(started, undefined);
											yield* Deferred.await(release);
											return Snapshot(older_at, "Older Codex");
										}),
								}),
							),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* EngineUsageController;
						const load = yield* controller.Load("codex").pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						yield* controller.Seed(Snapshot(newer_at, "Newer Codex"));
						yield* Deferred.succeed(release, undefined);
						const loaded = yield* Fiber.join(load);
						const current = yield* controller.Current;
						return { current: current.entries.get("codex"), loaded };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.loaded.report?.display_name).toBe("Newer Codex");
		expect(result.current?.report?.display_name).toBe("Newer Codex");
	});

	it("wires both usage surfaces through the application-scoped controller", async () => {
		const { readFile } = await import("node:fs/promises");
		const [sidebar, settings] = await Promise.all([
			readFile("modules/frontend/src/routes/components/sidebar-identity.svelte", "utf8"),
			readFile("modules/frontend/src/routes/components/settings/engine.svelte", "utf8"),
		]);

		expect(sidebar).toContain("EngineUsageController");
		expect(sidebar).toContain("const entries = [...next.entries.values()]");
		expect(sidebar).not.toContain("const MergeReports");
		expect(settings).toContain("EngineUsageController");
		expect(sidebar).not.toContain("client.GetEngineUsage");
		expect(settings).not.toContain("client.GetEngineUsage");
	});
});
