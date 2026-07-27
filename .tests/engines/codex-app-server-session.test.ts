import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";
import { Cause, Effect, Exit, Fiber, Stream } from "effect";

import { CodexProcessFactoryLive, open_codex_app_server_session } from "@artisan/engines";

const fixture_path = fileURLToPath(new URL("./fixtures/fake-app-server.ts", import.meta.url));
const snowman = String.fromCodePoint(0x2603);

interface CircularValue {
	self?: CircularValue;
}

function make_session(
	options: {
		readonly diagnostic_capacity?: number;
		readonly max_frame_bytes?: number;
		readonly notification_capacity?: number;
		readonly notification_ingress_capacity?: number;
		readonly request_timeout_ms?: number;
	} = {},
) {
	return open_codex_app_server_session({
		...options,
		spawn: {
			args: [fixture_path],
			command: process.execPath,
		},
	});
}

function is_process_alive(pid: number) {
	try {
		process.kill(pid, 0);

		return true;
	} catch {
		return false;
	}
}

async function wait_for_process_exit(pid: number) {
	for (let attempt = 0; attempt < 40; attempt += 1) {
		if (!is_process_alive(pid)) {
			return;
		}

		await new Promise<void>((resolve) => setTimeout(resolve, 50));
	}
}

describe("Codex app-server session", () => {
	it("rejects an invalid frame bound before opening a process", async () => {
		await expect(
			Effect.runPromise(
				Effect.scoped(make_session({ max_frame_bytes: 0 })).pipe(
					Effect.provide(CodexProcessFactoryLive),
				),
			),
		).rejects.toMatchObject({
			_tag: "CodexAppServerConfigurationError",
			option: "max_frame_bytes",
		});
	});

	it("diagnoses and shuts down on a newline-free oversized frame", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session({ max_frame_bytes: 128 });
					const diagnostic_fiber = yield* session.Diagnostics.pipe(
						Stream.filter((diagnostic) =>
							diagnostic.message.includes("exceeded 128 bytes"),
						),
						Stream.take(1),
						Stream.runCollect,
						Effect.forkChild,
					);
					const request = yield* session
						.Request("scenario/oversizedLine", {})
						.pipe(Effect.exit);

					return {
						diagnostics: [...(yield* Fiber.join(diagnostic_fiber))],
						request,
					};
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);
		const failure = Exit.isFailure(result.request)
			? Cause.squash(result.request.cause)
			: undefined;

		expect(failure).toMatchObject({ _tag: "CodexAppServerProtocolError" });
		expect(result.diagnostics).toEqual([
			expect.objectContaining({
				level: "error",
				message: expect.stringContaining("exceeded 128 bytes"),
				source: "stdout",
			}),
		]);
	});

	it("handshakes through a split inside the snowman UTF-8 code point and sends initialized", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const initialized = yield* session.Handshake({
						client_name: "artisan-test",
						client_version: "0.3.0",
					});
					const inspected = yield* session.Request("scenario/inspect", {});

					return { initialized, inspected };
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		expect(result.initialized.result.userAgent).toBe(`fake-codex snowman: ${snowman}`);
		expect(result.inspected.result).toMatchObject({
			received: [
				{ method: "initialize" },
				{ method: "initialized" },
				{ method: "scenario/inspect" },
			],
		});
	});

	it("serializes concurrent writes and correlates out-of-order responses with unique ids", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const slow_fiber = yield* session
						.Request("slow", { value: "slow" })
						.pipe(Effect.forkChild);
					const fast_fiber = yield* session
						.Request("fast", { value: "fast" })
						.pipe(Effect.forkChild);
					const fast = yield* Fiber.join(fast_fiber);
					const slow = yield* Fiber.join(slow_fiber);
					const inspected = yield* session.Request("scenario/inspect", {});

					return { fast, inspected, slow };
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		expect(result.fast.result).toEqual({ value: "fast" });
		expect(result.slow.result).toEqual({ value: "slow" });
		expect(result.inspected.result).toMatchObject({
			received: [
				{ id: 1, method: "slow" },
				{ id: 2, method: "fast" },
				{ id: 3, method: "scenario/inspect" },
			],
		});
	});

	it("correlates a response after notifications backpressure the consumer queue", async () => {
		const response = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session({
						notification_capacity: 1,
						notification_ingress_capacity: 16,
					});

					return yield* session.Request("scenario/notificationFlood", { count: 8 });
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		expect(response.result).toEqual({ count: 8 });
	});

	it("accepts additive metadata on otherwise valid JSON-RPC envelopes", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const notification_fiber = yield* Stream.runCollect(
						session.Notifications.pipe(Stream.take(1)),
					).pipe(Effect.forkChild);
					const response = yield* session.Request("scenario/additiveNotification", {});

					return {
						notification: [...(yield* Fiber.join(notification_fiber))],
						response,
					};
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		expect(result.response.result).toEqual({ ok: true });
		expect(result.notification).toMatchObject([
			{
				method: "test/additive",
				payload: {
					emittedAtMs: 42,
					params: { value: "preserved" },
				},
			},
		]);
	});

	it("finalizes when the downstream notification queue is full and unconsumed", async () => {
		const finalized = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session({
						notification_capacity: 1,
						notification_ingress_capacity: 16,
					});

					yield* session.Request("scenario/notificationFlood", { count: 8 });
				}),
			).pipe(
				Effect.provide(CodexProcessFactoryLive),
				Effect.as(true),
				Effect.timeoutOrElse({
					duration: 3_000,
					orElse: () => Effect.succeed(false),
				}),
			),
		);

		expect(finalized).toBe(true);
	});

	it("fails the session explicitly when lossless notification ingress overflows", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session({
						notification_capacity: 1,
						notification_ingress_capacity: 2,
					});
					const diagnostic_fiber = yield* Stream.runCollect(
						session.Diagnostics.pipe(
							Stream.filter((diagnostic) => diagnostic.message.includes("ingress")),
							Stream.take(1),
						),
					).pipe(Effect.forkChild);
					const request_exit = yield* session
						.Request("scenario/notificationFlood", { count: 100 })
						.pipe(Effect.exit);

					return {
						diagnostics: yield* Fiber.join(diagnostic_fiber),
						request_exit,
					};
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		const error = Exit.isFailure(result.request_exit)
			? Cause.squash(result.request_exit.cause)
			: undefined;

		expect(error).toMatchObject({
			_tag: "CodexAppServerNotificationOverflowError",
			capacity: 2,
		});
		expect(result.diagnostics).toMatchObject([{ level: "error", source: "stdout" }]);
	});

	it("recovers after malformed JSONL and sequences every complete frame", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const diagnostics_fiber = yield* Stream.runCollect(
						session.Diagnostics.pipe(Stream.take(1)),
					).pipe(Effect.forkChild);
					const notifications_fiber = yield* Stream.runCollect(
						session.Notifications.pipe(Stream.take(2)),
					).pipe(Effect.forkChild);
					const response = yield* session.Request("scenario/frames", {});
					const diagnostics = yield* Fiber.join(diagnostics_fiber);
					const notifications = yield* Fiber.join(notifications_fiber);

					yield* session.Respond("approval-1", { answers: ["yes"] });

					return { diagnostics, notifications, response };
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		expect(result.response.result).toEqual({ ok: true });
		expect(result.diagnostics).toMatchObject([
			{ frame_sequence: 1, raw_frame_base64: "bm90IGpzb24=", source: "stdout" },
		]);
		expect(result.notifications).toMatchObject([
			{ frame_sequence: 2, method: "thread/started" },
			{ frame_sequence: 3, id: "approval-1", method: "item/tool/requestUserInput" },
		]);
	});

	it("rejects ambiguous and malformed inbound envelopes with provenance diagnostics", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const diagnostics_fiber = yield* Stream.runCollect(
						session.Diagnostics.pipe(Stream.take(6)),
					).pipe(Effect.forkChild);
					const response = yield* session.Request("scenario/invalidEnvelopes", {});

					return {
						diagnostics: yield* Fiber.join(diagnostics_fiber),
						response,
					};
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		expect(result.response.result).toEqual({ ok: true });
		expect(result.diagnostics).toHaveLength(6);
		expect(result.diagnostics.map((diagnostic) => diagnostic.frame_sequence)).toEqual([
			1, 2, 3, 4, 5, 6,
		]);
		expect(result.diagnostics.every((diagnostic) => diagnostic.raw_frame_base64)).toBe(true);
	});

	it("lets a buffered final response win before process completion fails pending requests", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const diagnostics_fiber = yield* Stream.runCollect(session.Diagnostics).pipe(
						Effect.forkChild,
					);
					const response = yield* session.Request("scenario/respondThenExit", {}, 2_000);

					return {
						diagnostics: yield* Fiber.join(diagnostics_fiber),
						response,
					};
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		expect(result.response.result).toEqual({ final: true });
		expect(result.diagnostics).toMatchObject([
			{ message: "Codex app-server closed with code 0 and signal null", source: "process" },
		]);
	});

	it("removes an interrupted request so its late response becomes a diagnostic", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const diagnostic_fiber = yield* Stream.runCollect(
						session.Diagnostics.pipe(
							Stream.filter((diagnostic) => diagnostic.source === "stdout"),
							Stream.take(1),
						),
					).pipe(Effect.forkChild);
					const request_fiber = yield* session
						.Request("scenario/late", { delay_ms: 80 })
						.pipe(Effect.forkChild);

					yield* Effect.sleep(10);
					yield* Fiber.interrupt(request_fiber);

					const diagnostics = yield* Fiber.join(diagnostic_fiber);
					const inspected = yield* session.Request("scenario/inspect", {});

					return { diagnostics, inspected };
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		expect(result.diagnostics).toMatchObject([
			{
				frame_sequence: 1,
				message: "Received an uncorrelated response for request 1",
				source: "stdout",
			},
		]);
		expect(result.inspected.result).toMatchObject({
			received: [
				{ id: 1, method: "scenario/late" },
				{ id: 2, method: "scenario/inspect" },
			],
		});
	});

	it("rejects circular and bigint params before registering or writing a request", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const circular: CircularValue = {};

					circular.self = circular;

					const circular_exit = yield* session
						.Request("invalid/circular", circular)
						.pipe(Effect.exit);
					const bigint_exit = yield* session
						.Request("invalid/bigint", { value: 1n })
						.pipe(Effect.exit);
					const inspected = yield* session.Request("scenario/inspect", {});

					return { bigint_exit, circular_exit, inspected };
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		const circular_error = Exit.isFailure(result.circular_exit)
			? Cause.squash(result.circular_exit.cause)
			: undefined;
		const bigint_error = Exit.isFailure(result.bigint_exit)
			? Cause.squash(result.bigint_exit.cause)
			: undefined;

		expect(circular_error).toMatchObject({
			_tag: "CodexAppServerSerializationError",
			operation: "request",
		});
		expect(bigint_error).toMatchObject({
			_tag: "CodexAppServerSerializationError",
			operation: "request",
		});
		expect(result.inspected.result).toMatchObject({
			received: [{ id: 1, method: "scenario/inspect" }],
		});
	});

	it("drains stderr flood without blocking a correlated request", async () => {
		const response = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session({ diagnostic_capacity: 2 });

					return yield* session.Request("scenario/stderr", {}, 2_000);
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		expect(response.result).toEqual({ ok: true });
	});

	it("fails a timeout once and continues after the late response", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const timed_out = yield* session
						.Request("scenario/late", { delay_ms: 60 }, 10)
						.pipe(Effect.exit);

					yield* Effect.sleep(80);

					return { inspected: yield* session.Request("scenario/inspect", {}), timed_out };
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		expect(Exit.isFailure(result.timed_out)).toBe(true);
		expect(result.inspected.result).toMatchObject({
			received: [
				{ id: 1, method: "scenario/late" },
				{ id: 2, method: "scenario/inspect" },
			],
		});
	});

	it("fails all pending requests when the child crashes", async () => {
		const result = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const crash_fiber = yield* session
						.Request("scenario/crash", {})
						.pipe(Effect.forkChild);
					const pending_fiber = yield* session
						.Request("slow", { value: "lost" })
						.pipe(Effect.forkChild);

					return {
						crash: yield* Fiber.await(crash_fiber),
						pending: yield* Fiber.await(pending_fiber),
					};
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		expect(Exit.isFailure(result.crash)).toBe(true);
		expect(Exit.isFailure(result.pending)).toBe(true);
	});

	it("closes the child and grandchild process tree without leaving orphans", async () => {
		const pids = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const response = yield* session.Request("scenario/processTree", {});

					yield* session.Close;
					yield* session.Close;

					return response.result as {
						readonly grandchild_pid: number;
						readonly pid: number;
					};
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		await Promise.all([
			wait_for_process_exit(pids.pid),
			wait_for_process_exit(pids.grandchild_pid),
		]);

		expect(is_process_alive(pids.pid)).toBe(false);
		expect(is_process_alive(pids.grandchild_pid)).toBe(false);
	});

	it("releases the child when the owning scope closes", async () => {
		const pid = await Effect.runPromise(
			Effect.scoped(
				Effect.gen(function* () {
					const session = yield* make_session();
					const response = yield* session.Request("scenario/pid", {});

					return (response.result as { readonly pid: number }).pid;
				}).pipe(Effect.provide(CodexProcessFactoryLive)),
			),
		);

		await wait_for_process_exit(pid);

		expect(is_process_alive(pid)).toBe(false);
	});
});
