import { Deferred, Effect, Exit, Fiber, Layer } from "effect";
import { describe, expect, it } from "vitest";

import type { RichLinkResolution } from "@artisan/protocol";
import { ArtisanClient, ArtisanClientError } from "@artisan/transport/client";
import {
	RichLinkMetadataController,
	RichLinkMetadataControllerLive,
} from "../../modules/frontend/src/lib/components/markdown/rich-link-metadata-controller";
import { FixtureArtisanClientService } from "../../modules/frontend/src/lib/runtime/fixtures/client";

const Resolution = (
	expires_at: string,
	overrides: Partial<RichLinkResolution> = {},
): RichLinkResolution => ({
	cache: { expires_at, status: "miss" },
	fetched_at: "2026-08-15T12:00:00.000Z",
	final_url: "https://example.com/",
	page_name: "Example",
	requested_url: "https://example.com/",
	site_name: "Example",
	...overrides,
});

describe("rich link metadata controller", () => {
	it("shares an admitted canonical-url flight across interrupted and remounted callers", async () => {
		let calls = 0;
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const started = yield* Deferred.make<void>();
					const release = yield* Deferred.make<void>();
					const services = yield* Layer.build(
						RichLinkMetadataControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									ResolveRichLink: () =>
										Effect.gen(function* () {
											calls += 1;
											yield* Deferred.succeed(started, undefined);
											yield* Deferred.await(release);
											return Resolution("2099-01-01T00:00:00.000Z");
										}),
								}),
							),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* RichLinkMetadataController;
						const starter = yield* controller
							.Load("https://example.com/#overview")
							.pipe(Effect.forkScoped);
						yield* Deferred.await(started);
						const follower = yield* controller
							.Load("https://example.com/")
							.pipe(Effect.forkScoped);
						yield* Fiber.interrupt(starter);
						yield* Deferred.succeed(release, undefined);
						const loaded = yield* Fiber.join(follower);
						const remounted = yield* controller.Load("https://example.com/");
						return { loaded, remounted };
					}).pipe(Effect.provide(services));
				}),
			),
		);
		expect(calls).toBe(1);
		expect(result.loaded).toEqual(result.remounted);
	});

	it("expires failed and backend-expired flights before a later retry", async () => {
		let calls = 0;
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const services = yield* Layer.build(
						RichLinkMetadataControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									ResolveRichLink: () => {
										calls += 1;
										if (calls === 1)
											return Effect.fail(
												new ArtisanClientError({
													cause: undefined,
													code: "connection",
													message: "offline",
													protocol_code: "offline",
													retryable: true,
												}),
											);
										return Effect.succeed(
											Resolution(
												calls === 2
													? "2000-01-01T00:00:00.000Z"
													: "2099-01-01T00:00:00.000Z",
											),
										);
									},
								}),
							),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* RichLinkMetadataController;
						expect(
							Exit.isFailure(
								yield* controller.Load("https://example.com").pipe(Effect.exit),
							),
						).toBe(true);
						yield* controller.Load("https://example.com");
						yield* controller.Load("https://example.com");
					}).pipe(Effect.provide(services));
				}),
			),
		);
		expect(calls).toBe(3);
	});

	it("retains only the 64 most recently used canonical URLs", async () => {
		let calls = 0;
		await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const services = yield* Layer.build(
						RichLinkMetadataControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									ResolveRichLink: ({ url }) => {
										calls += 1;
										return Effect.succeed(
											Resolution("2099-01-01T00:00:00.000Z", {
												final_url: url,
												requested_url: url,
											}),
										);
									},
								}),
							),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* RichLinkMetadataController;
						for (let index = 0; index < 65; index += 1) {
							yield* controller.Load(`https://example.com/${index}`);
						}
						yield* controller.Load("https://example.com/64#cached");
						yield* controller.Load("https://example.com/0");
					}).pipe(Effect.provide(services));
				}),
			),
		);
		expect(calls).toBe(66);
	});

	it("does not let an older backend result replace newer retained metadata", async () => {
		let calls = 0;
		const loaded = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const services = yield* Layer.build(
						RichLinkMetadataControllerLive.pipe(
							Layer.provide(
								Layer.succeed(ArtisanClient, {
									...FixtureArtisanClientService,
									ResolveRichLink: () => {
										calls += 1;
										return Effect.succeed(
											calls === 1
												? Resolution("2000-01-01T00:00:00.000Z", {
														fetched_at: "2026-08-15T12:00:00.000Z",
														page_name: "Newer",
													})
												: Resolution("2099-01-01T00:00:00.000Z", {
														fetched_at: "2026-08-15T11:00:00.000Z",
														page_name: "Older",
													}),
										);
									},
								}),
							),
						),
					);
					return yield* Effect.gen(function* () {
						const controller = yield* RichLinkMetadataController;
						yield* controller.Load("https://example.com");
						const second = yield* controller.Load("https://example.com");
						yield* controller.Load("https://example.com");
						return second;
					}).pipe(Effect.provide(services));
				}),
			),
		);
		expect(loaded.page_name).toBe("Newer");
		expect(calls).toBe(3);
	});
});
