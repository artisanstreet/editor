import { Deferred, Effect, Exit, Fiber, Layer, Ref } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";

import { ArtisanClient, ArtisanClientError } from "@artisan/transport/client";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";
import {
	ProjectRepositoryController,
	ProjectRepositoryControllerLive,
} from "../../modules/frontend/src/lib/workspace/project-repository-controller";

const RepositoryResultFor = (project_id: string | undefined) =>
	FixtureArtisanClientService.GetProjectRepositories().pipe(
		Effect.map((result) => ({
			repositories: result.repositories.map((entry) => ({
				...entry,
				project_id: project_id ?? entry.project_id,
			})),
		})),
	);

const ColdConnectionFailure = new ArtisanClientError({
	cause: undefined,
	code: "connection",
	message: "Forge is not ready yet.",
	protocol_code: "forge_not_ready",
	retryable: true,
});

describe("project repository controller", () => {
	it("coalesces simultaneous consumers of one project into one repository RPC", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetProjectRepositories: (project_ids) =>
							Effect.gen(function* () {
								yield* Ref.update(reads, (current) => current + 1);
								yield* Deferred.succeed(started, undefined);
								yield* Deferred.await(release);
								return yield* RepositoryResultFor(project_ids?.[0]);
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(ProjectRepositoryControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* ProjectRepositoryController;
						const consumers = yield* Effect.all(
							[controller.Load("project_a"), controller.Load("project_a")],
							{ concurrency: "unbounded" },
						).pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						expect(yield* Ref.get(reads)).toBe(1);
						yield* Deferred.succeed(release, undefined);
						const repositories = yield* Fiber.join(consumers);
						return { reads: yield* Ref.get(reads), repositories };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.reads).toBe(1);
		expect(result.repositories[0]).toEqual(result.repositories[1]);
	});

	it("retries one failed cold attempt without splitting concurrent consumers", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const first_attempt = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetProjectRepositories: (project_ids) =>
							Effect.gen(function* () {
								const attempt = yield* Ref.updateAndGet(
									reads,
									(current) => current + 1,
								);
								if (attempt === 1) {
									yield* Deferred.succeed(first_attempt, undefined);
									return yield* Effect.fail(ColdConnectionFailure);
								}
								return yield* RepositoryResultFor(project_ids?.[0]);
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(ProjectRepositoryControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* ProjectRepositoryController;
						const consumers = yield* Effect.all(
							[controller.Load("project_a"), controller.Load("project_a")],
							{ concurrency: "unbounded" },
						).pipe(Effect.forkScoped);
						yield* Deferred.await(first_attempt);
						expect(yield* Ref.get(reads)).toBe(1);
						yield* TestClock.adjust("100 millis");
						const repositories = yield* Fiber.join(consumers);
						return {
							reads: yield* Ref.get(reads),
							repositories,
							retained: (yield* controller.Current).get("project_a"),
						};
					}).pipe(Effect.provide(services));
				}),
			).pipe(Effect.provide(TestClock.layer())),
		);

		expect(result.reads).toBe(2);
		expect(result.repositories[0]).toEqual(result.repositories[1]);
		expect(result.retained).toEqual(result.repositories[0]);
	});

	it("expires an exhausted cold failure so a later ready Forge can publish", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const should_fail = yield* Ref.make(true);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetProjectRepositories: (project_ids) =>
							Effect.gen(function* () {
								yield* Ref.update(reads, (current) => current + 1);
								if (yield* Ref.get(should_fail)) {
									return yield* Effect.fail(ColdConnectionFailure);
								}
								return yield* RepositoryResultFor(project_ids?.[0]);
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(ProjectRepositoryControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* ProjectRepositoryController;
						const failed = yield* controller.Load("project_a").pipe(Effect.forkScoped);
						yield* TestClock.adjust("10 seconds");
						const failed_exit = yield* Fiber.await(failed);
						const failed_reads = yield* Ref.get(reads);
						yield* Ref.set(should_fail, false);
						const repository = yield* controller.Load("project_a");
						return {
							failed_exit,
							failed_reads,
							reads: yield* Ref.get(reads),
							repository,
							retained: (yield* controller.Current).get("project_a"),
						};
					}).pipe(Effect.provide(services));
				}),
			).pipe(Effect.provide(TestClock.layer())),
		);

		expect(Exit.isFailure(result.failed_exit)).toBe(true);
		expect(result.reads).toBe(result.failed_reads + 1);
		expect(result.retained).toEqual(result.repository);
	});

	it("keeps a starter's cold lookup alive after that consumer is interrupted", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const reads = yield* Ref.make(0);
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetProjectRepositories: (project_ids) =>
							Effect.gen(function* () {
								yield* Ref.update(reads, (current) => current + 1);
								yield* Deferred.succeed(started, undefined);
								yield* Deferred.await(release);
								return yield* RepositoryResultFor(project_ids?.[0]);
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(ProjectRepositoryControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* ProjectRepositoryController;
						const starter = yield* controller.Load("project_a").pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						yield* Fiber.interrupt(starter);
						const follower = yield* controller
							.Load("project_a")
							.pipe(Effect.forkScoped);
						yield* Deferred.succeed(release, undefined);
						const repository = yield* Fiber.join(follower);
						return {
							reads: yield* Ref.get(reads),
							repository,
							retained: (yield* controller.Current).get("project_a"),
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.reads).toBe(1);
		expect(result.retained).toEqual(result.repository);
	});

	it("uses the warm key without another RPC and keeps projects independently keyed", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const requests = yield* Ref.make<ReadonlyArray<string>>([]);
					const client_layer = Layer.succeed(ArtisanClient, {
						...FixtureArtisanClientService,
						GetProjectRepositories: (project_ids) =>
							Effect.gen(function* () {
								yield* Ref.update(requests, (current) => [
									...current,
									project_ids?.[0] ?? "all-projects",
								]);
								return yield* RepositoryResultFor(project_ids?.[0]);
							}),
					});
					const services = yield* Layer.build(
						Layer.provide(ProjectRepositoryControllerLive, client_layer),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* ProjectRepositoryController;
						yield* controller.Load("project_a");
						yield* controller.Load("project_a");
						yield* controller.Load("project_b");
						return {
							requests: yield* Ref.get(requests),
							state: yield* controller.Current,
						};
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result.requests).toEqual(["project_a", "project_b"]);
		expect([...result.state.keys()]).toEqual(["project_a", "project_b"]);
	});
});
