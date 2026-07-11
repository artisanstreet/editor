import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Cause, Effect, Exit, Layer } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import type { CommandEnvelope } from "@artisan/protocol";
import { make_backend_runtime, ProtocolRouter, WorkspaceEvidenceRecorder } from "@artisan/backend";

import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));
const temporary_directories: Array<string> = [];

async function make_database_path() {
	const directory = await mkdtemp(join(tmpdir(), "artisan-workspace-evidence-recorder-"));

	temporary_directories.push(directory);

	return join(directory, "artisan.db");
}

function make_metadata_layer() {
	let next_id = 0;

	return Layer.succeed(RuntimeMetadata, {
		instance_id: "workspace_evidence_recorder_test",
		MakeId: (prefix) => Effect.sync(() => `${prefix}_${++next_id}`),
		Now: Effect.succeed("2026-07-11T19:00:00.000Z"),
	});
}

function make_create_command(thread_id: string): CommandEnvelope {
	return {
		kind: "command",
		message_id: `create_${thread_id}`,
		origin: "frontend",
		payload: {
			title: "Workspace evidence",
			type: "thread.create",
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: "2026-07-11T19:00:00.000Z",
		thread_id,
	};
}

async function create_thread(runtime: ReturnType<typeof make_backend_runtime>, thread_id: string) {
	await runtime.runPromise(
		Effect.gen(function* () {
			const router = yield* ProtocolRouter;

			yield* router.Route(make_create_command(thread_id));
		}),
	);
}

function filesystem_input(overrides: Partial<{ readonly path: string }> = {}) {
	return {
		agent_id: "agent_evidence",
		operation: "write" as const,
		operation_id: "filesystem_evidence",
		path: "C:/work/alpha/src/main.ts",
		raw_origin: { provider: "codex", reference: "call_42" },
		run_id: "run_evidence",
		thread_id: "thread_evidence",
		...overrides,
	};
}

afterEach(async () => {
	await Promise.all(
		temporary_directories
			.splice(0)
			.map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

describe("WorkspaceEvidenceRecorder", () => {
	it("publishes content-free attributed evidence with deterministic trace identities", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			await create_thread(runtime, "thread_evidence");
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const recorder = yield* WorkspaceEvidenceRecorder;

					const filesystem = yield* recorder.RecordFilesystemMutation(filesystem_input());
					const process = yield* recorder.RecordProcessOwnership({
						operation_id: "process_evidence",
						source: "artisan_tool",
						thread_id: "thread_evidence",
						working_directory: "C:/work/alpha",
					});
					const git = yield* recorder.RecordGitWorkspaceObserved({
						branch: "feature/evidence",
						changed_file_count: 2,
						has_diff: true,
						operation_id: "git_evidence",
						root_path: "C:/work/alpha",
						thread_id: "thread_evidence",
						worktree_path: "C:/work/alpha",
					});

					return { filesystem, git, process };
				}),
			);

			expect(result.filesystem).toMatchObject({
				status: "accepted",
				event: {
					agent_id: "agent_evidence",
					causation_id: "workspace_evidence:filesystem_evidence:causation",
					correlation_id: "workspace_evidence:filesystem_evidence:correlation",
					payload: { type: "filesystem.mutation" },
					raw_origin: { provider: "codex", reference: "call_42" },
					run_id: "run_evidence",
				},
			});
			expect(result.process.event.payload.type).toBe("process.ownership");
			expect(result.git.event.payload.type).toBe("git.workspace.observed");
		} finally {
			await runtime.dispose();
		}
	});

	it("returns the original event for exact retries after a runtime restart", async () => {
		const database_path = await make_database_path();
		const first_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		await create_thread(first_runtime, "thread_evidence");
		const accepted = await first_runtime.runPromise(
			Effect.gen(function* () {
				const recorder = yield* WorkspaceEvidenceRecorder;

				return yield* recorder.RecordFilesystemMutation(filesystem_input());
			}),
		);

		await first_runtime.dispose();

		const second_runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			const duplicate = await second_runtime.runPromise(
				Effect.gen(function* () {
					const recorder = yield* WorkspaceEvidenceRecorder;

					return yield* recorder.RecordFilesystemMutation(filesystem_input());
				}),
			);

			expect(duplicate).toEqual({ event: accepted.event, status: "duplicate" });
		} finally {
			await second_runtime.dispose();
		}
	});

	it("rejects operation-id reuse when its intent or attribution changes", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			await create_thread(runtime, "thread_evidence");
			const recorder = await runtime.runPromise(WorkspaceEvidenceRecorder);

			await runtime.runPromise(recorder.RecordFilesystemMutation(filesystem_input()));

			const conflicts = [
				recorder.RecordFilesystemMutation(
					filesystem_input({ path: "C:/work/alpha/src/changed.ts" }),
				),
				recorder.RecordFilesystemMutation({
					...filesystem_input(),
					thread_id: "thread_changed",
				}),
				recorder.RecordFilesystemMutation({
					...filesystem_input(),
					agent_id: "agent_changed",
				}),
				recorder.RecordFilesystemMutation({
					...filesystem_input(),
					raw_origin: { provider: "claude", reference: "call_42" },
				}),
				recorder.RecordFilesystemMutation({
					...filesystem_input(),
					run_id: "run_changed",
				}),
				recorder.RecordProcessOwnership({
					operation_id: "filesystem_evidence",
					source: "artisan_tool",
					thread_id: "thread_evidence",
					working_directory: "C:/work/alpha",
				}),
			];

			for (const conflict of conflicts) {
				await expect(runtime.runPromise(conflict)).rejects.toMatchObject({
					_tag: "WorkspaceEvidenceConflict",
					operation_id: "filesystem_evidence",
				});
			}
		} finally {
			await runtime.dispose();
		}
	});

	it("serializes concurrent attempts before appending the same operation", async () => {
		const database_path = await make_database_path();
		const runtime = make_backend_runtime({
			database_path,
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			await create_thread(runtime, "thread_evidence");
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const recorder = yield* WorkspaceEvidenceRecorder;
					const journal = yield* JournalStore;
					const attempts = yield* Effect.all(
						Array.from({ length: 4 }, () =>
							recorder.RecordFilesystemMutation(filesystem_input()),
						),
						{ concurrency: "unbounded" },
					);
					const events = yield* journal.ReadCorrelatedEvents(
						"workspace_evidence:filesystem_evidence:correlation",
					);

					return { attempts, events };
				}),
			);

			expect(result.attempts.map((attempt) => attempt.status)).toEqual([
				"accepted",
				"duplicate",
				"duplicate",
				"duplicate",
			]);
			expect(result.events).toHaveLength(1);
		} finally {
			await runtime.dispose();
		}
	});

	it("rejects malformed tool evidence before it reaches the journal", async () => {
		const runtime = make_backend_runtime({
			database_path: await make_database_path(),
			migrations_path,
			runtime_metadata: make_metadata_layer(),
		});

		try {
			await create_thread(runtime, "thread_evidence");
			const result = await runtime.runPromise(
				Effect.gen(function* () {
					const recorder = yield* WorkspaceEvidenceRecorder;
					const journal = yield* JournalStore;
					const invalid = yield* Effect.exit(
						recorder.RecordFilesystemMutation({
							...filesystem_input(),
							operation_id: " ",
						}),
					);
					const events = yield* journal.ReadReplay({ after_journal_sequence: 0 });

					return { events, invalid };
				}),
			);

			const error = Exit.isFailure(result.invalid)
				? Cause.squash(result.invalid.cause)
				: undefined;

			expect(error).toMatchObject({ _tag: "WorkspaceEvidenceInvalid" });
			expect(result.events.map(({ payload }) => payload.type)).not.toContain(
				"filesystem.mutation",
			);
		} finally {
			await runtime.dispose();
		}
	});
});
