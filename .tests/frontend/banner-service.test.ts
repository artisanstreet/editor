import { describe, expect, it } from "vitest";
import { Deferred, Effect, Layer } from "effect";
import { ArtisanClientError } from "../../modules/transport/src/client-contract";

import {
	ForgeConnectionBannerId,
	PresentForgeConnectionState,
} from "../../modules/frontend/src/lib/banner/connection-banner";
import {
	BannerPresenter,
	BannerReporter,
	BannerReporterError,
	BannerService,
	BannerServiceLive,
	is_banner_executable_action,
	type BannerEvent,
	type BannerReportEvent,
} from "../../modules/frontend/src/lib/banner/service";

describe("BannerService", () => {
	it("presents every severity and forwards structured events to the reporter", async () => {
		const presented: Array<BannerEvent> = [];
		const reported: Array<BannerReportEvent> = [];
		const dependencies = Layer.merge(
			Layer.succeed(
				BannerPresenter,
				BannerPresenter.of({
					Dismiss: () => Effect.void,
					Show: (event) => Effect.sync(() => presented.push(event)),
				}),
			),
			Layer.succeed(
				BannerReporter,
				BannerReporter.of({
					Report: (event) => Effect.sync(() => reported.push(event)),
				}),
			),
		);

		await Effect.runPromise(
			Effect.gen(function* () {
				const banner = yield* BannerService;
				yield* banner.error("Error", { code: "test.error" });
				yield* banner.warning("Warning");
				yield* banner.info("Info");
				yield* banner.success("Success");
			}).pipe(Effect.provide(BannerServiceLive.pipe(Layer.provide(dependencies)))),
		);

		expect(presented.map(({ severity }) => severity)).toEqual([
			"error",
			"warning",
			"info",
			"success",
		]);
		expect(reported).toEqual(presented);
	});

	it("does not fail a displayed banner when reporting fails", async () => {
		const presented: Array<BannerEvent> = [];
		const dependencies = Layer.merge(
			Layer.succeed(
				BannerPresenter,
				BannerPresenter.of({
					Dismiss: () => Effect.void,
					Show: (event) => Effect.sync(() => presented.push(event)),
				}),
			),
			Layer.succeed(
				BannerReporter,
				BannerReporter.of({
					Report: () =>
						Effect.fail(
							new BannerReporterError({ cause: new Error("telemetry unavailable") }),
						),
				}),
			),
		);

		await Effect.runPromise(
			Effect.gen(function* () {
				const banner = yield* BannerService;
				yield* banner.error("Still visible");
			}).pipe(Effect.provide(BannerServiceLive.pipe(Layer.provide(dependencies)))),
		);

		expect(presented).toHaveLength(1);
	});

	it("executes data-driven banner actions through the scoped Effect worker", async () => {
		const dependencies = Layer.merge(
			Layer.succeed(
				BannerPresenter,
				BannerPresenter.of({
					Dismiss: () => Effect.void,
					Show: (event, on_action) =>
						Effect.sync(() => {
							const action = event.actions?.[0];
							if (action !== undefined && is_banner_executable_action(action)) {
								on_action(action);
							}
						}),
				}),
			),
			Layer.succeed(BannerReporter, BannerReporter.of({ Report: () => Effect.void })),
		);

		await Effect.runPromise(
			Effect.gen(function* () {
				const completed = yield* Deferred.make<void>();
				const banner = yield* BannerService;
				yield* banner.error("Disconnected", {
					id: "connection",
					actions: [
						{
							Execute: Deferred.succeed(completed, undefined).pipe(Effect.asVoid),
							icon: "refresh",
							id: "retry",
							label: "Retry now",
						},
					],
				});
				yield* Deferred.await(completed);
			}).pipe(Effect.provide(BannerServiceLive.pipe(Layer.provide(dependencies)))),
		);
	});

	it("replaces connection progress with errors and dismisses it when ready", async () => {
		const presented: Array<BannerEvent> = [];
		const dismissed: Array<string> = [];
		const dependencies = Layer.merge(
			Layer.succeed(
				BannerPresenter,
				BannerPresenter.of({
					Dismiss: (id) => Effect.sync(() => dismissed.push(id)),
					Show: (event) => Effect.sync(() => presented.push(event)),
				}),
			),
			Layer.succeed(BannerReporter, BannerReporter.of({ Report: () => Effect.void })),
		);
		const runtime = BannerServiceLive.pipe(Layer.provide(dependencies));

		await Effect.runPromise(
			Effect.gen(function* () {
				yield* PresentForgeConnectionState({ phase: "connecting" }, Effect.void);
				yield* PresentForgeConnectionState(
					{
						attempts: 5,
						error: new ArtisanClientError({
							cause: new Error("opaque browser event"),
							code: "connection",
							message: "Transport bootstrap failed.",
							protocol_code: "transport.connection",
							retryable: true,
						}),
						phase: "exhausted",
					},
					Effect.void,
				);
				yield* PresentForgeConnectionState({ phase: "ready" }, Effect.void);
			}).pipe(Effect.provide(runtime)),
		);

		expect(presented).toMatchObject([
			{
				description: "Keep this page open while Artisan establishes the session.",
				id: ForgeConnectionBannerId,
				severity: "info",
				title: "Connecting to Forge…",
			},
			{
				actions: [
					{
						href: "artisan://forge/start",
						id: "start-forge",
						label: "Start Forge",
					},
					{
						id: "retry",
						label: "Retry now",
					},
				],
				description: "Start the installed local service, or retry this connection.",
				id: ForgeConnectionBannerId,
				severity: "error",
				title: "Could not connect to Forge",
			},
		]);
		expect(dismissed).toEqual([ForgeConnectionBannerId]);
	});
});
