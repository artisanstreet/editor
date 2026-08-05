import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import { AgentGraphOrchestrator, AgentOrchestrator, make_backend_runtime } from "@artisan/backend";
import type { Engine, EngineOpenInput, EngineRun } from "@artisan/engines";
import type { CommandEnvelope } from "@artisan/protocol";
import { ConversationReadModel } from "../../modules/backend/src/conversation";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import type { AuthoritativeCommandEnvelope } from "../../modules/backend/src/persistence/orchestration/message-command";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const sent_at = "2026-08-03T12:00:00.000Z";

const make_database_path = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-product-instructions-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

const capabilities = Object.fromEntries(
	[
		"approval",
		"auth",
		"cancel",
		"close",
		"events",
		"global_guidance",
		"model_selection",
		"native_continuation",
		"native_tools",
		"probe",
		"question",
		"raw_frames",
		"resume",
		"start",
		"steer",
		"subagents",
	].map((name) => [name, { state: "supported" }]),
) as Engine["Descriptor"]["capabilities"];

const make_engine = (options: { readonly persistent?: boolean } = {}) => {
	const open_inputs: Array<EngineOpenInput> = [];
	const Open = (input: EngineOpenInput) =>
		Effect.sync(() => {
			open_inputs.push(input);
			const native_thread_id = `native:${input.artisan_run_id}`;
			return {
				artisan_run_id: input.artisan_run_id,
				Closed: options.persistent ? Effect.never : Effect.succeed("completed" as const),
				Events: options.persistent ? Stream.never : Stream.empty,
				native_thread_id,
				resume_token: { native_thread_id },
				Send: () => Effect.void,
			} satisfies EngineRun;
		});

	return {
		engine: {
			Descriptor: {
				capabilities,
				display_name: "Product instruction test engine",
				id: "test",
				transport: "test",
			},
			Open,
			Probe: () => Effect.die("Probe is not used by this test"),
		} satisfies Engine,
		open_inputs,
	};
};

const command = <const Payload extends CommandEnvelope["payload"]>(
	message_id: string,
	payload: Payload,
): Omit<AuthoritativeCommandEnvelope, "payload"> & { readonly payload: Payload } => ({
	kind: "command",
	message_id,
	origin: "frontend",
	payload,
	protocol_version: 1,
	schema_version: 1,
	sent_at,
	thread_id: "product_instructions_thread",
});

const wait_for = async (predicate: () => boolean) => {
	const deadline = Date.now() + 5_000;
	while (!predicate()) {
		if (Date.now() > deadline) throw new Error("Timed out waiting for the Engine.Open input");
		await new Promise((resolve) => setTimeout(resolve, 5));
	}
};

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("product instruction orchestration handoff", () => {
	it("supplies the same product instructions to ordinary and graph Engine.Open calls", async () => {
		const fake = make_engine();
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			engines: [fake.engine],
			migrations_path,
		});

		try {
			const { conversations, graph, journal, orchestrator } = await runtime.runPromise(
				Effect.gen(function* () {
					return {
						conversations: yield* ConversationReadModel,
						graph: yield* AgentGraphOrchestrator,
						journal: yield* JournalStore,
						orchestrator: yield* AgentOrchestrator,
					};
				}),
			);
			await runtime.runPromise(
				journal.AcceptThreadCreate(
					command("create", { title: "Product instructions", type: "thread.create" }),
				),
			);
			await runtime.runPromise(
				orchestrator.Handle(
					command("ordinary", {
						engine_id: "test",
						text: "Ordinary user request.",
						type: "thread.send_message",
						working_directory: tmpdir(),
					}),
				),
			);
			await runtime.runPromise(
				graph.Handle(
					command("graph", {
						assignments: [
							{
								assignment_id: "assignment",
								engine_id: "test",
								expected_result: "A concise result.",
								instructions: "Graph assignment instructions.",
								parent_node_id: "product_instructions_group",
								permission_policy: {
									approval: "on_request",
									network_access: false,
									write_access: false,
								},
								profile: "test-model",
								role: "tester",
								scope: { kind: "files", value: "src", write_access: false },
								summary_contract: "Summarize the result.",
								workspace: {
									isolation: "isolated",
									working_directory: tmpdir(),
									workspace_id: "workspace",
								},
							},
						],
						group_id: "product_instructions_group",
						max_concurrency: 1,
						name_bank: ["Tester"],
						type: "orchestration.group.start",
					}),
				),
			);
			await wait_for(() => fake.open_inputs.length === 2);

			const [ordinary, graph_run] = fake.open_inputs;
			expect(ordinary?._tag).toBe("start");
			expect(graph_run?._tag).toBe("start");
			if (ordinary?._tag !== "start" || graph_run?._tag !== "start") {
				throw new Error("Expected fresh product-instruction test runs");
			}
			expect(ordinary?.product_instructions).toEqual(graph_run?.product_instructions);
			expect(ordinary?.product_instructions).toMatchObject({ source: "artisan-editor" });
			expect(ordinary?.product_instructions?.content).toContain(
				"You are responding inside Artisan Editor.",
			);
			expect(ordinary?.product_instructions?.content).toContain(
				"syntax highlighting, an optional filename, and optional selected-line emphasis",
			);
			expect(ordinary?.product_instructions?.content).toContain(
				"renders Mermaid fences as diagrams",
			);
			expect(ordinary?.product_instructions?.content).not.toContain(
				"currently presents only an ordinary code block",
			);
			expect(ordinary).not.toHaveProperty("global_guidance");
			expect(ordinary).toMatchObject({ initial_text: "Ordinary user request." });
			expect(ordinary?.initial_text).not.toContain(
				"You are responding inside Artisan Editor.",
			);
			expect(graph_run).not.toHaveProperty("global_guidance");
			expect(graph_run).toMatchObject({
				initial_text: expect.stringContaining("Graph assignment instructions."),
			});
			expect(graph_run?.initial_text).not.toContain(
				"You are responding inside Artisan Editor.",
			);

			const projection = await runtime.runPromise(
				conversations.ReadSnapshot("product_instructions_thread"),
			);
			expect(JSON.stringify(projection)).not.toContain(
				"You are responding inside Artisan Editor.",
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("re-supplies product instructions when a persisted native run resumes after restart", async () => {
		const database_path = await make_database_path();
		const initial = make_engine({ persistent: true });
		const initial_runtime = make_backend_runtime({
			database_path,
			engines: [initial.engine],
			migrations_path,
		});

		try {
			const { journal, orchestrator } = await initial_runtime.runPromise(
				Effect.gen(function* () {
					return {
						journal: yield* JournalStore,
						orchestrator: yield* AgentOrchestrator,
					};
				}),
			);
			await initial_runtime.runPromise(
				journal.AcceptThreadCreate(
					command("create_for_resume", {
						title: "Product instruction recovery",
						type: "thread.create",
					}),
				),
			);
			await initial_runtime.runPromise(
				orchestrator.Handle(
					command("start_for_resume", {
						engine_id: "test",
						text: "Keep this native run recoverable.",
						type: "thread.send_message",
						working_directory: tmpdir(),
					}),
				),
			);
			await wait_for(() => initial.open_inputs.length === 1);
		} finally {
			await initial_runtime.dispose();
		}

		const recovery = make_engine({ persistent: true });
		const recovery_runtime = make_backend_runtime({
			database_path,
			engines: [recovery.engine],
			migrations_path,
		});

		try {
			await recovery_runtime.runPromise(
				Effect.gen(function* () {
					yield* AgentOrchestrator;
				}),
			);
			await wait_for(() => recovery.open_inputs.length === 1);

			const resumed = recovery.open_inputs[0];
			expect(resumed).toMatchObject({
				_tag: "resume",
				product_instructions: { source: "artisan-editor" },
				resume_token: { native_thread_id: expect.any(String) },
			});
			expect(resumed?.product_instructions?.content).toContain(
				"You are responding inside Artisan Editor.",
			);
		} finally {
			await recovery_runtime.dispose();
		}
	});
});
