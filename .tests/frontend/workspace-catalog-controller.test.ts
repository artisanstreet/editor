import { readFileSync } from "node:fs";

import { Deferred, Effect, Fiber, Layer, Ref } from "effect";
import { describe, expect, it } from "vitest";

import { ArtisanClient } from "@artisan/transport/client";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";
import {
	fixture_artisan_client_data,
	fixture_project,
} from "../../modules/frontend/src/lib/runtime/fixtures/data";
import {
	WorkspaceCatalogController,
	WorkspaceCatalogControllerLive,
} from "../../modules/frontend/src/lib/root/workspace-catalog-controller";

const WithController = <A, E, R>(effect: Effect.Effect<A, E, R>) =>
	Effect.scoped(
		Effect.gen(function* () {
			const services = yield* Layer.build(
				Layer.provide(
					WorkspaceCatalogControllerLive,
					Layer.succeed(ArtisanClient, FixtureArtisanClientService),
				),
			);
			return yield* effect.pipe(Effect.provide(services));
		}),
	);

it("atomically admits catalog refresh flights while leaving only deferred waits interruptible", () => {
	const source = readFileSync(
		"modules/frontend/src/lib/root/workspace-catalog-controller.ts",
		"utf8",
	);
	expect(source).toContain("Effect.uninterruptibleMask((restore)");
	expect(source).toContain("Complete(kind, claimed, load(claimed.revision))");
	expect(source).toContain("restore(Deferred.await(claimed.deferred))");
	expect(source).toContain("restore(Deferred.await(deferred))");
});

describe("workspace catalog controller", () => {
	it("begins cold without pretending either catalog is loaded", async () => {
		const current = await Effect.runPromise(
			WithController(
				Effect.gen(function* () {
					return yield* (yield* WorkspaceCatalogController).Current;
				}),
			),
		);

		expect(current).toEqual({
			projects: [],
			projects_loaded: false,
			threads: [],
			threads_loaded: false,
		});
	});

	it("reads projects and threads concurrently, then publishes one complete snapshot", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const projects_started = yield* Deferred.make<void>();
					const threads_started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						ListProjects: Effect.gen(function* () {
							yield* Deferred.succeed(projects_started, undefined);
							yield* Deferred.await(release);
							return { projects: [fixture_project] };
						}),
						ListThreads: Effect.gen(function* () {
							yield* Deferred.succeed(threads_started, undefined);
							yield* Deferred.await(release);
							return fixture_artisan_client_data.threads;
						}),
					});
					const services = yield* Layer.build(
						Layer.provide(WorkspaceCatalogControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* WorkspaceCatalogController;
						const refresh = yield* controller.Refresh.pipe(Effect.forkScoped);
						yield* Deferred.await(projects_started);
						yield* Deferred.await(threads_started);
						expect(yield* controller.Current).toEqual({
							projects: [],
							projects_loaded: false,
							threads: [],
							threads_loaded: false,
						});
						yield* Deferred.succeed(release, undefined);
						const snapshot = yield* Fiber.join(refresh);
						return { snapshot };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.snapshot.projects).toEqual([fixture_project]);
		expect(result.snapshot.threads).toEqual(fixture_artisan_client_data.threads);
		expect(result.snapshot.projects_loaded).toBe(true);
		expect(result.snapshot.threads_loaded).toBe(true);
	});

	it("applies authoritative thread updates without rereading projects", async () => {
		const state = await Effect.runPromise(
			WithController(
				Effect.gen(function* () {
					const controller = yield* WorkspaceCatalogController;
					yield* controller.RefreshProjects;
					return yield* controller.ApplyThreadListUpdate({
						journal_sequence: 1,
						thread: fixture_artisan_client_data.threads[0]!,
						type: "upsert",
					});
				}),
			),
		);

		expect(state.projects).toEqual([fixture_project]);
		expect(state.projects_loaded).toBe(true);
		expect(state.threads_loaded).toBe(true);
		expect(state.threads).toEqual(fixture_artisan_client_data.threads);
	});

	it("coalesces concurrent snapshot refresh callers into one project and thread read", async () => {
		const counts = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const project_reads = yield* Ref.make(0);
					const thread_reads = yield* Ref.make(0);
					const projects_started = yield* Deferred.make<void>();
					const threads_started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						ListProjects: Effect.gen(function* () {
							yield* Ref.update(project_reads, (count) => count + 1);
							yield* Deferred.succeed(projects_started, undefined);
							yield* Deferred.await(release);
							return { projects: [fixture_project] };
						}),
						ListThreads: Effect.gen(function* () {
							yield* Ref.update(thread_reads, (count) => count + 1);
							yield* Deferred.succeed(threads_started, undefined);
							yield* Deferred.await(release);
							return fixture_artisan_client_data.threads;
						}),
					});
					const services = yield* Layer.build(
						Layer.provide(WorkspaceCatalogControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* WorkspaceCatalogController;
						const first = yield* controller.Refresh.pipe(Effect.forkScoped);
						const second = yield* controller.Refresh.pipe(Effect.forkScoped);
						yield* Deferred.await(projects_started);
						yield* Deferred.await(threads_started);
						yield* Deferred.succeed(release, undefined);
						yield* Fiber.join(first);
						yield* Fiber.join(second);
						return {
							projects: yield* Ref.get(project_reads),
							threads: yield* Ref.get(thread_reads),
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(counts).toEqual({ projects: 1, threads: 1 });
	});

	it("keeps an admitted snapshot refresh alive when its starter is interrupted", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const project_reads = yield* Ref.make(0);
					const thread_reads = yield* Ref.make(0);
					const projects_started = yield* Deferred.make<void>();
					const threads_started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						ListProjects: Effect.gen(function* () {
							yield* Ref.update(project_reads, (count) => count + 1);
							yield* Deferred.succeed(projects_started, undefined);
							yield* Deferred.await(release);
							return { projects: [fixture_project] };
						}),
						ListThreads: Effect.gen(function* () {
							yield* Ref.update(thread_reads, (count) => count + 1);
							yield* Deferred.succeed(threads_started, undefined);
							yield* Deferred.await(release);
							return fixture_artisan_client_data.threads;
						}),
					});
					const services = yield* Layer.build(
						Layer.provide(WorkspaceCatalogControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* WorkspaceCatalogController;
						const starter = yield* controller.Refresh.pipe(Effect.forkScoped);
						yield* Effect.all(
							[Deferred.await(projects_started), Deferred.await(threads_started)],
							{ concurrency: "unbounded", discard: true },
						);
						const follower = yield* controller.Refresh.pipe(Effect.forkScoped);
						yield* Effect.yieldNow;
						yield* Fiber.interrupt(starter);
						yield* Deferred.succeed(release, undefined);
						const snapshot = yield* Fiber.join(follower);
						return {
							project_reads: yield* Ref.get(project_reads),
							snapshot,
							thread_reads: yield* Ref.get(thread_reads),
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.project_reads).toBe(1);
		expect(result.thread_reads).toBe(1);
		expect(result.snapshot).toMatchObject({
			projects: [fixture_project],
			projects_loaded: true,
			threads: fixture_artisan_client_data.threads,
			threads_loaded: true,
		});
	});

	it("refreshes either half without rereading the other", async () => {
		const counts = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const project_reads = yield* Ref.make(0);
					const thread_reads = yield* Ref.make(0);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						ListProjects: Effect.gen(function* () {
							yield* Ref.update(project_reads, (count) => count + 1);
							return { projects: [fixture_project] };
						}),
						ListThreads: Effect.gen(function* () {
							yield* Ref.update(thread_reads, (count) => count + 1);
							return fixture_artisan_client_data.threads;
						}),
					});
					const services = yield* Layer.build(
						Layer.provide(WorkspaceCatalogControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* WorkspaceCatalogController;
						yield* controller.RefreshProjects;
						yield* controller.RefreshThreads;
						return {
							projects: yield* Ref.get(project_reads),
							threads: yield* Ref.get(thread_reads),
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(counts).toEqual({ projects: 1, threads: 1 });
	});

	it("does not let a delayed refresh overwrite newer authoritative catalog updates", async () => {
		const state = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const release = yield* Deferred.make<void>();
					const projects_started = yield* Deferred.make<void>();
					const threads_started = yield* Deferred.make<void>();
					const updated_project = { ...fixture_project, display_name: "Updated project" };
					const updated_thread = {
						...fixture_artisan_client_data.threads[0]!,
						title: "Updated thread",
					};
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						ListProjects: Effect.gen(function* () {
							yield* Deferred.succeed(projects_started, undefined);
							yield* Deferred.await(release);
							return { projects: [fixture_project] };
						}),
						ListThreads: Effect.gen(function* () {
							yield* Deferred.succeed(threads_started, undefined);
							yield* Deferred.await(release);
							return fixture_artisan_client_data.threads;
						}),
					});
					const services = yield* Layer.build(
						Layer.provide(WorkspaceCatalogControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* WorkspaceCatalogController;
						const refresh = yield* controller.Refresh.pipe(Effect.forkScoped);
						yield* Deferred.await(projects_started);
						yield* Deferred.await(threads_started);
						yield* controller.ApplyProjectCatalogUpdate({
							snapshot: { projects: [updated_project] },
							type: "replacement",
						});
						yield* controller.ApplyThreadListUpdate({
							journal_sequence: 2,
							thread: updated_thread,
							type: "upsert",
						});
						yield* Deferred.succeed(release, undefined);
						yield* Fiber.join(refresh);
						return yield* controller.Current;
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(state.projects).toEqual([{ ...fixture_project, display_name: "Updated project" }]);
		expect(state.threads).toEqual([expect.objectContaining({ title: "Updated thread" })]);
	});
});
