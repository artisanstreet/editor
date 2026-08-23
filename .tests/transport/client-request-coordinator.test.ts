import { Deferred, Effect, Fiber } from "effect";
import { describe, expect, it } from "vitest";

import type { ThreadListQueryEnvelope, ThreadListQueryResultEnvelope } from "@artisan/protocol";
import {
	client_error,
	RequestDelivered,
	RequestHeld,
} from "../../modules/transport/src/internal/client-common";
import {
	make_client_request_coordinator,
	request_deadline_ms_for,
} from "../../modules/transport/src/internal/client-request-coordinator";

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

const empty_result: ThreadListQueryResultEnvelope = {
	correlation_id: request.message_id,
	kind: "thread.list.query.result",
	message_id: "message_late_result",
	origin: "backend",
	payload: { journal_sequence: 0, threads: [] },
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-06-19T00:00:01.000Z",
};

describe("client request coordinator", () => {
	it("assigns short local deadlines without truncating external or human operations", () => {
		expect(request_deadline_ms_for("thread.list.query")).toBe(2_000);
		expect(request_deadline_ms_for("workspace.file.read.query")).toBe(15_000);
		expect(request_deadline_ms_for("engine.install.request")).toBe(120_000);
		expect(request_deadline_ms_for("project.directory.pick")).toBe(1_800_000);
	});

	/**
	 * Abandoning a read costs a retry; abandoning a command decides nothing,
	 * because Forge may accept it anyway. A message send carrying attachment
	 * bytes and a durable journal write must not be held to the budget a local
	 * projection read gets — that is what reported landed sends as failures.
	 */
	it("does not hold a durable command to a local read's budget", () => {
		expect(request_deadline_ms_for("command")).toBe(30_000);
		expect(request_deadline_ms_for("command")).toBeGreaterThan(
			request_deadline_ms_for("thread.list.query"),
		);
	});

	/**
	 * The backend spawns an external CLI to answer these and bounds itself at
	 * 15 seconds. Giving up first cannot report anything true — it abandons a
	 * request that is still running and calls it a deadline miss, which is what
	 * made a healthy Claude account read as a timed-out usage lookup. The
	 * client's deadline has to outlast the server's budget so the server's own
	 * bounded failure is the one that surfaces.
	 */
	it("outlasts the backend's own budget for provider-boundary reads", () => {
		/** Mirrors `usage_timeout` in the engine usage handler and the Codex probe. */
		const backend_provider_budget_ms = 15_000;

		for (const kind of ["engine.usage.query", "model_behaviour.query"] as const) {
			expect(request_deadline_ms_for(kind)).toBeGreaterThan(backend_provider_budget_ms);
			expect(request_deadline_ms_for(kind)).toBeGreaterThan(
				request_deadline_ms_for("thread.list.query"),
			);
		}
	});

	it("fails a held request at its deadline and immediately releases its slot", async () => {
		const program = Effect.gen(function* () {
			const attempted: Array<string> = [];
			const coordinator = yield* make_client_request_coordinator(
				(envelope) =>
					Effect.sync(() => {
						attempted.push(envelope.message_id);

						return RequestHeld;
					}),
				() => 10,
			);
			const first = yield* coordinator.Request(request).pipe(Effect.flip);
			const second = yield* coordinator.Request(request).pipe(Effect.flip);

			return { attempted, first, second };
		});
		const result = await Effect.runPromise(program);

		expect(result.attempted).toEqual([request.message_id, request.message_id]);
		expect(result.first).toMatchObject({
			code: "connection",
			protocol_code: "request.timeout",
			retryable: true,
		});
		expect(result.second).toMatchObject({ protocol_code: "request.timeout" });
	});

	it("discards a delivered request's late result and then releases its correlation", async () => {
		const program = Effect.gen(function* () {
			let attempts = 0;
			const coordinator = yield* make_client_request_coordinator(
				() =>
					Effect.sync(() => {
						attempts += 1;

						return attempts === 1 ? RequestDelivered("connection_first") : RequestHeld;
					}),
				() => 10,
			);
			const first = yield* coordinator.Request(request).pipe(Effect.flip);

			yield* coordinator.Resolve(empty_result);

			const second = yield* coordinator.Request(request).pipe(Effect.flip);

			return { attempts, first, second };
		});
		const result = await Effect.runPromise(program);

		expect(result.attempts).toBe(2);
		expect(result.first).toMatchObject({ protocol_code: "request.timeout" });
		expect(result.second).toMatchObject({ protocol_code: "request.timeout" });
	});

	it("retains resend ownership when its caller leaves during reconnect", async () => {
		const program = Effect.gen(function* () {
			const retry_started = yield* Deferred.make<void>();
			const release_retry = yield* Deferred.make<void>();
			let attempts = 0;
			const coordinator = yield* make_client_request_coordinator(() =>
				Effect.gen(function* () {
					attempts += 1;
					if (attempts !== 2) return RequestHeld;
					yield* Deferred.succeed(retry_started, undefined);
					yield* Deferred.await(release_retry);
					return RequestDelivered("connection_retry");
				}),
			);
			const waiting = yield* coordinator.Request(request).pipe(Effect.forkScoped);
			yield* Effect.yieldNow;
			const retry = yield* coordinator.Retry.pipe(Effect.forkScoped);
			yield* Deferred.await(retry_started);
			yield* Fiber.interrupt(waiting);
			yield* Deferred.succeed(release_retry, undefined);
			yield* Fiber.join(retry);
			yield* coordinator.Resolve(empty_result);

			const after_late_result = yield* coordinator.Request(request).pipe(Effect.forkScoped);
			yield* Effect.yieldNow;
			yield* Fiber.interrupt(after_late_result);
			return attempts;
		}).pipe(Effect.scoped);

		expect(await Effect.runPromise(program)).toBe(3);
	});

	it("settles once when a result arrives during reconnect resend", async () => {
		const program = Effect.gen(function* () {
			const retry_started = yield* Deferred.make<void>();
			const release_retry = yield* Deferred.make<void>();
			let attempts = 0;
			const coordinator = yield* make_client_request_coordinator(() =>
				Effect.gen(function* () {
					attempts += 1;
					if (attempts !== 2) return RequestHeld;
					yield* Deferred.succeed(retry_started, undefined);
					yield* Deferred.await(release_retry);
					return RequestDelivered("connection_retry");
				}),
			);
			const waiting = yield* coordinator.Request(request).pipe(Effect.forkScoped);
			yield* Effect.yieldNow;
			const retry = yield* coordinator.Retry.pipe(Effect.forkScoped);
			yield* Deferred.await(retry_started);

			yield* coordinator.Resolve(empty_result);
			const settled = yield* Fiber.join(waiting);
			yield* Deferred.succeed(release_retry, undefined);
			yield* Fiber.join(retry);
			yield* coordinator.Resolve({
				...empty_result,
				message_id: "message_duplicate_retry_result",
			});

			const after_duplicate = yield* coordinator.Request(request).pipe(Effect.forkScoped);
			yield* Effect.yieldNow;
			yield* Fiber.interrupt(after_duplicate);
			return { attempts, settled };
		}).pipe(Effect.scoped);
		const result = await Effect.runPromise(program);

		expect(result.attempts).toBe(3);
		expect(result.settled).toEqual(empty_result);
	});

	it("fails a request promptly and unregisters it when its initial send fails", async () => {
		const failure = client_error(
			"connection",
			"The transport send failed.",
			new Error("send failed"),
			true,
		);
		const program = Effect.gen(function* () {
			const coordinator = yield* make_client_request_coordinator(() => Effect.fail(failure));
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
			const coordinator = yield* make_client_request_coordinator((envelope) =>
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
			const coordinator = yield* make_client_request_coordinator((envelope) =>
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
			const coordinator = yield* make_client_request_coordinator((envelope) =>
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
			expect(result.while_remembered.cause.toString()).toContain("request deadline exceeded");
		}
		expect(result.attempted).toContain("message_after_reset");
	});

	it("keeps capacity when an interrupt lands after the request already settled", async () => {
		const program = Effect.gen(function* () {
			const attempted: Array<string> = [];
			const coordinator = yield* make_client_request_coordinator((envelope) =>
				Effect.sync(() => {
					attempted.push(envelope.message_id);

					return RequestDelivered("connection_settled");
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
			const coordinator = yield* make_client_request_coordinator((envelope) =>
				Effect.sync(() => {
					attempted.push(envelope.message_id);

					return RequestDelivered("connection_abandoned");
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

	it("retires the exact session behind an unanswered query", async () => {
		const retired: Array<string | undefined> = [];
		const program = Effect.gen(function* () {
			const coordinator = yield* make_client_request_coordinator(
				() => Effect.succeed(RequestDelivered("connection_zombie")),
				() => 10,
				(delivery) =>
					Effect.sync(() => {
						retired.push(
							delivery._tag === "Delivered" ? delivery.connection_id : undefined,
						);
					}),
			);
			return yield* coordinator.Request(request).pipe(Effect.flip);
		});

		const error = await Effect.runPromise(program);

		expect(retired).toEqual(["connection_zombie"]);
		expect(error).toMatchObject({ protocol_code: "request.timeout", retryable: true });
	});
});
