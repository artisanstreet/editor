import { Effect, Layer, Option, Ref } from "effect";
import { describe, expect, it } from "vitest";

import type {
	CommandEnvelope,
	EventEnvelope,
	GitIndexStageRequestEnvelope,
	OutboundControlEnvelope,
	OutboundEnvelope,
	ThreadCreateEnvelope,
} from "@artisan/protocol";
import { SnowflakeId } from "@artisan/protocol";

import { GitService } from "../../modules/backend/src/git/service";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { ThreadReadModel } from "../../modules/backend/src/persistence/thread-read-model";
import type {
	ConnectionState,
	ReadyState,
} from "../../modules/backend/src/protocol/connection-state";
import {
	MakeReadyMutations,
	ReadyConnectionRuntime,
} from "../../modules/backend/src/protocol/rpc/ready-mutations";
import { ProtocolRouter } from "../../modules/backend/src/protocol/router";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";
import { WorkspaceFileService } from "../../modules/backend/src/workspace/files/service";

const test_time = "2026-08-19T10:00:00.000Z";

const ready_state: ReadyState = {
	_tag: "Ready",
	acknowledged_cursors: {},
	acknowledged_journal_sequence: 0,
	connection_id: "connection_1",
	delivered_cursors: {},
	delivered_journal_sequence: 3,
	last_activity_ms: 0,
	stream_ticket: "ticket_1",
	subscriptions: {},
};

/**
 * The event a routed command commits carries the newest journal sequence of
 * the whole journal. Delivering it directly would advance the connection
 * cursor past every event journaled since the last notifier wake, so the
 * mutation layer must only ever ask for committed-tail delivery.
 */
const routed_event = {
	journal_sequence: 9,
	kind: "event",
	message_id: "event_routed",
	origin: "backend",
	payload: { title: "Routed", type: "thread.created" },
	protocol_version: 1,
	schema_version: 1,
	sent_at: test_time,
	sequence: 1,
	stream_id: "thread:thread_1",
	thread_id: "thread_1",
} as unknown as EventEnvelope;

const routed_receipt = {
	correlation_id: "command_tail_1",
	kind: "command.receipt",
	message_id: "receipt_routed",
	origin: "backend",
	payload: { journal_sequence: 9, status: "accepted" },
	protocol_version: 1,
	schema_version: 1,
	sent_at: test_time,
} as unknown as OutboundEnvelope;

const make_harness = Effect.gen(function* () {
	const deliveries: Array<string | undefined> = [];
	const enqueued: Array<OutboundControlEnvelope> = [];
	const errors: Array<{
		readonly code: string;
		readonly correlation_id: string | undefined;
	}> = [];
	const state = yield* Ref.make<ConnectionState>(ready_state);
	const runtime = ReadyConnectionRuntime.of({
		DeliverCommittedTail: (correlation_id?: string) =>
			Effect.sync(() => {
				deliveries.push(correlation_id);
			}),
		Enqueue: (envelope: OutboundControlEnvelope) =>
			Effect.sync(() => {
				enqueued.push(envelope);
			}),
		EnqueueError: (
			_current: ConnectionState,
			code: string,
			_message: string,
			_retryable: boolean,
			correlation_id?: string,
		) =>
			Effect.sync(() => {
				errors.push({ code, correlation_id });
			}),
		state,
	});
	const mutations = yield* MakeReadyMutations.pipe(
		Effect.provide(
			Layer.mergeAll(
				Layer.succeed(ReadyConnectionRuntime, runtime),
				Layer.succeed(
					ProtocolRouter,
					ProtocolRouter.of({
						RouteCommand: () => Effect.succeed([routed_receipt, routed_event]),
					} as never),
				),
				Layer.succeed(
					JournalStore,
					JournalStore.of({
						FindCommandThreadId: () => Effect.succeed(Option.none()),
					} as never),
				),
				Layer.succeed(
					ThreadReadModel,
					ThreadReadModel.of({
						Lookup: () =>
							Effect.succeed(
								Option.some({ thread_id: "thread_fresh", title: "Created" }),
							),
					} as never),
				),
				Layer.succeed(
					RuntimeMetadata,
					RuntimeMetadata.of({
						instance_id: "instance_test",
						MakeId: (prefix) => Effect.succeed(`${prefix}_test`),
						Now: Effect.succeed(test_time),
					}),
				),
				Layer.succeed(
					SnowflakeId,
					SnowflakeId.of({
						MakeBare: Effect.succeed("thread_fresh"),
					} as never),
				),
				Layer.succeed(WorkspaceFileService, WorkspaceFileService.of({} as never)),
				Layer.succeed(
					GitService,
					GitService.of({
						Request: () =>
							Effect.succeed({
								event: { journal_sequence: 9 },
								status: "accepted" as const,
							}),
					} as never),
				),
			),
		),
	);

	return { deliveries, enqueued, errors, mutations };
});

describe("ready mutation tail delivery", () => {
	it("delivers the committed journal tail for a routed command, never its own events", async () => {
		const harness = await Effect.runPromise(make_harness);
		const command = {
			kind: "command",
			message_id: "command_tail_1",
			origin: "frontend",
			payload: { type: "thread.retention.update" },
			protocol_version: 1,
			schema_version: 1,
			sent_at: test_time,
			thread_id: "thread_1",
		} as unknown as CommandEnvelope;

		await Effect.runPromise(harness.mutations.HandleCommand(command));

		expect(harness.deliveries).toEqual(["command_tail_1"]);
		expect(harness.enqueued).toContain(routed_receipt);
		expect(harness.enqueued).not.toContain(routed_event);
		expect(harness.errors).toEqual([]);
	});

	it("delivers the committed journal tail for a Forge-owned thread create", async () => {
		const harness = await Effect.runPromise(make_harness);
		const request = {
			kind: "thread.create.request",
			message_id: "create_tail_1",
			origin: "frontend",
			payload: { title: "Created" },
			protocol_version: 1,
			schema_version: 1,
			sent_at: test_time,
		} as unknown as ThreadCreateEnvelope;

		await Effect.runPromise(harness.mutations.HandleThreadCreate(request, ready_state));

		expect(harness.deliveries).toEqual(["create_tail_1"]);
		expect(harness.enqueued.map((envelope) => envelope.kind)).toEqual(["thread.create.result"]);
		expect(harness.errors).toEqual([]);
	});

	it("delivers the committed journal tail for an accepted Git mutation", async () => {
		const harness = await Effect.runPromise(make_harness);
		const stage = {
			kind: "git.index.stage.request",
			message_id: "git_tail_1",
			origin: "frontend",
			payload: { paths: ["src/example.ts"] },
			protocol_version: 1,
			schema_version: 1,
			sent_at: test_time,
			thread_id: "thread_1",
		} as unknown as GitIndexStageRequestEnvelope;

		await Effect.runPromise(harness.mutations.HandleGitMutation(stage));

		expect(harness.deliveries).toEqual(["git_tail_1"]);
		expect(harness.enqueued).toMatchObject([
			{
				kind: "command.receipt",
				payload: { journal_sequence: 9, status: "accepted" },
			},
		]);
		expect(harness.errors).toEqual([]);
	});
});
