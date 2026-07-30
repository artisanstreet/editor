import { Deferred, Effect, Fiber, Layer, Option, Stream } from "effect";
import { describe, expect, it } from "vitest";

import {
	PreviewHealthProbe,
	PreviewTarget,
	PreviewTargetClock,
} from "../../modules/backend/src/preview/target";
import { make_preview_target_layer } from "../../modules/backend/src/preview/target-service";
import { make_in_memory_rich_link_asset_store_layer } from "../../modules/backend/src/preview/rich-link-asset-store";
import {
	RichLinkClock,
	RichLinkDnsResolver,
	RichLinkHttpTransport,
	RichLinkMetadata,
} from "../../modules/backend/src/preview/rich-link-metadata";
import { make_in_memory_rich_link_cache_layer } from "../../modules/backend/src/preview/rich-link-infrastructure";
import { make_rich_link_metadata_layer } from "../../modules/backend/src/preview/rich-link-service";

function make_target_test_layer(active_probes: { value: number }) {
	let now_ms = 10_000;
	const clock = Layer.succeed(PreviewTargetClock, {
		Now: Effect.sync(() => now_ms++),
	});
	const probe = Layer.succeed(PreviewHealthProbe, {
		Probe: () =>
			Effect.acquireRelease(
				Effect.sync(() => {
					active_probes.value += 1;
				}),
				() =>
					Effect.sync(() => {
						active_probes.value -= 1;
					}),
			).pipe(
				Effect.as({
					latency_ms: 12,
					message: Option.none<string>(),
					status: "healthy" as const,
					status_code: Option.some(200),
				}),
			),
	});

	return make_preview_target_layer({ sliding_event_capacity: 2 }).pipe(
		Layer.provide(Layer.merge(clock, probe)),
	);
}

describe("PreviewTarget", () => {
	it("registers localhost targets while external metadata rejects the same URL", async () => {
		const active_probes = { value: 0 };
		const target_layer = make_target_test_layer(active_probes);
		const target = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* PreviewTarget;

				return yield* registry.Register({
					id: "preview-1",
					project_id: "project-1",
					source: { kind: "terminal", terminal_id: "terminal-1" },
					url: "http://localhost:5173/app",
					workspace_id: "workspace-1",
				});
			}).pipe(Effect.provide(target_layer)),
		);

		expect(target.url).toBe("http://localhost:5173/app");
		expect(Option.getOrThrow(target.source)).toEqual({
			kind: "terminal",
			terminal_id: "terminal-1",
		});

		let transport_calls = 0;
		const rich_infrastructure = Layer.mergeAll(
			Layer.succeed(RichLinkDnsResolver, {
				Resolve: () => Effect.succeed([{ address: "93.184.216.34", family: 4 }]),
			}),
			Layer.succeed(RichLinkHttpTransport, {
				Request: () =>
					Effect.sync(() => {
						transport_calls += 1;

						return { body: new Uint8Array(), headers: {}, status: 200 };
					}),
			}),
			Layer.succeed(RichLinkClock, { Now: Effect.succeed(0) }),
			make_in_memory_rich_link_cache_layer(),
			make_in_memory_rich_link_asset_store_layer(),
		);
		const rich_layer = make_rich_link_metadata_layer().pipe(Layer.provide(rich_infrastructure));
		const error = await Effect.runPromise(
			Effect.gen(function* () {
				const metadata = yield* RichLinkMetadata;

				return yield* metadata.Resolve(target.url).pipe(Effect.flip);
			}).pipe(Effect.provide(rich_layer)),
		);

		expect(error.code).toBe("blocked_address");
		expect(transport_calls).toBe(0);
	});

	it("maintains a provider-neutral read model and bounded sliding status stream", async () => {
		const active_probes = { value: 0 };
		const layer = make_target_test_layer(active_probes);
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* PreviewTarget;
				const events_fiber = yield* registry.SlidingEvents.pipe(
					Stream.take(2),
					Stream.runCollect,
					Effect.forkChild,
				);

				yield* Effect.yieldNow;
				yield* registry.Register({
					id: "preview-2",
					project_id: "project-2",
					source: { kind: "process", process_id: "process-2" },
					url: "http://127.0.0.1:4173/",
					workspace_id: "workspace-2",
				});
				const stopped = yield* registry.SetState("preview-2", "stopped");
				const events = yield* Fiber.join(events_fiber);
				const listed = yield* registry.List("workspace-2");

				return { events, listed, stopped };
			}).pipe(Effect.provide(layer)),
		);

		expect(result.stopped.state).toBe("stopped");
		expect(result.listed).toHaveLength(1);
		expect(result.events.map((event) => event.kind)).toEqual(["registered", "state"]);
	});

	it("drops the oldest unread events when the sliding stream reaches capacity", async () => {
		const active_probes = { value: 0 };
		const layer = make_target_test_layer(active_probes);
		const events = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* PreviewTarget;
				const first_received = yield* Deferred.make<void>();
				const release_first = yield* Deferred.make<void>();
				let is_first = true;
				const events_fiber = yield* registry.SlidingEvents.pipe(
					Stream.mapEffect((event) =>
						Effect.gen(function* () {
							if (is_first) {
								is_first = false;
								yield* Deferred.succeed(first_received, undefined);
								yield* Deferred.await(release_first);
							}

							return event;
						}),
					),
					Stream.take(3),
					Stream.runCollect,
					Effect.forkChild,
				);

				yield* Effect.yieldNow;
				yield* registry.Register({
					id: "preview-sliding",
					project_id: "project-sliding",
					url: "http://localhost:5173/",
					workspace_id: "workspace-sliding",
				});
				yield* Deferred.await(first_received);
				yield* registry.SetState("preview-sliding", "stopped");
				yield* registry.SetState("preview-sliding", "registered");
				yield* registry.SetState("preview-sliding", "healthy");
				yield* Deferred.succeed(release_first, undefined);

				return yield* Fiber.join(events_fiber);
			}).pipe(Effect.provide(layer)),
		);

		expect(events.map((event) => event.kind)).toEqual(["registered", "state", "state"]);
		expect(events.map((event) => event.target.state)).toEqual([
			"registered",
			"registered",
			"healthy",
		]);
	});

	it("owns health probe resources within caller scope", async () => {
		const active_probes = { value: 0 };
		const layer = make_target_test_layer(active_probes);
		const healthy = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const registry = yield* PreviewTarget;

					yield* registry.Register({
						id: "preview-3",
						project_id: "project-3",
						url: "http://[::1]:3000/",
						workspace_id: "workspace-3",
					});

					return yield* registry.Probe("preview-3");
				}),
			).pipe(Effect.provide(layer)),
		);

		expect(healthy.state).toBe("healthy");
		expect(Option.getOrThrow(healthy.health).status_code).toEqual(Option.some(200));
		expect(active_probes.value).toBe(0);
	});

	it("rejects public URLs and duplicate registrations", async () => {
		const active_probes = { value: 0 };
		const layer = make_target_test_layer(active_probes);
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* PreviewTarget;
				const registration = {
					id: "preview-4",
					project_id: "project-4",
					url: "http://localhost:8080/",
					workspace_id: "workspace-4",
				} as const;

				yield* registry.Register(registration);

				const duplicate = yield* registry.Register(registration).pipe(Effect.flip);
				const external = yield* registry
					.Register({ ...registration, id: "external", url: "https://example.com/" })
					.pipe(Effect.flip);

				return { duplicate, external };
			}).pipe(Effect.provide(layer)),
		);

		expect(result.duplicate.code).toBe("duplicate");
		expect(result.external.code).toBe("invalid_target");
	});

	it("rejects empty target, project, and workspace identifiers", async () => {
		const active_probes = { value: 0 };
		const layer = make_target_test_layer(active_probes);
		const errors = await Effect.runPromise(
			Effect.gen(function* () {
				const registry = yield* PreviewTarget;
				const registrations = [
					{
						id: " ",
						project_id: "project",
						url: "http://localhost:5173/",
						workspace_id: "workspace",
					},
					{
						id: "target",
						project_id: "\t",
						url: "http://localhost:5173/",
						workspace_id: "workspace",
					},
					{
						id: "target",
						project_id: "project",
						url: "http://localhost:5173/",
						workspace_id: "",
					},
				] as const;

				return yield* Effect.forEach(
					registrations,
					(registration) => registry.Register(registration).pipe(Effect.flip),
					{ concurrency: 1 },
				);
			}).pipe(Effect.provide(layer)),
		);

		expect(errors.map((error) => error.code)).toEqual([
			"invalid_target",
			"invalid_target",
			"invalid_target",
		]);
	});
});
