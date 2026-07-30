import { Effect, Fiber } from "effect";
import { describe, expect, it } from "vitest";

import type { ThreadListQueryEnvelope } from "@artisan/protocol";
import { client_error } from "../../modules/transport/src/internal/client-common";
import { make_client_request_coordinator } from "../../modules/transport/src/internal/client-request-coordinator";

const request: ThreadListQueryEnvelope = {
	kind: "thread.list.query",
	message_id: "message_send_failure",
	origin: "frontend",
	payload: {},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-06-19T00:00:00.000Z",
};

describe("client request coordinator", () => {
	it("fails a request promptly and unregisters it when its initial send fails", async () => {
		const failure = client_error(
			"connection",
			"The transport send failed.",
			new Error("send failed"),
			true,
		);
		const program = Effect.gen(function* () {
			const coordinator = yield* make_client_request_coordinator(4, () =>
				Effect.fail(failure),
			);
			const first = yield* Effect.exit(coordinator.Request(request));
			const second = yield* Effect.exit(coordinator.Request(request));

			return { first, second };
		});
		const result = await Effect.runPromise(program);

		expect(result.first._tag).toBe("Failure");
		expect(result.second._tag).toBe("Failure");
		if (result.first._tag === "Failure" && result.second._tag === "Failure") {
			expect(result.first.cause.toString()).toContain("The transport send failed.");
			expect(result.second.cause.toString()).toContain("The transport send failed.");
			expect(result.second.cause.toString()).not.toContain("request id is already active");
		}
	});

	it("fails queued and later requests once the connection parks, and carries them again on resume", async () => {
		const parked = client_error(
			"connection",
			"Forge is unreachable.",
			new Error("reconnect budget spent"),
			true,
		);
		const program = Effect.gen(function* () {
			const sent: Array<string> = [];
			const coordinator = yield* make_client_request_coordinator(4, (envelope) =>
				Effect.sync(() => {
					sent.push(envelope.message_id);
				}),
			);

			/** A request with no session in place waits to be retried on reconnect. */
			const queued = yield* Effect.forkScoped(coordinator.Request(request));
			yield* Effect.yieldNow;
			yield* coordinator.Park(parked);
			const queued_exit = yield* Effect.exit(Fiber.join(queued));
			const while_parked = yield* Effect.exit(
				coordinator.Request({ ...request, message_id: "message_while_parked" }),
			);

			yield* coordinator.Resume;
			yield* Effect.forkScoped(
				coordinator.Request({ ...request, message_id: "message_after_resume" }),
			);
			yield* Effect.yieldNow;

			return { queued_exit, sent, while_parked };
		}).pipe(Effect.scoped);
		const result = await Effect.runPromise(program);

		expect(result.queued_exit._tag).toBe("Failure");
		expect(result.while_parked._tag).toBe("Failure");
		if (result.while_parked._tag === "Failure") {
			expect(result.while_parked.cause.toString()).toContain("Forge is unreachable.");
		}
		/** A resumed coordinator accepts and sends again rather than staying poisoned. */
		expect(result.sent).toContain("message_after_resume");
	});
});
