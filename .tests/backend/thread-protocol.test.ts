import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeCommandEnvelope,
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	ThreadContentErasedEvent,
	ThreadErasedEvent,
	ThreadListItem,
	ThreadRetentionPolicy,
	type ThreadMetadataRefineCommand,
	type ThreadRenameCommand,
} from "@artisan/protocol";

describe("thread protocol", () => {
	it("types manual rename commands and the locked sidebar projection", async () => {
		const payload = {
			title: "Durable thread identity",
			type: "thread.rename",
		} satisfies ThreadRenameCommand;
		const command = await Effect.runPromise(
			DecodeCommandEnvelope({
				kind: "command",
				message_id: "rename_1",
				origin: "frontend",
				payload,
				protocol_version: 1,
				schema_version: 1,
				sent_at: "2026-07-10T18:00:00.000Z",
				thread_id: "thread_1",
			}),
		);
		const projection = Schema.decodeUnknownSync(ThreadListItem)({
			activity_version: 1,
			affinity_version: 0,
			archived_at: undefined,
			created_at: "2026-07-10T17:00:00.000Z",
			current_goal: "Ship durable thread identity",
			last_activity_at: "2026-07-10T18:00:00.000Z",
			linked_projects: [],
			live_status: "Idle",
			metadata_version: 1,
			pinned: false,
			project_affinity_scores: [],
			project_locked: false,
			rename_suggestion: undefined,
			thread_id: "thread_1",
			title: "Durable thread identity",
			title_locked: true,
			title_source: "manual",
			updated_at: "2026-07-10T18:00:00.000Z",
		});

		expect(command.payload).toEqual(payload);
		expect(projection).toMatchObject({
			title: "Durable thread identity",
			title_locked: true,
			title_source: "manual",
		});
	});

	it("types versioned refinement and the opinionated retention policy surface", async () => {
		const refinement = {
			basis_activity_version: 4,
			basis_metadata_version: 2,
			current_goal: "Purge expired thread content",
			rename_suggestion: "Durable thread retention",
			title: "Thread retention boundary",
			type: "thread.metadata.refine",
		} satisfies ThreadMetadataRefineCommand;
		const [command, query, result] = await Effect.runPromise(
			Effect.all([
				DecodeCommandEnvelope({
					kind: "command",
					message_id: "refine_1",
					origin: "frontend",
					payload: refinement,
					protocol_version: 1,
					schema_version: 1,
					sent_at: "2026-07-10T18:00:00.000Z",
					thread_id: "thread_1",
				}),
				DecodeInboundControlEnvelope({
					kind: "thread.retention.query",
					message_id: "retention_query_1",
					origin: "frontend",
					payload: {},
					protocol_version: 1,
					schema_version: 1,
					sent_at: "2026-07-10T18:00:00.000Z",
				}),
				DecodeOutboundControlEnvelope({
					correlation_id: "retention_query_1",
					kind: "thread.retention.query.result",
					message_id: "retention_result_1",
					origin: "backend",
					payload: {
						enabled: true,
						inactivity_days: 7,
					},
					protocol_version: 1,
					schema_version: 1,
					sent_at: "2026-07-10T18:00:00.000Z",
				}),
			]),
		);

		expect(command.payload).toEqual(refinement);
		expect(query.kind).toBe("thread.retention.query");
		expect(result.payload).toEqual({ enabled: true, inactivity_days: 7 });
	});

	it("rejects unsafe retention durations at both policy bounds", () => {
		const decode = Schema.decodeUnknownSync(ThreadRetentionPolicy);

		expect(() => decode({ enabled: true, inactivity_days: 0 })).toThrow();
		expect(() => decode({ enabled: true, inactivity_days: 3651 })).toThrow();
		expect(decode({ enabled: true, inactivity_days: 1 })).toEqual({
			enabled: true,
			inactivity_days: 1,
		});
		expect(decode({ enabled: true, inactivity_days: 3650 })).toEqual({
			enabled: true,
			inactivity_days: 3650,
		});
	});

	it("rejects sidebar projections that omit stable metadata invariants", () => {
		const decode = Schema.decodeUnknownSync(ThreadListItem);

		expect(() =>
			decode({
				created_at: "2026-07-10T17:00:00.000Z",
				thread_id: "thread_legacy",
				title: "Legacy row",
				updated_at: "2026-07-10T17:00:00.000Z",
			}),
		).toThrow();
	});

	it("decodes content-free erasure events and ordered list removals", async () => {
		const content_erased = Schema.decodeUnknownSync(ThreadContentErasedEvent)({
			type: "thread.content_erased",
		});
		const erased = Schema.decodeUnknownSync(ThreadErasedEvent)({
			type: "thread.erased",
		});
		const remove = await Effect.runPromise(
			DecodeOutboundControlEnvelope({
				journal_sequence: 12,
				kind: "thread.list.remove",
				message_id: "remove_1",
				origin: "backend",
				payload: { thread_id: "thread_1" },
				protocol_version: 1,
				schema_version: 1,
				sent_at: "2026-07-10T18:00:00.000Z",
				sequence: 3,
				stream_id: "thread-list:subscription_1",
				subscription_id: "subscription_1",
			}),
		);

		expect(content_erased.type).toBe("thread.content_erased");
		expect(erased.type).toBe("thread.erased");
		expect(remove).toMatchObject({
			journal_sequence: 12,
			kind: "thread.list.remove",
			payload: { thread_id: "thread_1" },
			sequence: 3,
		});
	});
});
