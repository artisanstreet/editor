import { describe, expect, it } from "vitest";
import { Effect, Layer, Redacted } from "effect";

import {
	EmptySecretStoreLive,
	make_oauth_layer,
	OAuth,
	OAuthAdapter,
	secret_reference,
	SecretStore,
} from "@artisan/backend";

describe("Marketplace secret and OAuth boundaries", () => {
	it("redacts resolved secrets and default-denies unavailable storage", async () => {
		const reference = await Effect.runPromise(secret_reference("marketplace/github"));
		const denied = await Effect.runPromise(
			Effect.gen(function* () {
				return yield* (yield* SecretStore).Get(reference);
			}).pipe(Effect.exit, Effect.provide(EmptySecretStoreLive)),
		);
		expect(denied._tag).toBe("Failure");
		const secret = await Effect.runPromise(
			Effect.gen(function* () {
				return yield* (yield* SecretStore).Get(reference);
			}).pipe(
				Effect.provide(
					Layer.succeed(SecretStore, {
						Get: () => Effect.succeed(Redacted.make("never-log-me")),
					}),
				),
			),
		);
		expect(String(secret)).not.toContain("never-log-me");
	});

	it("does not begin OAuth during layer acquisition", async () => {
		let begins = 0;
		const adapter = Layer.succeed(OAuthAdapter, {
			Begin: () =>
				Effect.sync(() => {
					begins += 1;
					return { authorization_url: "https://example.test/login", state: "state" };
				}),
			Complete: () => Effect.succeed({ capability_id: "mcp", state: "active" as const }),
			Refresh: () => Effect.succeed({ capability_id: "mcp", state: "active" as const }),
			Revoke: () => Effect.void,
			Status: () => Effect.succeed({ capability_id: "mcp", state: "absent" as const }),
		});
		await Effect.runPromise(
			Effect.void.pipe(Effect.provide(make_oauth_layer.pipe(Layer.provide(adapter)))),
		);
		expect(begins).toBe(0);
		await Effect.runPromise(
			Effect.gen(function* () {
				return yield* (yield* OAuth).Begin({
					authorization_url: "https://example.test/login",
					capability_id: "mcp",
					scopes: [],
				});
			}).pipe(Effect.provide(make_oauth_layer.pipe(Layer.provide(adapter)))),
		);
		expect(begins).toBe(1);
	});
});
