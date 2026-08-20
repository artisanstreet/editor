import { Deferred, Effect, Fiber, Layer, Option } from "effect";
import { describe, expect, it } from "vitest";

import {
	PreviewCoordinator,
	PreviewCoordinatorLive,
} from "../../modules/backend/src/preview/coordinator";
import {
	PreviewRepository,
	PreviewRepositoryError,
} from "../../modules/backend/src/preview/repository";
import { RichLinkAssetStore } from "../../modules/backend/src/preview/rich-link-asset-store";
import { RichLinkMetadata } from "../../modules/backend/src/preview/rich-link-metadata";
import {
	PreviewExternalBrowser,
	PreviewInspection,
} from "../../modules/backend/src/preview/runtime";
import { PreviewService } from "../../modules/backend/src/preview/service";
import { PreviewHealthProbe, PreviewTarget } from "../../modules/backend/src/preview/target";

const restored_target = {
	created_at: "2026-01-01T00:00:00.000Z",
	health_json: null,
	last_error: null,
	launch_state: "idle",
	port: 5173,
	project_id: "project-1",
	routes_json: "[]",
	source_json: null,
	state: "registered",
	target_id: "restored",
	thread_id: "thread-1",
	updated_at: "2026-01-01T00:00:00.000Z",
	url: "http://localhost:5173/",
	workspace_id: "workspace-1",
} as const;

const requested_target = { ...restored_target, target_id: "requested" } as const;

function register_input(id = "requested") {
	return {
		id,
		message_id: `message-${id}`,
		port: 5173,
		project_id: "project-1",
		routes: [],
		thread_id: "thread-1",
		url: "http://localhost:5173/",
		workspace_id: "workspace-1",
	} as const;
}

function make_layer(options: {
	readonly fail_recovery?: boolean;
	readonly release: Deferred.Deferred<void>;
	readonly recovery_started: Deferred.Deferred<void>;
	readonly steps: Array<string>;
}) {
	let list_calls = 0;
	const service = {
		AcquireDispatchLease: () =>
			Effect.succeed({
				acquired_at: "2026-01-01T00:00:00.000Z",
				expires_at: "2026-01-01T00:01:00.000Z",
				kind: "probe" as const,
				lease_id: "lease-1",
				owner_instance_id: "instance-1",
				session_id: null,
				target_id: "requested",
				thread_id: "thread-1",
			}),
		Get: () => Effect.succeed(requested_target),
		List: () =>
			Effect.sync(() => {
				list_calls += 1;
				return list_calls;
			}).pipe(
				Effect.flatMap((call) =>
					call === 1
						? Deferred.succeed(options.recovery_started, undefined).pipe(
								Effect.andThen(Deferred.await(options.release)),
								Effect.andThen(
									options.fail_recovery
										? Effect.fail(
												new PreviewRepositoryError({
													code: "storage",
													message: "recovery unavailable",
												}),
											)
										: Effect.sync(() => {
												options.steps.push("targets");
												return [restored_target];
											}),
								),
							)
						: Effect.succeed([requested_target] as never),
				),
			),
		RecoverDispatchLeases: () =>
			Effect.sync(() => {
				options.steps.push("leases");
				return [];
			}),
		RecoverInspections: () =>
			Effect.sync(() => {
				options.steps.push("inspections");
				return [];
			}),
		Register: () =>
			Effect.sync(() => {
				options.steps.push("register");
				return requested_target;
			}),
		ReleaseDispatchLease: () => Effect.void,
		RenewDispatchLease: () => Effect.die("not used"),
		ReplayTargetUpdate: () => Effect.succeed(Option.none()),
		UpdateInspection: () => Effect.die("not used"),
		UpdateTarget: () => Effect.succeed(requested_target),
		ValidateTargetUrl: (url: string) => Effect.succeed(url),
	};
	const runtime_targets = {
		Get: () => Effect.succeed(Option.none()),
		List: () => Effect.succeed([]),
		Probe: () => Effect.die("not used"),
		Register: (input: { readonly id: string }) =>
			Effect.sync(() => {
				options.steps.push(`runtime:${input.id}`);
				return {
					created_at_ms: 0,
					health: Option.none(),
					id: input.id,
					project_id: "project-1",
					source: Option.none(),
					state: "registered" as const,
					updated_at_ms: 0,
					url: "http://localhost:5173/",
					workspace_id: "workspace-1",
				};
			}),
		Remove: () => Effect.void,
		SetState: () => Effect.die("not used"),
		SlidingEvents: undefined,
	};

	return PreviewCoordinatorLive.pipe(
		Layer.provide(
			Layer.mergeAll(
				Layer.succeed(PreviewService, service as never),
				Layer.succeed(PreviewRepository, {
					ListOpenInspections: () => Effect.succeed([]),
				} as never),
				Layer.succeed(PreviewTarget, runtime_targets as never),
				Layer.succeed(PreviewHealthProbe, {
					Probe: () =>
						Effect.sync(() => options.steps.push("probe")).pipe(
							Effect.as({
								latency_ms: 1,
								message: Option.none(),
								status: "healthy" as const,
								status_code: Option.some(200),
							}),
						),
				} as never),
				Layer.succeed(PreviewExternalBrowser, {
					Launch: () => Effect.die("not used"),
				} as never),
				Layer.succeed(PreviewInspection, { Close: () => Effect.void } as never),
				Layer.succeed(RichLinkMetadata, { Resolve: () => Effect.die("not used") } as never),
				Layer.succeed(RichLinkAssetStore, {
					Get: () => Effect.succeed(Option.none()),
					limits: { max_entries: 1, max_total_bytes: 1 },
					Put: () => Effect.die("not used"),
				} as never),
			),
		),
	);
}

describe("PreviewCoordinatorLive recovery loading", () => {
	it("constructs before recovery, serves durable reads, then admits runtime work in recovery order", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const release = yield* Deferred.make<void>();
					const recovery_started = yield* Deferred.make<void>();
					const steps: Array<string> = [];
					return yield* Effect.gen(function* () {
						const coordinator = yield* PreviewCoordinator;
						yield* Deferred.await(recovery_started);

						expect(yield* coordinator.Get("requested")).toMatchObject({
							id: "requested",
						});
						expect(yield* coordinator.List()).toHaveLength(1);
						expect(yield* coordinator.Asset("missing")).toEqual(Option.none());
						const registration = yield* coordinator
							.Register(register_input())
							.pipe(Effect.forkChild);
						yield* Effect.yieldNow;
						expect(steps).toEqual([]);

						yield* Deferred.succeed(release, undefined);
						yield* Fiber.join(registration);
						return steps;
					}).pipe(Effect.provide(make_layer({ recovery_started, release, steps })));
				}),
			),
		);
		expect(result).toEqual([
			"targets",
			"runtime:restored",
			"leases",
			"inspections",
			"register",
			"runtime:requested",
		]);
	});

	it("does not admit a probe until recovery has reconstructed runtime state", async () => {
		const steps = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const release = yield* Deferred.make<void>();
					const recovery_started = yield* Deferred.make<void>();
					const steps: Array<string> = [];
					return yield* Effect.gen(function* () {
						const coordinator = yield* PreviewCoordinator;
						yield* Deferred.await(recovery_started);
						const probe = yield* coordinator
							.Probe({ message_id: "probe", target_id: "requested" })
							.pipe(Effect.forkChild);
						yield* Effect.yieldNow;
						expect(steps).toEqual([]);
						yield* Deferred.succeed(release, undefined);
						yield* Fiber.join(probe);
						return steps;
					}).pipe(Effect.provide(make_layer({ recovery_started, release, steps })));
				}),
			),
		);

		expect(steps).toEqual(["targets", "runtime:restored", "leases", "inspections", "probe"]);
	});

	it("returns the typed recovery failure to runtime operations while quiesce can drain", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const release = yield* Deferred.make<void>();
					const recovery_started = yield* Deferred.make<void>();
					const steps: Array<string> = [];
					return yield* Effect.gen(function* () {
						const coordinator = yield* PreviewCoordinator;
						yield* Deferred.await(recovery_started);
						yield* Deferred.succeed(release, undefined);
						const error = yield* coordinator
							.Register(register_input())
							.pipe(Effect.flip);
						yield* coordinator.QuiesceThread("thread-1");
						return { error, steps };
					}).pipe(
						Effect.provide(
							make_layer({ fail_recovery: true, recovery_started, release, steps }),
						),
					);
				}),
			),
		);

		expect(result.error).toBeInstanceOf(PreviewRepositoryError);
		expect(result.error).toMatchObject({ code: "storage", message: "recovery unavailable" });
		expect(result.steps).toEqual([]);
	});

	it("interrupts a held recovery and gated waiter when its service scope closes", async () => {
		const interrupted = { value: false };
		const steps: Array<string> = [];
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const release = yield* Deferred.make<void>();
					const recovery_started = yield* Deferred.make<void>();
					return yield* Effect.gen(function* () {
						const coordinator = yield* PreviewCoordinator;
						yield* Deferred.await(recovery_started);
						yield* Effect.forkScoped(
							coordinator.Register(register_input()).pipe(
								Effect.onInterrupt(() =>
									Effect.sync(() => {
										interrupted.value = true;
									}),
								),
							),
						);
						yield* Effect.yieldNow;
					}).pipe(Effect.provide(make_layer({ recovery_started, release, steps })));
				}),
			),
		);

		expect(interrupted.value).toBe(true);
		expect(steps).toEqual([]);
	});
});
