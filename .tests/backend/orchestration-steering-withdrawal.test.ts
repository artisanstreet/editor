import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { AgentOrchestrator, make_backend_runtime } from "@artisan/backend";
import type { Engine, EngineCommand, EngineRun } from "@artisan/engines";
import { ConversationReadModel } from "../../modules/backend/src/conversation";
import type { AuthoritativeCommandEnvelope } from "../../modules/backend/src/persistence/orchestration/message-command";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-steering-withdrawal-"));
	temporary_directories.push(directory);
	return join(directory, "artisan.db");
}

function command<const Payload extends AuthoritativeCommandEnvelope["payload"]>(
	message_id: string,
	thread_id: string,
	payload: Payload,
): Omit<AuthoritativeCommandEnvelope, "payload"> & { readonly payload: Payload } {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-08-19T20:00:00.000Z",
		thread_id,
	};
}

/** A steer-capable engine whose steer delivery blocks until the gate releases. */
function make_engine(id: string, steer_gate: Deferred.Deferred<"released">) {
	const commands: Array<EngineCommand> = [];
	const opened: Array<string> = [];
	const capabilities = Object.fromEntries(
		[
			"approval",
			"auth",
			"cancel",
			"close",
			"events",
			"model_selection",
			"native_tools",
			"probe",
			"question",
			"raw_frames",
			"resume",
			"start",
			"steer",
			"subagents",
		].map((name) => [name, { state: "supported" as const }]),
	) as Engine["Descriptor"]["capabilities"];
	const engine: Engine = {
		Descriptor: { capabilities, display_name: id, id, transport: "test" },
		Open: (input) =>
			Effect.gen(function* () {
				opened.push(input.artisan_run_id);
				const run_closed = yield* Deferred.make<"closed">();
				yield* Effect.addFinalizer(() => Deferred.succeed(run_closed, "closed"));
				return {
					artisan_run_id: input.artisan_run_id,
					Closed: Deferred.await(run_closed),
					Events: Stream.fromEffect(Deferred.await(run_closed)).pipe(Stream.drain),
					native_thread_id: `native:${input.artisan_run_id}`,
					resume_token: { native_thread_id: `native:${input.artisan_run_id}` },
					Send: (sent) =>
						Effect.gen(function* () {
							commands.push(sent);
							if (sent._tag === "steer") yield* Deferred.await(steer_gate);
						}),
				} satisfies EngineRun;
			}),
		Probe: () => Effect.die("not used"),
	};
	return { commands, engine, opened };
}

async function wait_for(predicate: () => boolean | Promise<boolean>) {
	const deadline = Date.now() + 2_000;
	while (!(await predicate())) {
		if (Date.now() >= deadline) throw new Error("timed out waiting for orchestration");
		await new Promise((resolve) => setTimeout(resolve, 5));
	}
}

async function setup(engines: ReadonlyArray<Engine>, thread_id: string) {
	const runtime = make_backend_runtime({
		database_path: await make_database_path(),
		engines,
		migrations_path,
	});
	const journal = await runtime.runPromise(JournalStore);
	const orchestrator = await runtime.runPromise(AgentOrchestrator);
	await runtime.runPromise(
		journal.AcceptThreadCreate(
			command("create", thread_id, { title: "Withdrawal", type: "thread.create" }),
		),
	);
	return { journal, orchestrator, runtime, thread_id };
}

async function send(context: Awaited<ReturnType<typeof setup>>, message_id: string, text: string) {
	return await context.runtime.runPromise(
		context.orchestrator.Handle(
			command(message_id, context.thread_id, {
				engine_id: "capable",
				text,
				type: "thread.send_message",
				working_directory: "C:/work",
			}),
		),
	);
}

async function steer_source_references(context: Awaited<ReturnType<typeof setup>>) {
	const snapshot = await context.runtime.runPromise(
		Effect.gen(function* () {
			const conversations = yield* ConversationReadModel;
			return yield* conversations.ReadSnapshot(context.thread_id);
		}),
	);
	if (snapshot.status !== "available") throw new Error("Expected a conversation snapshot");
	return snapshot.snapshot.items
		.filter((item) => item.type === "user_message")
		.flatMap((item) => item.source_refs.map((reference) => reference.reference));
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("queued steering withdrawal", () => {
	/**
	 * A second steer accepted while the first still blocks delivery sits
	 * `pending` behind the per-thread dispatch fence — the exact state the
	 * withdrawal recalls. A withdrawn steer must never reach the engine and
	 * never project a message, even after the fence frees.
	 */
	it("withdraws a pending steer before dispatch and never projects it", async () => {
		const steer_gate = await Effect.runPromise(Deferred.make<"released">());
		const capable = make_engine("capable", steer_gate);
		const context = await setup([capable.engine], "thread_withdraw_pending");
		try {
			await send(context, "start", "Summarize the current implementation");
			await wait_for(() => capable.opened.length === 1);
			await send(context, "blocked_follow_up", "Also include verification evidence");
			await wait_for(() => capable.commands.length === 1);
			await send(context, "queued_follow_up", "Actually just run the focused tests");

			const withdrawal = await context.runtime.runPromise(
				context.orchestrator.Handle(
					command("withdraw_queued", context.thread_id, {
						command_id: "queued_follow_up",
						type: "thread.withdraw_message",
					}),
				),
			);
			expect(withdrawal.status).toBe("accepted");

			await Effect.runPromise(Deferred.succeed(steer_gate, "released"));
			await wait_for(async () =>
				(
					await context.runtime.runPromise(
						context.journal.ReadCorrelatedEvents("blocked_follow_up"),
					)
				).some((event) => event.payload.type === "thread.message_routed"),
			);

			/** The withdrawn steer stayed out of the engine even once dispatch freed. */
			expect(capable.commands.map((sent) => sent._tag)).toEqual(["steer"]);
			const withdrawn_events = await context.runtime.runPromise(
				context.journal.ReadCorrelatedEvents("queued_follow_up"),
			);
			expect(withdrawn_events.map((event) => event.payload.type)).not.toContain(
				"thread.message_steering",
			);
			const references = await steer_source_references(context);
			expect(references).toContain("blocked_follow_up");
			expect(references).not.toContain("queued_follow_up");
		} finally {
			await context.runtime.dispose();
		}
	});

	/**
	 * Once dispatch has claimed the steer the engine may already hold its text,
	 * so the recall is refused rather than pretended — and the refusal must not
	 * disturb the delivery it lost the race to.
	 */
	it("refuses to withdraw a steer the engine already holds", async () => {
		const steer_gate = await Effect.runPromise(Deferred.make<"released">());
		const capable = make_engine("capable", steer_gate);
		const context = await setup([capable.engine], "thread_withdraw_claimed");
		try {
			await send(context, "start", "Summarize the current implementation");
			await wait_for(() => capable.opened.length === 1);
			await send(context, "claimed_follow_up", "Also include verification evidence");
			await wait_for(() => capable.commands.length === 1);

			await expect(
				context.runtime.runPromise(
					context.orchestrator.Handle(
						command("withdraw_claimed", context.thread_id, {
							command_id: "claimed_follow_up",
							type: "thread.withdraw_message",
						}),
					),
				),
			).rejects.toThrow();

			await Effect.runPromise(Deferred.succeed(steer_gate, "released"));
			await wait_for(async () =>
				(
					await context.runtime.runPromise(
						context.journal.ReadCorrelatedEvents("claimed_follow_up"),
					)
				).some((event) => event.payload.type === "thread.message_routed"),
			);
			expect(await steer_source_references(context)).toContain("claimed_follow_up");
		} finally {
			await context.runtime.dispose();
		}
	});
});
