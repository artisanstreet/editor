import { describe, expect, it } from "vitest";
import { Deferred, Effect, Layer } from "effect";
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
});
