import { Deferred, Effect, Exit, Fiber, Layer, Schema, Stream } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";

import { RichLinkFavicon } from "@artisan/protocol";
import { ArtisanClient, ArtisanClientError } from "@artisan/transport/client";
import {
	maximum_retained_rich_link_asset_bytes,
	maximum_retained_rich_link_assets,
	RichLinkAssetController,
	RichLinkAssetControllerLive,
} from "../../modules/frontend/src/lib/components/markdown/rich-link-asset-controller";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";

const Favicon = (asset_id: string, bytes: number) =>
	Schema.decodeUnknownSync(RichLinkFavicon)({
		asset_id,
		bytes,
		content_type: "image/png",
		source: "fallback",
		source_url: "https://example.com/favicon.png",
	});

const AssetId = (index: number) => index.toString(16).padStart(64, "0");

describe("rich link asset controller", () => {
	it("shares one app-owned asset stream across concurrent and remounted anchors", async () => {
		let opens = 0;
		const bytes = new Uint8Array([1, 2, 3]);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const services = yield* Layer.build(
						Layer.provide(
							RichLinkAssetControllerLive,
							Layer.succeed(ArtisanClient, {
								...FixtureArtisanClientService,
								OpenAsset: () =>
									Effect.succeed(
										Stream.fromEffect(
											Effect.gen(function* () {
												opens += 1;
												yield* Deferred.succeed(started, undefined);
												yield* Deferred.await(release);
												return bytes;
											}),
										),
									),
							}),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* RichLinkAssetController;
						const favicon = Favicon(AssetId(1), bytes.byteLength);
						const starter = yield* controller.Load(favicon).pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						const follower = yield* controller.Load(favicon).pipe(Effect.forkScoped);
						yield* Fiber.interrupt(starter);
						yield* Deferred.succeed(release, undefined);
						const loaded = yield* Fiber.join(follower);
						const remounted = yield* controller.Load(favicon);
						return { loaded, remounted };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(opens).toBe(1);
		expect(result.loaded?.bytes).toEqual(bytes);
		expect(result.remounted?.bytes).toEqual(bytes);
		expect(result.loaded?.bytes).not.toBe(result.remounted?.bytes);
	});

	it("removes failed flights so the next anchor can retry", async () => {
		let opens = 0;
		const bytes = new Uint8Array([4]);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const services = yield* Layer.build(
						Layer.provide(
							RichLinkAssetControllerLive,
							Layer.succeed(ArtisanClient, {
								...FixtureArtisanClientService,
								OpenAsset: () => {
									opens += 1;
									return opens === 1
										? Effect.fail(
												new ArtisanClientError({
													cause: "test",
													code: "stream_closed",
													message: "closed",
													protocol_code: "test",
													retryable: true,
												}),
											)
										: Effect.succeed(Stream.make(bytes));
								},
							}),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* RichLinkAssetController;
						const favicon = Favicon(AssetId(2), bytes.byteLength);
						const failed = yield* controller.Load(favicon).pipe(Effect.exit);
						const retried = yield* controller.Load(favicon);
						return { failed, retried };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(opens).toBe(2);
		expect(Exit.isFailure(result.failed)).toBe(true);
		expect(result.retried?.bytes).toEqual(bytes);
	});

	it("does not retain a length-invalid stream and retries it on the next mount", async () => {
		let opens = 0;
		const expected = new Uint8Array([5, 6]);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const services = yield* Layer.build(
						Layer.provide(
							RichLinkAssetControllerLive,
							Layer.succeed(ArtisanClient, {
								...FixtureArtisanClientService,
								OpenAsset: () => {
									opens += 1;
									return Effect.succeed(
										Stream.make(opens === 1 ? new Uint8Array([0]) : expected),
									);
								},
							}),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* RichLinkAssetController;
						const favicon = Favicon(AssetId(3), expected.byteLength);
						const rejected = yield* controller.Load(favicon);
						const retried = yield* controller.Load(favicon);
						return { rejected, retried };
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(opens).toBe(2);
		expect(result.rejected).toBeUndefined();
		expect(result.retried?.bytes).toEqual(expected);
	});

	it("expires one shared timed-out flight at its deadline before a later anchor reopens it", async () => {
		let opens = 0;
		const bytes = new Uint8Array([7]);
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const services = yield* Layer.build(
						Layer.provide(
							RichLinkAssetControllerLive,
							Layer.succeed(ArtisanClient, {
								...FixtureArtisanClientService,
								OpenAsset: () =>
									Effect.gen(function* () {
										opens += 1;
										if (opens === 1) {
											yield* Deferred.succeed(started, undefined);
											return Stream.never;
										}
										return Stream.make(bytes);
									}),
							}),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* RichLinkAssetController;
						const favicon = Favicon(AssetId(4), bytes.byteLength);
						const first = yield* controller.Load(favicon).pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						const follower = yield* controller.Load(favicon).pipe(Effect.forkScoped);
						/** Let both callers reach the shared Deferred before moving virtual time. */
						yield* Effect.yieldNow;
						yield* Effect.sync(() => expect(opens).toBe(1));
						yield* TestClock.adjust("1999 millis");
						yield* Effect.sync(() => expect(first.pollUnsafe()).toBeUndefined());
						yield* TestClock.adjust("1 millis");
						const [expired, shared] = yield* Effect.all([
							Fiber.join(first),
							Fiber.join(follower),
						]);
						const retried = yield* controller.Load(favicon);
						return { expired, shared, retried };
					}).pipe(Effect.provide(services));
				}),
			).pipe(Effect.provide(TestClock.layer())),
		);

		expect(opens).toBe(2);
		expect(result.expired).toBeUndefined();
		expect(result.shared).toBeUndefined();
		expect(result.retried?.bytes).toEqual(bytes);
	});

	it("evicts least-recently-used assets by entry count and retained bytes", async () => {
		let opens = 0;
		const bytes_by_id = new Map<string, Uint8Array>();
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const services = yield* Layer.build(
						Layer.provide(
							RichLinkAssetControllerLive,
							Layer.succeed(ArtisanClient, {
								...FixtureArtisanClientService,
								OpenAsset: (asset_id) => {
									opens += 1;
									return Effect.succeed(Stream.make(bytes_by_id.get(asset_id)!));
								},
							}),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* RichLinkAssetController;
						const favicons = Array.from(
							{ length: maximum_retained_rich_link_assets + 1 },
							(_, index) => {
								const asset_id = AssetId(index + 10);
								const bytes = new Uint8Array([index]);
								bytes_by_id.set(asset_id, bytes);
								return Favicon(asset_id, bytes.byteLength);
							},
						);
						yield* Effect.forEach(favicons, (favicon) => controller.Load(favicon));
						yield* controller.Load(favicons[0]!);

						const byte_sized =
							Math.floor(maximum_retained_rich_link_asset_bytes / 2) + 1;
						const byte_favicons = [100, 101].map((index) => {
							const asset_id = AssetId(index);
							const bytes = new Uint8Array(byte_sized);
							bytes_by_id.set(asset_id, bytes);
							return Favicon(asset_id, bytes.byteLength);
						});
						yield* Effect.forEach(byte_favicons, (favicon) => controller.Load(favicon));
						yield* controller.Load(byte_favicons[0]!);
					}).pipe(Effect.provide(services));
				}),
			),
		);

		expect(result).toBeUndefined();
		expect(opens).toBe(maximum_retained_rich_link_assets + 1 + 1 + 2 + 1);
	});
});
