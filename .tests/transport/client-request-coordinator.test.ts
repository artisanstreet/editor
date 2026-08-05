import { Effect, Fiber } from "effect";
import { describe, expect, it } from "vitest";

import type { ThreadListQueryEnvelope } from "@artisan/protocol";
import {
	client_error,
	RequestDelivered,
	RequestHeld,
} from "../../modules/transport/src/internal/client-common";
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

const unreachable = client_error(
	"connection",
	"Forge is unreachable.",
	new Error("reconnect budget spent"),
	true,
);

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
		const program = Effect.gen(function* () {
			const attempted: Array<string> = [];
			const coordinator = yield* make_client_request_coordinator(4, (envelope) =>
				Effect.sync(() => {
					attempted.push(envelope.message_id);

					return RequestHeld;
				}),
			);

			/** A request with no session in place waits to be retried on reconnect. */
			const queued = yield* Effect.forkScoped(coordinator.Request(request));
			yield* Effect.yieldNow;
			yield* coordinator.Park(unreachable);
			const queued_exit = yield* Effect.exit(Fiber.join(queued));
			const while_parked = yield* Effect.exit(
				coordinator.Request({ ...request, message_id: "message_while_parked" }),
			);

			yield* coordinator.Resume;
			yield* Effect.forkScoped(
				coordinator.Request({ ...request, message_id: "message_after_resume" }),
			);
			yield* Effect.yieldNow;

			return { attempted, queued_exit, while_parked };
		}).pipe(Effect.scoped);
		const result = await Effect.runPromise(program);

		expect(result.queued_exit._tag).toBe("Failure");
		expect(result.while_parked._tag).toBe("Failure");
		if (result.while_parked._tag === "Failure") {
			expect(result.while_parked.cause.toString()).toContain("Forge is unreachable.");
		}
		/** A resumed coordinator accepts and sends again rather than staying poisoned. */
		expect(result.attempted).toContain("message_after_resume");
	});

	it("releases the slot of a request interrupted before any session carried it", async () => {
		const program = Effect.gen(function* () {
			const attempted: Array<string> = [];
			const coordinator = yield* make_client_request_coordinator(1, (envelope) =>
				Effect.sync(() => {
					attempted.push(envelope.message_id);

					return RequestHeld;
				}),
			);

			/**
			 * Nothing carried this envelope, so no result can ever correlate to
			 * it and its capacity belongs to the next caller immediately.
			 */
			const queued = yield* Effect.forkScoped(coordinator.Request(request));
			yield* Effect.yieldNow;
			yield* Fiber.interrupt(queued);

			yield* Effect.forkScoped(
				coordinator.Request({ ...request, message_id: "message_after_interrupt" }),
			);
			yield* Effect.yieldNow;

			return attempted;
		}).pipe(Effect.scoped);

		expect(await Effect.runPromise(program)).toContain("message_after_interrupt");
	});

	it("reclaims a request interrupted mid-send once a new session resets the connection", async () => {
		const program = Effect.gen(function* () {
			const attempted: Array<string> = [];
			const coordinator = yield* make_client_request_coordinator(1, (envelope) =>
				Effect.sync(() => {
					attempted.push(envelope.message_id);
				}).pipe(Effect.andThen(Effect.never)),
			);

			/** Interrupted while the envelope is still on its way to the wire. */
			const sending = yield* Effect.forkScoped(coordinator.Request(request));
			yield* Effect.yieldNow;
			yield* Fiber.interrupt(sending);

			/** Delivery was unknowable, so the correlation still guards its slot. */
			const while_remembered = yield* Effect.exit(
				coordinator.Request({ ...request, message_id: "message_while_remembered" }),
			);

			/** A fresh session can no longer answer it, and the slot comes back. */
			yield* coordinator.ResetConnection;
			yield* Effect.forkScoped(
				coordinator.Request({ ...request, message_id: "message_after_reset" }),
			);
			yield* Effect.yieldNow;

			return { attempted, while_remembered };
		}).pipe(Effect.scoped);
		const result = await Effect.runPromise(program);

		expect(result.while_remembered._tag).toBe("Failure");
		if (result.while_remembered._tag === "Failure") {
			expect(result.while_remembered.cause.toString()).toContain(
				"The pending request limit was reached.",
			);
		}
		expect(result.attempted).toContain("message_after_reset");
	});

	it("keeps capacity when an interrupt lands after the request already settled", async () => {
		const program = Effect.gen(function* () {
			const attempted: Array<string> = [];
			const coordinator = yield* make_client_request_coordinator(1, (envelope) =>
				Effect.sync(() => {
					attempted.push(envelope.message_id);

					return RequestDelivered;
				}),
			);

			const settled = yield* Effect.forkScoped(coordinator.Request(request));
			yield* Effect.yieldNow;
			/** The answer lands, then the caller goes away before it is resumed. */
			yield* coordinator.Reject(request.message_id, {
				code: "projection.unavailable",
				message: "The thread list could not be read.",
				retryable: true,
			});
			yield* Fiber.interrupt(settled);

			yield* Effect.forkScoped(
				coordinator.Request({ ...request, message_id: "message_after_settled" }),
			);
			yield* Effect.yieldNow;

			return attempted;
		}).pipe(Effect.scoped);

		expect(await Effect.runPromise(program)).toContain("message_after_settled");
	});

	it("drops correlations abandoned under a session the connection has parked", async () => {
		const program = Effect.gen(function* () {
			const attempted: Array<string> = [];
			const coordinator = yield* make_client_request_coordinator(1, (envelope) =>
				Effect.sync(() => {
					attempted.push(envelope.message_id);

					return RequestDelivered;
				}),
			);

			const abandoned = yield* Effect.forkScoped(coordinator.Request(request));
			yield* Effect.yieldNow;
			yield* Fiber.interrupt(abandoned);

			/** The parked session owed that answer; nothing else will deliver it. */
			yield* coordinator.Park(unreachable);
			yield* coordinator.Resume;
			yield* Effect.forkScoped(
				coordinator.Request({ ...request, message_id: "message_after_park" }),
			);
			yield* Effect.yieldNow;

			return attempted;
		}).pipe(Effect.scoped);

		expect(await Effect.runPromise(program)).toContain("message_after_park");
	});
});
