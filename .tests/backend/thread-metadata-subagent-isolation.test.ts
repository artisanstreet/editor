import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import type { CommandEnvelope } from "@artisan/protocol";
import {
	make_backend_runtime,
	make_thread_metadata_refiner_test_layer,
	ProtocolRouter,
	ThreadMetadataRefinementCoordinator,
} from "@artisan/backend";

import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const migrations_path = fileURLToPath(new URL("../../modules/backend/drizzle", import.meta.url));

const MakeRuntimeMetadata = () => {
	let id = 0;
	let instant = Date.parse("2026-08-10T12:00:00.000Z");
	return Layer.succeed(
		RuntimeMetadata,
		RuntimeMetadata.of({
			instance_id: "backend-subagent-status",
			MakeId: (prefix) => Effect.sync(() => `${prefix}-subagent-status-${(id += 1)}`),
			Now: Effect.sync(() => new Date((instant += 1_000)).toISOString()),
		}),
	);
};

const MakeCommand = (message_id: string, payload: CommandEnvelope["payload"]): CommandEnvelope => ({
	kind: "command",
	message_id,
	origin: "frontend",
	payload,
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-10T12:00:00.000Z",
	thread_id: "thread-subagent-status",
});

describe("thread metadata subagent isolation", () => {
	it("settles from the orchestration group rather than one child run", async () => {
		const directory = await mkdtemp(join(tmpdir(), "artisan-subagent-status-"));
		const seen: Array<string> = [];
		const runtime = make_backend_runtime({
			database_path: join(directory, "artisan.db"),
			migrations_path,
			runtime_metadata: MakeRuntimeMetadata(),
			thread_metadata_refiner: make_thread_metadata_refiner_test_layer((input) =>
				Effect.sync(() => {
					seen.push(input.trigger);
					return { current_goal: `Refined ${input.trigger}` };
				}),
			),
		});

		try {
			const states = await runtime.runPromise(
				Effect.gen(function* () {
					const coordinator = yield* ThreadMetadataRefinementCoordinator;
					const journal = yield* JournalStore;
					const router = yield* ProtocolRouter;
					const threads = yield* ThreadReadModel;

					yield* router.Route(
						MakeCommand("create-subagent-status", {
							title: "Subagent status",
							type: "thread.create",
						}),
					);
					yield* journal.AppendEvent({
						causation_id: "user-cause",
						correlation_id: "user-correlation",
						payload: {
							message_id: "user-message",
							reason: "no_active_run",
							text: "Delegate the review",
							type: "thread.message_queued",
							working_directory: "C:/workspace/artisan",
						},
						thread_id: "thread-subagent-status",
					});
					yield* coordinator.WaitForIdle;
					const before_child = (yield* threads.Snapshot()).threads[0]!;

					yield* journal.AppendEvent({
						agent_id: "agent-child",
						causation_id: "child-complete",
						correlation_id: "root-run",
						payload: {
							action: "provider observed subagent",
							attempt: 1,
							group_id: "group-native",
							node_id: "child-run",
							node_type: "agent_run",
							state: "complete",
							type: "orchestration.graph.lifecycle",
						},
						run_id: "child-run",
						thread_id: "thread-subagent-status",
					});
					yield* coordinator.WaitForIdle;
					const after_child = (yield* threads.Snapshot()).threads[0]!;

					yield* journal.AppendEvent({
						agent_id: "agent-root",
						causation_id: "group-complete",
						correlation_id: "root-run",
						payload: {
							action: "provider root lifecycle reconciled",
							group_id: "group-native",
							node_id: "group-native",
							node_type: "orchestration_group",
							state: "complete",
							type: "orchestration.graph.lifecycle",
						},
						thread_id: "thread-subagent-status",
					});
					yield* coordinator.WaitForIdle;
					const after_group = (yield* threads.Snapshot()).threads[0]!;

					yield* journal.AppendEvent({
						agent_id: "agent-root",
						causation_id: "assistant-complete",
						correlation_id: "root-run",
						payload: {
							message_id: "assistant-message",
							text: "The delegated review is complete.",
							type: "assistant.message_completed",
						},
						run_id: "root-run",
						thread_id: "thread-subagent-status",
					});
					yield* coordinator.WaitForIdle;
					const after_assistant = (yield* threads.Snapshot()).threads[0]!;

					return { after_assistant, after_child, after_group, before_child };
				}),
			);

			expect(states.before_child.live_status).toBe("Idle");
			expect(states.after_child.live_status).toBe(states.before_child.live_status);
			expect(states.after_child.last_activity_at).not.toBe(
				states.before_child.last_activity_at,
			);
			expect(states.after_child.last_message_at).toBe(states.before_child.last_message_at);
			expect(states.after_child.reader_activity_at).toBe(
				states.before_child.reader_activity_at,
			);
			expect(states.after_group.live_status).toBe("Idle");
			expect(states.after_group.reader_activity_at).not.toBe(
				states.after_child.reader_activity_at,
			);
			expect(states.after_group.last_message_at).toBe(states.before_child.last_message_at);
			expect(states.after_assistant.last_message_at).not.toBe(
				states.after_group.last_message_at,
			);
			expect(states.after_assistant.last_activity_at).toBe(
				states.after_group.last_activity_at,
			);
			expect(seen).toEqual(["user_message", "run_completed", "assistant_message"]);
		} finally {
			await runtime.dispose();
			await rm(directory, { force: true, recursive: true });
		}
	});
});
