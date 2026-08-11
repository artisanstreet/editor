import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type {
	EngineObservation,
	EngineSubagentObservation,
	EngineSubagentTranscriptObservation,
} from "@artisan/engines";
import { AgentGraphRepository, make_backend_runtime } from "@artisan/backend";

import { Database } from "../../modules/backend/src/persistence/database";
import { OrchestrationRepository } from "../../modules/backend/src/persistence/orchestration/repository";
import {
	ConversationSources,
	NativeSubagentTranscriptInbox,
	OrchestrationRuns,
} from "../../modules/backend/src/persistence/tables";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const directories: Array<string> = [];
const now = "2026-08-11T00:00:00.000Z";

const MakePath = async () => {
	const directory = await mkdtemp(join(tmpdir(), "artisan-native-transcript-retention-"));
	directories.push(directory);
	return join(directory, "artisan.db");
};

const Transcript = (observation_id: string): EngineSubagentTranscriptObservation => ({
	_tag: "subagent_transcript",
	agent_native_thread_id: "native-child",
	artisan_run_id: "root-run",
	content: {
		_tag: "agent_message_completed",
		item_id: "child-message",
		message: "Child prose",
		phase: "final",
	},
	observation_id,
	parent_native_thread_id: "native-root",
	raw: { engine_id: "codex", frame: {}, transport: "test" },
	sequence: 1,
});

const ChildLifecycle = (): EngineSubagentObservation => ({
	_tag: "subagent",
	agent_native_thread_id: "native-child",
	artisan_run_id: "root-run",
	observation_id: "child-running",
	parent_native_thread_id: "native-root",
	raw: {
		engine_id: "codex",
		frame: { state: "running" },
		native_id: "native:child-running",
		transport: "test",
	},
	sequence: 2,
	state: "running",
	turn_id: "turn:native-child",
});

const RootTerminal = (): EngineObservation => ({
	_tag: "run_terminal",
	artisan_run_id: "root-run",
	observation_id: "root-terminal",
	raw: {
		engine_id: "codex",
		frame: { state: "completed" },
		native_id: "native:root-terminal",
		transport: "test",
	},
	sequence: 1,
	state: "completed",
});

const InsertRoot = Effect.gen(function* () {
	const database = yield* Database;
	yield* database.client.insert(OrchestrationRuns).values({
		agent_id: "root-agent",
		created_at: now,
		engine_id: "codex",
		native_resume_json: null,
		native_thread_id: "native-root",
		run_id: "root-run",
		status: "running",
		thread_id: "thread-native",
		updated_at: now,
		working_directory: "C:\\workspace",
	});
});

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("native subagent transcript retention", () => {
	it("deletes a transcript in the same transaction as its durable projection", async () => {
		const runtime = make_backend_runtime({
			database_path: await MakePath(),
			engines: [],
			migrations_path,
		});

		try {
			await runtime.runPromise(InsertRoot);
			const remaining = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					const graph = yield* AgentGraphRepository;
					const database = yield* Database;
					const lifecycle = ChildLifecycle();
					yield* repository.RecordObservation(Transcript("child-transcript"));
					yield* repository.RecordObservation(lifecycle);
					yield* graph.RecordObservedSubagent(lifecycle);
					yield* graph.RecoverObservedSubagents;
					return yield* Effect.all({
						inbox: database.client.select().from(NativeSubagentTranscriptInbox),
						sources: database.client.select().from(ConversationSources),
					});
				}),
			);

			expect(remaining.inbox).toEqual([]);
			expect(remaining.sources).not.toContainEqual(
				expect.objectContaining({ source_id: "observation:child-transcript" }),
			);
		} finally {
			await runtime.dispose();
		}
	});

	it("deletes a late transcript once the root's terminal state durably rejects projection", async () => {
		const runtime = make_backend_runtime({
			database_path: await MakePath(),
			engines: [],
			migrations_path,
		});

		try {
			await runtime.runPromise(InsertRoot);
			const remaining = await runtime.runPromise(
				Effect.gen(function* () {
					const repository = yield* OrchestrationRepository;
					const graph = yield* AgentGraphRepository;
					const database = yield* Database;
					yield* repository.RecordObservation(RootTerminal());
					yield* repository.RecordObservation(Transcript("late-transcript"));
					yield* graph.RecoverObservedSubagents;
					return yield* database.client.select().from(NativeSubagentTranscriptInbox);
				}),
			);

			expect(remaining).toEqual([]);
		} finally {
			await runtime.dispose();
		}
	});
});
