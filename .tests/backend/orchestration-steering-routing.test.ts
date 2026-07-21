import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Deferred, Effect, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { AgentOrchestrator, make_backend_runtime } from "@artisan/backend";
import {
	EngineProcessError,
	type Engine,
	type EngineCommand,
	type EngineRun,
} from "@artisan/engines";
import type { CommandEnvelope, EventEnvelope } from "@artisan/protocol";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-steering-routing-"));
	temporary_directories.push(directory);
	return join(directory, "artisan.db");
}

function command(
	message_id: string,
	thread_id: string,
	payload: CommandEnvelope["payload"],
): CommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-18T20:00:00.000Z",
		thread_id,
	};
}

function make_engine(
	id: string,
	steer_state: "supported" | "unsupported" | "experimental" = "supported",
	fail_steer = false,
) {
	const commands: Array<EngineCommand> = [];
	const opened: Array<string> = [];
	const open_inputs: Array<Parameters<Engine["Open"]>[0]> = [];
	const closed: Array<Deferred.Deferred<"closed">> = [];
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
		].map((name) => [name, { state: name === "steer" ? steer_state : ("supported" as const) }]),
	) as Engine["Descriptor"]["capabilities"];
	const engine: Engine = {
		Descriptor: { capabilities, display_name: id, id, transport: "test" },
		Open: (input) =>
			Effect.gen(function* () {
				opened.push(input.artisan_run_id);
				open_inputs.push(input);
				const run_closed = yield* Deferred.make<"closed">();
				closed.push(run_closed);
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
							if (fail_steer && sent._tag === "steer") {
								yield* Effect.fail(
									new EngineProcessError({
										cause: "send failed",
										operation: "write",
									}),
								);
							}
						}),
				} satisfies EngineRun;
			}),
		Probe: () => Effect.die("not used"),
	};
	return { closed, commands, engine, open_inputs, opened };
}

async function wait_for(predicate: () => boolean | Promise<boolean>) {
	const deadline = Date.now() + 2_000;
	while (!(await predicate())) {
		if (Date.now() >= deadline) throw new Error("timed out waiting for orchestration");
		await new Promise((resolve) => setTimeout(resolve, 5));
	}
}

const routed = (events: ReadonlyArray<EventEnvelope>) =>
	events.findLast((event) => event.payload.type === "thread.message_routed")?.payload;

async function setup(
	engines: ReadonlyArray<Engine>,
	input_database_path?: string,
	thread_id = "thread_steering",
) {
	const database_path = input_database_path ?? (await make_database_path());
	const runtime = make_backend_runtime({ database_path, engines, migrations_path });
	const journal = await runtime.runPromise(JournalStore);
	const orchestrator = await runtime.runPromise(AgentOrchestrator);
	await runtime.runPromise(
		journal.AcceptThreadCreate(
			command("create", thread_id, { title: "Steering", type: "thread.create" }),
		),
	);
	return { database_path, journal, orchestrator, runtime, thread_id };
}

async function start(
	context: Awaited<ReturnType<typeof setup>>,
	engine_id: string,
	message_id = "start",
) {
	await context.runtime.runPromise(
		context.orchestrator.Handle(
			command(message_id, context.thread_id, {
				engine_id,
				text: "Summarize the current implementation",
				type: "thread.send_message",
				working_directory: "C:/work",
			}),
		),
	);
}

async function follow_up(
	context: Awaited<ReturnType<typeof setup>>,
	engine_id: string,
	message_id: string,
) {
	const accepted = await context.runtime.runPromise(
		context.orchestrator.Handle(
			command(message_id, context.thread_id, {
				engine_id,
				text: "Also include the verification evidence",
				type: "thread.send_message",
				working_directory: "C:/work",
			}),
		),
	);
	return accepted;
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("thread follow-up steering routing", () => {
	it("resolves durable thread policy into the normal engine launch metadata", async () => {
		const policy_engine = make_engine("policy-engine");
		const context = await setup([policy_engine.engine]);

		try {
			await context.runtime.runPromise(
				context.orchestrator.Handle(
					command("set_policy", context.thread_id, {
						policy: {
							engine_id: "codex",
							model: "gpt-5.3-codex",
							permission_mode: "never",
							reasoning_effort: "xhigh",
							sandbox_mode: "read_only",
							strict_clarification: true,
							web_search_enabled: true,
						},
						type: "thread.session_policy.update",
					}),
				),
			);
			await start(context, "policy-engine");
			await wait_for(() => policy_engine.open_inputs.length === 1);

			expect(policy_engine.open_inputs[0]).toMatchObject({
				model: "gpt-5.3-codex",
				permission_policy: {
					approval: "never",
					network_access: false,
					write_access: false,
				},
				provider_options: { "codex.reasoning_effort": "xhigh" },
			});
		} finally {
			await context.runtime.dispose();
		}
	});

	it("steers a capable active run by default and exact retries do not redeliver", async () => {
		const capable = make_engine("capable");
		const context = await setup([capable.engine]);
		let restarted: ReturnType<typeof make_backend_runtime> | undefined;
		try {
			await start(context, "capable");
			await wait_for(() => capable.opened.length === 1);
			const first = await follow_up(context, "capable", "follow_up");
			expect(routed(first.events)).toBeUndefined();
			await wait_for(() => capable.commands.length === 1);
			const duplicate = await follow_up(context, "capable", "follow_up");
			const delivered_events = await context.runtime.runPromise(
				context.journal.ReadCorrelatedEvents("follow_up"),
			);
			expect(routed(delivered_events)).toMatchObject({
				outcome: "steered",
				type: "thread.message_routed",
			});
			expect(duplicate.status).toBe("duplicate");
			expect(capable.commands).toEqual([
				{
					_tag: "steer",
					command_id: "follow_up",
					text: "Also include the verification evidence",
				},
			]);
			expect(capable.opened).toHaveLength(1);
			const before_restart_events = await context.runtime.runPromise(
				context.journal.ReadCorrelatedEvents("follow_up"),
			);

			await context.runtime.dispose();
			restarted = make_backend_runtime({
				database_path: context.database_path,
				engines: [capable.engine],
				migrations_path,
			});
			const restarted_orchestrator = await restarted.runPromise(AgentOrchestrator);
			const after_restart = await restarted.runPromise(
				restarted_orchestrator.Handle(
					command("follow_up", context.thread_id, {
						engine_id: "capable",
						text: "Also include the verification evidence",
						type: "thread.send_message",
						working_directory: "C:/work",
					}),
				),
			);
			expect(after_restart.status).toBe("duplicate");
			expect(capable.commands).toHaveLength(1);
			expect(capable.opened).toHaveLength(1);
			const restarted_journal = await restarted.runPromise(JournalStore);
			expect(
				await restarted.runPromise(restarted_journal.ReadCorrelatedEvents("follow_up")),
			).toEqual(before_restart_events);
		} finally {
			if (restarted) await restarted.dispose();
			else await context.runtime.dispose();
		}
	});

	it("queues disabled, unsupported, experimental, and engine-mismatched follow-ups", async () => {
		for (const scenario of [
			{ state: "supported" as const, disable: true, requested: "active", reason: "disabled" },
			{
				state: "unsupported" as const,
				disable: false,
				requested: "active",
				reason: "unsupported",
			},
			{
				state: "experimental" as const,
				disable: false,
				requested: "active",
				reason: "unsupported",
			},
			{
				state: "supported" as const,
				disable: false,
				requested: "other",
				reason: "ambiguous_target",
			},
		]) {
			const active = make_engine("active", scenario.state);
			const other = make_engine("other");
			const context = await setup(
				[active.engine, other.engine],
				await make_database_path(),
				`thread_${scenario.reason}_${scenario.state}`,
			);
			try {
				await start(context, "active");
				await wait_for(() => active.opened.length === 1);
				if (scenario.disable) {
					await context.runtime.runPromise(
						context.orchestrator.Handle(
							command("disable", context.thread_id, {
								enabled: false,
								type: "thread.auto_steer.update",
							}),
						),
					);
				}
				const accepted = await follow_up(context, scenario.requested, "follow_up");
				expect(routed(accepted.events)).toMatchObject({
					outcome: "queued",
					reason: scenario.reason,
					type: "thread.message_routed",
				});
				expect(active.commands).toHaveLength(0);
			} finally {
				await context.runtime.dispose();
			}
		}
	});

	it("falls back with rejected when the durable run has no live owner", async () => {
		const capable = make_engine("capable");
		const context = await setup([capable.engine]);
		try {
			await start(context, "capable");
			await wait_for(() => capable.opened.length === 1);
			await Effect.runPromise(Deferred.succeed(capable.closed[0]!, "closed"));
			await new Promise((resolve) => setTimeout(resolve, 20));
			await follow_up(context, "capable", "missing_live");
			await wait_for(async () =>
				(
					await context.runtime.runPromise(
						context.journal.ReadCorrelatedEvents("missing_live"),
					)
				).some(
					(event) =>
						event.payload.type === "thread.message_routed" &&
						event.payload.reason === "rejected",
				),
			);
			const events = await context.runtime.runPromise(
				context.journal.ReadCorrelatedEvents("missing_live"),
			);
			expect(routed(events)).toMatchObject({ outcome: "queued", reason: "rejected" });
			expect(
				events.find((event) => event.payload.type === "thread.message_queued")?.payload,
			).toMatchObject({ reason: "rejected" });
			expect(capable.commands).toHaveLength(0);
		} finally {
			await context.runtime.dispose();
		}
	});

	it("falls back with delivery_failed when Engine Send fails", async () => {
		const failing = make_engine("failing", "supported", true);
		const context = await setup([failing.engine]);
		try {
			await start(context, "failing");
			await wait_for(() => failing.opened.length === 1);
			const accepted = await follow_up(context, "failing", "send_failure");
			expect(routed(accepted.events)).toBeUndefined();
			await wait_for(async () =>
				(
					await context.runtime.runPromise(
						context.journal.ReadCorrelatedEvents("send_failure"),
					)
				).some(
					(event) =>
						event.payload.type === "thread.message_routed" &&
						event.payload.reason === "delivery_failed",
				),
			);
			const events = await context.runtime.runPromise(
				context.journal.ReadCorrelatedEvents("send_failure"),
			);
			expect(routed(events)).toMatchObject({ outcome: "queued", reason: "delivery_failed" });
			expect(
				events.find((event) => event.payload.type === "thread.message_queued")?.payload,
			).toMatchObject({ reason: "delivery_failed" });
			expect(failing.commands).toHaveLength(1);
		} finally {
			await context.runtime.dispose();
		}
	});
});
