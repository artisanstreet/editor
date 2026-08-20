import { mkdtemp, readFile, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";

import {
	Cause,
	Context,
	Deferred,
	Effect,
	Exit,
	Fiber,
	Layer,
	ManagedRuntime,
	Result,
	Scope,
} from "effect";
import { afterEach, describe, expect, it } from "vitest";
import { MakeSnowflakeIdLive } from "@artisan/protocol";

import {
	GuidanceFileStore,
	GuidanceFileStoreLive,
} from "../../modules/backend/src/guidance/file-store";
import { GuidanceProviderRegistry } from "../../modules/backend/src/guidance/provider-mirrors";
import {
	GlobalGuidanceRepository,
	GlobalGuidanceRepositoryLive,
} from "../../modules/backend/src/guidance/repository";
import {
	GlobalGuidanceInvariantError,
	GlobalGuidanceService,
	make_global_guidance_service_layer,
} from "../../modules/backend/src/guidance/service";
import { make_database_layer } from "../../modules/backend/src/persistence/database";
import { JournalNotifierLive } from "../../modules/backend/src/persistence/journal-notifier";
import { JournalStoreLive } from "../../modules/backend/src/persistence/journal-store";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

interface StartupFixtureOptions {
	readonly fail_first?: boolean;
	readonly hold_claim?: boolean;
	readonly hold_publishing?: boolean;
	readonly hold_refresh?: boolean;
	readonly hold_first_retry_observation?: boolean;
}

const make_runtime = async (options: StartupFixtureOptions = {}) => {
	const root = await mkdtemp(join(tmpdir(), "artisan-guidance-startup-"));
	temporary_directories.push(root);
	const entered = Effect.runSync(Deferred.make<void>());
	const release = Effect.runSync(Deferred.make<void>());
	const refresh_entered = Effect.runSync(Deferred.make<void>());
	const release_refresh = Effect.runSync(Deferred.make<void>());
	const claim_entered = Effect.runSync(Deferred.make<void>());
	const release_claim = Effect.runSync(Deferred.make<void>());
	const publishing_entered = Effect.runSync(Deferred.make<void>());
	const release_publishing = Effect.runSync(Deferred.make<void>());
	const retry_observed = Effect.runSync(Deferred.make<void>());
	const release_retry_observation = Effect.runSync(Deferred.make<void>());
	let discoveries = 0;
	let flights = 0;
	let next_id = 0;
	let file_reads = 0;
	let repository_reads = 0;
	let retry_observations = 0;
	const metadata = Layer.succeed(RuntimeMetadata, {
		instance_id: "guidance_startup_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed("2026-08-15T12:00:00.000Z"),
	});
	const registry = Layer.succeed(GuidanceProviderRegistry, {
		Providers: [
			{
				Discover: Effect.sync(() => {
					discoveries += 1;
					return {
						_tag: "Absent" as const,
						path: join(root, "provider", "AGENTS.md"),
					};
				}),
				mode: "native_file" as const,
				provider: "codex" as const,
			},
			{ mode: "runtime" as const, provider: "claude" as const },
		],
	});
	const infrastructure = Layer.mergeAll(
		make_database_layer({ database_path: join(root, "artisan.db"), migrations_path }),
		metadata,
		JournalNotifierLive,
	);
	const repository = GlobalGuidanceRepositoryLive.pipe(
		Layer.provideMerge(JournalStoreLive.pipe(Layer.provideMerge(infrastructure))),
		Layer.provideMerge(infrastructure),
	);
	const counted_repository = Layer.effect(
		GlobalGuidanceRepository,
		Effect.gen(function* () {
			const actual = yield* GlobalGuidanceRepository;
			return GlobalGuidanceRepository.of({
				...actual,
				Read: Effect.sync(() => {
					repository_reads += 1;
				}).pipe(Effect.andThen(actual.Read)),
			});
		}),
	).pipe(Layer.provide(repository));
	const counted_files = Layer.effect(
		GuidanceFileStore,
		Effect.gen(function* () {
			const actual = yield* GuidanceFileStore;
			return GuidanceFileStore.of({
				...actual,
				Read: (path) =>
					Effect.sync(() => {
						file_reads += 1;
					}).pipe(Effect.andThen(actual.Read(path))),
			});
		}),
	).pipe(Layer.provide(GuidanceFileStoreLive));
	const service = make_global_guidance_service_layer({
		backups_directory: join(root, "backups"),
		canonical_path: join(root, "guidance", "GLOBAL.md"),
		OnInitializeStart: Effect.gen(function* () {
			flights += 1;
			if (flights === 1) {
				yield* Deferred.succeed(entered, undefined);
				yield* Deferred.await(release);
				if (options.fail_first) {
					return yield* new GlobalGuidanceInvariantError({
						operation: "startup_test_failure",
					});
				}
			}
			if (flights === 2 && options.hold_refresh) {
				yield* Deferred.succeed(refresh_entered, undefined);
				yield* Deferred.await(release_refresh);
			}
		}),
		...(options.hold_claim
			? {
					OnInitializationClaimed: Deferred.succeed(claim_entered, undefined).pipe(
						Effect.andThen(Deferred.await(release_claim)),
					),
				}
			: {}),
		...(options.hold_publishing
			? {
					OnInitializationPublishing: Deferred.succeed(
						publishing_entered,
						undefined,
					).pipe(Effect.andThen(Deferred.await(release_publishing))),
				}
			: {}),
		...(options.hold_first_retry_observation
			? {
					OnRetryObserved: Effect.sync(() => {
						retry_observations += 1;
					}).pipe(
						Effect.andThen(
							Effect.suspend(() =>
								retry_observations === 1
									? Deferred.succeed(retry_observed, undefined).pipe(
											Effect.andThen(
												Deferred.await(release_retry_observation),
											),
										)
									: Effect.void,
							),
						),
					),
				}
			: {}),
	}).pipe(
		Layer.provideMerge(counted_repository),
		Layer.provideMerge(registry),
		Layer.provideMerge(counted_files),
		Layer.provideMerge(metadata),
		Layer.provideMerge(MakeSnowflakeIdLive(37).pipe(Layer.orDie)),
	);

	return {
		canonical: join(root, "guidance", "GLOBAL.md"),
		claim_entered,
		discoveries: () => discoveries,
		file_reads: () => file_reads,
		entered,
		flights: () => flights,
		layer: service,
		publishing_entered,
		repository_reads: () => repository_reads,
		release,
		release_claim,
		release_publishing,
		release_refresh,
		release_retry_observation,
		refresh_entered,
		retry_observed,
		runtime: ManagedRuntime.make(service),
	};
};

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("global guidance startup loading", () => {
	it("publishes the layer while one exact initialization flight holds authority", async () => {
		const fixture = await make_runtime();

		try {
			const guidance = await fixture.runtime.runPromise(
				Effect.service(GlobalGuidanceService),
			);
			await Effect.runPromise(Deferred.await(fixture.entered));
			expect(fixture.flights()).toBe(1);
			expect(fixture.discoveries()).toBe(0);

			const get = fixture.runtime.runPromise(guidance.Get);
			const update = fixture.runtime.runPromise(
				guidance.Update({
					content: "Do not run early.",
					message_id: "early_update",
					origin: "frontend",
					sent_at: "2026-08-15T12:00:00.000Z",
				}),
			);
			const engine = fixture.runtime.runPromise(guidance.ResolveForEngine("claude"));

			await expect(readFile(fixture.canonical, "utf8")).rejects.toMatchObject({
				code: "ENOENT",
			});
			expect(fixture.flights()).toBe(1);
			yield_release(fixture.release);

			const [snapshot, mutation, resolved] = await Promise.all([get, update, engine]);
			expect(snapshot.content).toBe("");
			expect(mutation.snapshot.content).toBe("Do not run early.\n");
			expect(resolved).toMatchObject({ _tag: "Some" });
			expect(fixture.flights()).toBe(1);
			expect(await readFile(fixture.canonical, "utf8")).toBe("Do not run early.\n");
		} finally {
			await fixture.runtime.dispose();
		}
	});

	it("delivers one typed first-flight failure to every cold waiter and shares one retry", async () => {
		const fixture = await make_runtime({ fail_first: true });

		try {
			const guidance = await fixture.runtime.runPromise(
				Effect.service(GlobalGuidanceService),
			);
			await Effect.runPromise(Deferred.await(fixture.entered));
			const waiters = [
				fixture.runtime.runPromiseExit(guidance.Get),
				fixture.runtime.runPromiseExit(
					guidance.Update({
						content: "Blocked.",
						message_id: "blocked_update",
						origin: "frontend",
						sent_at: "2026-08-15T12:00:00.000Z",
					}),
				),
				fixture.runtime.runPromiseExit(guidance.ResolveForEngine("claude")),
			];
			yield_release(fixture.release);
			const first_exits = await Promise.all(
				waiters as ReadonlyArray<Promise<Exit.Exit<unknown, unknown>>>,
			);

			for (const exit of first_exits) {
				expect(Exit.isFailure(exit)).toBe(true);
				if (Exit.isFailure(exit)) {
					const error = Cause.findError(
						exit.cause as Cause.Cause<GlobalGuidanceInvariantError>,
					);
					expect(Result.isSuccess(error)).toBe(true);
					if (Result.isSuccess(error))
						expect(error.success).toMatchObject({
							operation: "startup_test_failure",
						});
				}
			}
			expect(fixture.flights()).toBe(1);
			expect(fixture.discoveries()).toBe(0);

			const retries = await Promise.all([
				fixture.runtime.runPromise(guidance.Initialize),
				fixture.runtime.runPromise(guidance.Initialize),
				fixture.runtime.runPromise(guidance.Initialize),
			]);
			expect(retries.map((snapshot) => snapshot.content)).toEqual(["", "", ""]);
			expect(fixture.flights()).toBe(2);
		} finally {
			await fixture.runtime.dispose();
		}
	});

	it("does not cancel the service-owned initializer when an external cold waiter interrupts", async () => {
		const fixture = await make_runtime();

		try {
			const guidance = await fixture.runtime.runPromise(
				Effect.service(GlobalGuidanceService),
			);
			const caller = fixture.runtime.runFork(guidance.Get);
			await Effect.runPromise(Deferred.await(fixture.entered));
			await Effect.runPromise(Fiber.interrupt(caller));
			expect(fixture.flights()).toBe(1);
			yield_release(fixture.release);

			await expect(fixture.runtime.runPromise(guidance.Get)).resolves.toMatchObject({
				content: "",
			});
			expect(fixture.flights()).toBe(1);
		} finally {
			await fixture.runtime.dispose();
		}
	});

	it("settles externally scoped waiters on service close without late startup publication", async () => {
		const fixture = await make_runtime();
		const guidance = await fixture.runtime.runPromise(Effect.service(GlobalGuidanceService));
		await Effect.runPromise(Deferred.await(fixture.entered));
		const waiters = [
			fixture.runtime.runPromiseExit(guidance.Get),
			fixture.runtime.runPromiseExit(guidance.ResolveForEngine("claude")),
		];

		await fixture.runtime.dispose();
		const exits = await Promise.all(
			waiters as ReadonlyArray<Promise<Exit.Exit<unknown, unknown>>>,
		);
		for (const exit of exits) expect(Exit.isFailure(exit)).toBe(true);
		expect(fixture.discoveries()).toBe(0);
		await expect(readFile(fixture.canonical, "utf8")).rejects.toMatchObject({
			code: "ENOENT",
		});
		yield_release(fixture.release);
		expect(fixture.flights()).toBe(1);
		expect(fixture.discoveries()).toBe(0);
	});

	it("retains established refresh behavior for a Get begun after startup is ready", async () => {
		const fixture = await make_runtime();

		try {
			const guidance = await fixture.runtime.runPromise(
				Effect.service(GlobalGuidanceService),
			);
			await Effect.runPromise(Deferred.await(fixture.entered));
			yield_release(fixture.release);
			await fixture.runtime.runPromise(guidance.Get);
			expect(fixture.flights()).toBe(1);
			expect(fixture.discoveries()).toBe(3);
			expect(fixture.file_reads()).toBe(4);
			expect(fixture.repository_reads()).toBe(2);
			await fixture.runtime.runPromise(guidance.Get);
			expect(fixture.flights()).toBe(2);
		} finally {
			await fixture.runtime.dispose();
		}
	});

	it("shares one ready refresh across Get and Initialize, then admits a later refresh", async () => {
		const fixture = await make_runtime({ hold_refresh: true });

		try {
			const guidance = await fixture.runtime.runPromise(
				Effect.service(GlobalGuidanceService),
			);
			await Effect.runPromise(Deferred.await(fixture.entered));
			yield_release(fixture.release);
			await fixture.runtime.runPromise(guidance.Get);
			const before_refresh = {
				discoveries: fixture.discoveries(),
				file_reads: fixture.file_reads(),
				repository_reads: fixture.repository_reads(),
			};

			const overlapping = [
				fixture.runtime.runPromise(guidance.Get),
				fixture.runtime.runPromise(guidance.Initialize),
				fixture.runtime.runPromise(guidance.Get),
			];
			await Effect.runPromise(Deferred.await(fixture.refresh_entered));
			expect(fixture.flights()).toBe(2);
			yield_release(fixture.release_refresh);
			await expect(Promise.all(overlapping)).resolves.toHaveLength(3);
			expect(fixture.flights()).toBe(2);
			expect(fixture.discoveries() - before_refresh.discoveries).toBe(2);
			expect(fixture.file_reads() - before_refresh.file_reads).toBe(1);
			expect(fixture.repository_reads() - before_refresh.repository_reads).toBe(2);
			const after_refresh = {
				discoveries: fixture.discoveries(),
				file_reads: fixture.file_reads(),
				repository_reads: fixture.repository_reads(),
			};

			await fixture.runtime.runPromise(guidance.Get);
			expect(fixture.flights()).toBe(3);
			expect(fixture.discoveries() - after_refresh.discoveries).toBe(2);
			expect(fixture.file_reads() - after_refresh.file_reads).toBe(1);
			expect(fixture.repository_reads() - after_refresh.repository_reads).toBe(2);
		} finally {
			await fixture.runtime.dispose();
		}
	});

	it("shares a retry observed from failed startup even after another caller reaches ready", async () => {
		const fixture = await make_runtime({
			fail_first: true,
			hold_first_retry_observation: true,
		});

		try {
			const guidance = await fixture.runtime.runPromise(
				Effect.service(GlobalGuidanceService),
			);
			await Effect.runPromise(Deferred.await(fixture.entered));
			yield_release(fixture.release);
			const failed = await fixture.runtime.runPromiseExit(guidance.Get);
			expect(Exit.isFailure(failed)).toBe(true);

			const late_retry = fixture.runtime.runPromise(guidance.Initialize);
			await Effect.runPromise(Deferred.await(fixture.retry_observed));
			await expect(fixture.runtime.runPromise(guidance.Initialize)).resolves.toMatchObject({
				content: "",
			});
			expect(fixture.flights()).toBe(2);
			yield_release(fixture.release_retry_observation);
			await expect(late_retry).resolves.toMatchObject({ content: "" });
			expect(fixture.flights()).toBe(2);
		} finally {
			await fixture.runtime.dispose();
		}
	});

	it("settles a waiter in an external scope when the service scope closes", async () => {
		const fixture = await make_runtime();
		const service_scope = await Effect.runPromise(Scope.make());
		const context = await Effect.runPromise(Layer.buildWithScope(fixture.layer, service_scope));
		const guidance = Context.get(context, GlobalGuidanceService);
		const caller_scope = await Effect.runPromise(Scope.make());

		try {
			await Effect.runPromise(Deferred.await(fixture.entered));
			const waiter = await Effect.runPromise(
				Effect.forkIn(guidance.Get.pipe(Effect.exit), caller_scope),
			);
			await Effect.runPromise(Effect.yieldNow);
			await Effect.runPromise(Scope.close(service_scope, Exit.void));
			const exit = await Effect.runPromise(Fiber.join(waiter));
			expect(Exit.isFailure(exit)).toBe(true);
			expect(fixture.discoveries()).toBe(0);
		} finally {
			await Effect.runPromise(Scope.close(caller_scope, Exit.void));
		}
	});

	it("does not strand an externally scoped waiter when close wins the claimed-owner race", async () => {
		const fixture = await make_runtime({ hold_claim: true });
		const service_scope = await Effect.runPromise(Scope.make());
		const context = await Effect.runPromise(Layer.buildWithScope(fixture.layer, service_scope));
		const guidance = Context.get(context, GlobalGuidanceService);
		const caller_scope = await Effect.runPromise(Scope.make());

		try {
			await Effect.runPromise(Deferred.await(fixture.claim_entered));
			const waiter = await Effect.runPromise(
				Effect.forkIn(guidance.Get.pipe(Effect.exit), caller_scope),
			);
			const closer_scope = await Effect.runPromise(Scope.make());
			await Effect.runPromise(
				Effect.forkIn(Scope.close(service_scope, Exit.void), closer_scope, {
					startImmediately: true,
				}),
			);
			const waited = await Effect.runPromise(
				Fiber.join(waiter).pipe(Effect.timeoutOption("1 second")),
			);
			expect(waited._tag).toBe("Some");
			if (waited._tag !== "Some")
				throw new Error("service close did not settle the external waiter");
			const exit = waited.value;
			await Effect.runPromise(Scope.close(closer_scope, Exit.void));
			expect(Exit.isFailure(exit)).toBe(true);
			expect(fixture.flights()).toBe(0);
			expect(fixture.discoveries()).toBe(0);
		} finally {
			await Effect.runPromise(Scope.close(caller_scope, Exit.void));
		}
	});

	it("settles external waiters when close interrupts a flight before completion publication", async () => {
		const fixture = await make_runtime({ hold_publishing: true });
		const service_scope = await Effect.runPromise(Scope.make());
		const context = await Effect.runPromise(Layer.buildWithScope(fixture.layer, service_scope));
		const guidance = Context.get(context, GlobalGuidanceService);
		const caller_scope = await Effect.runPromise(Scope.make());

		try {
			await Effect.runPromise(Deferred.await(fixture.entered));
			yield_release(fixture.release);
			await Effect.runPromise(Deferred.await(fixture.publishing_entered));
			const discoveries_before_close = fixture.discoveries();
			const waiter = await Effect.runPromise(
				Effect.forkIn(guidance.Get.pipe(Effect.exit), caller_scope),
			);
			const closer_scope = await Effect.runPromise(Scope.make());
			await Effect.runPromise(
				Effect.forkIn(Scope.close(service_scope, Exit.void), closer_scope, {
					startImmediately: true,
				}),
			);
			const waited = await Effect.runPromise(
				Fiber.join(waiter).pipe(Effect.timeoutOption("1 second")),
			);
			expect(waited._tag).toBe("Some");
			if (waited._tag !== "Some") {
				yield_release(fixture.release_publishing);
				throw new Error("service close did not settle the external waiter");
			}
			const exit = waited.value;
			yield_release(fixture.release_publishing);
			await Effect.runPromise(Scope.close(closer_scope, Exit.void));
			expect(Exit.isFailure(exit)).toBe(true);
			expect(fixture.flights()).toBe(1);
			expect(fixture.discoveries()).toBe(discoveries_before_close);
		} finally {
			await Effect.runPromise(Scope.close(caller_scope, Exit.void));
		}
	});
});

const yield_release = (release: Deferred.Deferred<void>) => {
	void Effect.runPromise(Deferred.succeed(release, undefined));
};
