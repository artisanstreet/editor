import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const route_path = "modules/frontend/src/routes/components/thread-route.svelte";
const composer_path = "modules/frontend/src/routes/components/thread-composer.svelte";

describe("thread message receipt performance", () => {
	it("returns at the durable receipt and leaves steering settlement to the subscription", () => {
		const route = readFileSync(route_path, "utf8");
		const send_message = route.slice(
			route.indexOf("const SendMessage"),
			route.indexOf("const UpdateSessionPolicy"),
		);

		expect(send_message).toContain("client.Command(");
		expect(send_message).toContain(
			"RefreshInteractionContext.pipe(Effect.forkIn(thread_scope), Effect.asVoid)",
		);
		/**
		 * Settlement is the engine taking up the steer, not Artisan echoing the
		 * user's own message back — that lands in milliseconds against a local
		 * Forge and left the "Steering" label below perceptibility.
		 */
		expect(send_message).toContain(
			"steering_settlement: AwaitSteeringAcknowledged(receipt.command_id)",
		);
		expect(send_message).not.toContain("GetConversation");
		expect(send_message).not.toContain("AwaitAcceptedProjection");
		expect(send_message).not.toContain("Effect.retry");
		expect(send_message).not.toContain("Effect.sleep");
	});

	it("settles shared receipt waiters from every authoritative snapshot path", () => {
		const route = readFileSync(route_path, "utf8");

		expect(route).toContain("const accepted_projection_waiters = yield* Ref.make(");
		expect(route).toContain("listeners: existing.listeners + 1");
		expect(route).toContain(
			"Effect.ensuring(ReleaseAcceptedProjectionWaiter(key, claimed.deferred))",
		);
		/** Each waiter carries what it waits for, so both settlements share one path. */
		expect(route).toContain("if (!waiter.Satisfied(candidate)) continue;");
		expect(route).toContain("yield* ResolveAcceptedProjectionWaiters(next);");
		expect(route).toContain("yield* ResolveAcceptedProjectionWaiters(snapshot);");
	});

	it("releases the submit gate before the projection-driven steering lip settles", () => {
		const composer = readFileSync(composer_path, "utf8");

		expect(composer).toContain("Effect.forkScoped");
		expect(composer).toContain("yield* steering_echo.pipe(");
		expect(composer).toContain(
			"Effect.suspend(() => ReleaseSubmission(retain_pending_lip, pending_lip_generation))",
		);
		expect(composer.indexOf("yield* submit_gate.Release;")).toBeLessThan(
			composer.indexOf("Effect.ensuring(steering.Settle(steer_generation))"),
		);
	});
});
