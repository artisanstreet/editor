import { mkdtemp, rm } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { Effect, Layer, Stream } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { Engine, EngineOpenInput } from "@artisan/engines";
import type { AuthoritativeCommandEnvelope } from "../../modules/backend/src/persistence/orchestration/message-command";
import {
	AgentOrchestrator,
	GlobalGuidanceService,
	GuidanceProviderRegistry,
	make_backend_runtime,
	make_guidance_provider_registry_layer,
	make_runtime_guidance_adapter,
	ProtocolRouter,
} from "@artisan/backend";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

function make_capabilities(global_guidance: "supported" | "unsupported") {
	return {
		approval: { state: "supported" as const },
		auth: { state: "supported" as const },
		cancel: { state: "supported" as const },
		close: { state: "supported" as const },
		events: { state: "supported" as const },
		global_guidance: { state: global_guidance },
		model_selection: { state: "supported" as const },
		native_tools: { state: "supported" as const },
		probe: { state: "supported" as const },
		question: { state: "supported" as const },
		raw_frames: { state: "supported" as const },
		resume: { state: "supported" as const },
		start: { state: "supported" as const },
		steer: { state: "supported" as const },
		subagents: { state: "supported" as const },
	} satisfies Engine["Descriptor"]["capabilities"];
}

async function make_capture_engine(global_guidance: "supported" | "unsupported" = "supported") {
	const opened: Array<EngineOpenInput> = [];
	const waiters: Array<(input: EngineOpenInput) => void> = [];
	const capture = (input: EngineOpenInput) => {
		const waiter = waiters.shift();

		if (waiter) {
			waiter(input);

			return;
		}

		opened.push(input);
	};
	const next_open = () => {
		const input = opened.shift();

		return input
			? Promise.resolve(input)
			: new Promise<EngineOpenInput>((resolve) => waiters.push(resolve));
	};
	const engine: Engine = {
		Descriptor: {
			capabilities: make_capabilities(global_guidance),
			display_name: "Captured Codex",
			id: "codex",
			transport: "test",
		},
		Open: (input) =>
			Effect.sync(() => capture(input)).pipe(
				Effect.as({
					artisan_run_id: input.artisan_run_id,
					Closed: Effect.succeed("closed" as const),
					Events: Stream.empty,
					native_thread_id: `native:${input.artisan_run_id}`,
					resume_token: { native_thread_id: `native:${input.artisan_run_id}` },
					Send: () => Effect.void,
				}),
			),
		Probe: () => Effect.die("Probe is not used by guidance handoff tests"),
	};

	return { engine, next_open };
}

async function make_paths(label: string) {
	const root = await mkdtemp(join(tmpdir(), `artisan-guidance-handoff-${label}-`));

	temporary_directories.push(root);

	return {
		canonical: join(root, "guidance", "GLOBAL.md"),
		database: join(root, "artisan.db"),
		provider: join(root, "provider", "AGENTS.md"),
		root,
	};
}

function command(
	message_id: string,
	thread_id: string,
	payload: AuthoritativeCommandEnvelope["payload"],
): AuthoritativeCommandEnvelope {
	return {
		kind: "command",
		message_id,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-11T13:00:00.000Z",
		thread_id,
	};
}

function route(
	runtime: ReturnType<typeof make_backend_runtime>,
	envelope: AuthoritativeCommandEnvelope,
) {
	return runtime.runPromise(
		Effect.gen(function* () {
			if (envelope.payload.type === "thread.send_message") {
				const orchestrator = yield* AgentOrchestrator;

				return yield* orchestrator.Handle(envelope);
			}
			const router = yield* ProtocolRouter;

			return yield* router.Route(envelope);
		}),
	);
}

async function update_guidance(runtime: ReturnType<typeof make_backend_runtime>, content: string) {
	await runtime.runPromise(
		Effect.gen(function* () {
			const guidance = yield* GlobalGuidanceService;

			yield* guidance.Update({
				content,
				message_id: "guidance_handoff_update",
				origin: "frontend",
				sent_at: "2026-07-11T13:00:00.000Z",
			});
		}),
	);
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("global guidance engine handoff", () => {
	it("passes runtime guidance separately to ordinary and graph runs", async () => {
		const paths = await make_paths("runtime");
		const captured = await make_capture_engine();
		const runtime = make_backend_runtime({
			database_path: paths.database,
			engines: [captured.engine],
			guidance: { canonical_path: paths.canonical },
			guidance_provider_registry: make_guidance_provider_registry_layer([
				make_runtime_guidance_adapter("codex"),
			]),
			migrations_path,
		});

		try {
			await update_guidance(runtime, "Keep guidance out of user text.");
			await route(
				runtime,
				command("create_ordinary", "thread_ordinary", {
					title: "Ordinary guidance",
					type: "thread.create",
				}),
			);
			await route(
				runtime,
				command("start_ordinary", "thread_ordinary", {
					engine_id: "codex",
					text: "Byte-exact ordinary request",
					type: "thread.send_message",
					working_directory: paths.root,
				}),
			);
			const ordinary = await captured.next_open();

			await route(
				runtime,
				command("create_graph", "thread_graph", {
					title: "Graph guidance",
					type: "thread.create",
				}),
			);
			const graph_result = await route(
				runtime,
				command("start_graph", "thread_graph", {
					assignments: [
						{
							assignment_id: "assignment_guidance",
							engine_id: "codex",
							expected_result: "A reviewed result",
							instructions: "Perform the graph assignment",
							parent_node_id: "group_guidance",
							permission_policy: {
								approval: "never",
								network_access: false,
								write_access: true,
							},
							profile: "default",
							role: "implementer",
							scope: { kind: "repo", value: paths.root, write_access: true },
							summary_contract: "Return a concise summary",
							workspace: {
								isolation: "shared",
								workspace_id: "workspace_guidance",
								working_directory: paths.root,
							},
						},
						{
							assignment_id: "assignment_companion",
							engine_id: "codex",
							expected_result: "A companion result",
							instructions: "Perform the companion assignment",
							parent_node_id: "group_guidance",
							permission_policy: {
								approval: "never",
								network_access: false,
								write_access: false,
							},
							profile: "default",
							role: "reviewer",
							scope: { kind: "repo", value: paths.root, write_access: false },
							summary_contract: "Return a companion summary",
							workspace: {
								isolation: "shared",
								workspace_id: "workspace_companion",
								working_directory: paths.root,
							},
						},
					],
					group_id: "group_guidance",
					max_concurrency: 1,
					name_bank: ["Gibby", "Bob"],
					type: "orchestration.group.start",
				}),
			);

			expect(graph_result).toContainEqual(
				expect.objectContaining({
					kind: "command.receipt",
					payload: expect.objectContaining({ status: "accepted" }),
				}),
			);

			const graph = await captured.next_open();
			const expected_guidance = {
				content: "Keep guidance out of user text.\n",
				source_file: paths.canonical,
			};

			expect(ordinary).toMatchObject({
				_tag: "start",
				global_guidance: expected_guidance,
				initial_text: "Byte-exact ordinary request",
			});
			expect(graph).toMatchObject({
				_tag: "start",
				global_guidance: expected_guidance,
			});
			expect("initial_text" in graph ? graph.initial_text : undefined).toBeOneOf([
				[
					"Perform the graph assignment",
					"Expected result: A reviewed result",
					"Summary contract: Return a concise summary",
				].join("\n\n"),
				[
					"Perform the companion assignment",
					"Expected result: A companion result",
					"Summary contract: Return a companion summary",
				].join("\n\n"),
			]);
		} finally {
			await runtime.dispose();
		}
	});

	it("does not duplicate native-file guidance into Engine.Open", async () => {
		const paths = await make_paths("native");
		const captured = await make_capture_engine();
		const native_registry = Layer.succeed(GuidanceProviderRegistry, {
			Providers: [
				{
					Discover: Effect.succeed({ _tag: "Absent" as const, path: paths.provider }),
					mode: "native_file" as const,
					provider: "codex" as const,
				},
			],
		});
		const runtime = make_backend_runtime({
			database_path: paths.database,
			engines: [captured.engine],
			guidance: { canonical_path: paths.canonical },
			guidance_provider_registry: native_registry,
			migrations_path,
		});

		try {
			await update_guidance(runtime, "Synced natively.");
			await route(
				runtime,
				command("create_native", "thread_native", {
					title: "Native guidance",
					type: "thread.create",
				}),
			);
			await route(
				runtime,
				command("start_native", "thread_native", {
					engine_id: "codex",
					text: "Native user request",
					type: "thread.send_message",
					working_directory: paths.root,
				}),
			);
			const opened = await captured.next_open();

			expect(opened).toMatchObject({
				_tag: "start",
				initial_text: "Native user request",
			});
			expect(opened).not.toHaveProperty("global_guidance");
		} finally {
			await runtime.dispose();
		}
	});
});
