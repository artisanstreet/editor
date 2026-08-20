import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Option } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { EngineObservation } from "@artisan/engines";
import type { CommandEnvelope } from "@artisan/protocol";
import { make_backend_runtime, ProtocolRouter, TerminalRepository } from "@artisan/backend";
import { ProjectCatalog } from "../../modules/backend/src/projects/project-catalog";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { ObservedTerminalAdoption } from "../../modules/backend/src/terminal/observed-adoption";
import { ObservedTerminalId } from "../../modules/backend/src/terminal/observed";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const project_id = "project_observed";
const thread_id = "thread_observed";

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

const make_runtime = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-observed-terminal-"));
	directories.push(directory);
	return make_backend_runtime({
		database_path: join(directory, "artisan.db"),
		migrations_path,
	});
};

const create_command: CommandEnvelope = {
	kind: "command",
	message_id: "create_observed_thread",
	origin: "frontend",
	payload: { project_id, title: "Observed terminals", type: "thread.create" },
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-16T09:00:00.000Z",
	thread_id,
};

/** Attaches a project and creates a thread inside it, the way the app does. */
const OpenThread = Effect.gen(function* () {
	const catalog = yield* ProjectCatalog;
	yield* catalog.Attach({
		display_name: "Observed",
		project_id,
		root_path: tmpdir(),
	});
	const router = yield* ProtocolRouter;
	yield* router.Route(create_command);
});

const run = {
	agent_id: "agent_root",
	run_id: "run_observed",
	thread_id,
	working_directory: tmpdir(),
};

/** The shape the Claude and Codex normalizers already emit for every shell call. */
const started: EngineObservation = {
	_tag: "terminal_activity",
	activity_id: "toolu_dev",
	artisan_run_id: "run_observed",
	command: "npm run dev",
	observation_id: "observation_1",
	shell: "pwsh",
	raw: { engine_id: "claude", frame: {}, transport: "stdio" },
	sequence: 1,
	state: "started",
};

/**
 * Artisan runs over another harness, so the shells a run opens belong to the
 * engine, not to Forge. They reach the backend only as `terminal_activity`
 * observations, whose only consumer was the transcript — which is why the
 * Terminals card stayed empty no matter what the engine was running.
 */
describe("observed terminal wiring", () => {
	it("puts a command the engine ran into the list the card reads", async () => {
		const runtime = await make_runtime();
		try {
			const terminals = await runtime.runPromise(
				Effect.gen(function* () {
					yield* OpenThread;
					const adoption = yield* ObservedTerminalAdoption;
					yield* adoption.AdoptBatch(run, [started]);
					const repository = yield* TerminalRepository;

					return yield* repository.List(thread_id, project_id);
				}),
			);

			/** Exactly what the card renders: one live row, titled by its shell. */
			expect(terminals).toHaveLength(1);
			expect(terminals[0]?.terminal_id).toBe(ObservedTerminalId("toolu_dev"));
			expect(terminals[0]?.executable).toBe("pwsh");
			expect(terminals[0]?.args).toEqual(["npm", "run", "dev"]);
			expect(terminals[0]?.state).toBe("active");
			/** The agent really did run it, so the ownership is not a fiction. */
			expect(terminals[0]?.ownership).toEqual({
				agent_id: "agent_root",
				kind: "agent",
				run_id: "run_observed",
			});
			/** Nothing here spawned a process, so nothing claims one. */
			expect(terminals[0]?.pid).toBeUndefined();
		} finally {
			await runtime.dispose();
		}
	});

	it("closes the same row when the command exits rather than opening another", async () => {
		const runtime = await make_runtime();
		try {
			const terminals = await runtime.runPromise(
				Effect.gen(function* () {
					yield* OpenThread;
					const adoption = yield* ObservedTerminalAdoption;
					yield* adoption.AdoptBatch(run, [started]);
					yield* adoption.AdoptBatch(run, [
						{ ...started, exit_code: 0, state: "completed" } as EngineObservation,
					]);
					const repository = yield* TerminalRepository;

					return yield* repository.List(thread_id, project_id);
				}),
			);

			/** One command is one terminal, whatever the frame count. */
			expect(terminals).toHaveLength(1);
			expect(terminals[0]?.state).toBe("closed");
			expect(terminals[0]?.exit_code).toBe(0);
		} finally {
			await runtime.dispose();
		}
	});

	it("adopts nothing for a thread with no project to file it under", async () => {
		const runtime = await make_runtime();
		try {
			const resolved = await runtime.runPromise(
				Effect.gen(function* () {
					const adoption = yield* ObservedTerminalAdoption;
					yield* adoption.AdoptBatch({ ...run, thread_id: "thread_absent" }, [started]);
					const threads = yield* ThreadReadModel;

					return yield* threads.Lookup("thread_absent");
				}),
			);

			/** Absorbed, not thrown: a missing view must never fail the run. */
			expect(Option.isNone(resolved)).toBe(true);
		} finally {
			await runtime.dispose();
		}
	});
});
