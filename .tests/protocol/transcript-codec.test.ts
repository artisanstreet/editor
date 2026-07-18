import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeInboundControlEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

const trace = {
	message_id: "message_1",
	origin: "frontend" as const,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: "2026-07-18T10:00:00.000Z",
};
const backend = {
	origin: "backend" as const,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: "2026-07-18T10:00:00.000Z",
};

describe("transcript and orchestration discovery protocol", () => {
	it("accepts bounded transcript and group-discovery requests", async () => {
		await expect(
			Effect.runPromise(
				DecodeInboundControlEnvelope({
					...trace,
					kind: "thread.transcript.query",
					payload: { after_journal_sequence: 7, limit: 50, thread_id: "thread_1" },
				}),
			),
		).resolves.toMatchObject({ kind: "thread.transcript.query" });
		await expect(
			Effect.runPromise(
				DecodeInboundControlEnvelope({
					...trace,
					kind: "orchestration.group.list.query",
					payload: { include_terminal: false, thread_id: "thread_1" },
				}),
			),
		).resolves.toMatchObject({ kind: "orchestration.group.list.query" });
	});

	it("allows only renderer-safe transcript facts in ordered append patches", async () => {
		const append = {
			...backend,
			message_id: "projection_message_1",
			kind: "thread.transcript.append",
			journal_sequence: 9,
			sequence: 1,
			stream_id: "projection:thread.transcript:thread_1:sub_1",
			subscription_id: "sub_1",
			payload: {
				entries: [
					{
						event_id: "event_9",
						journal_sequence: 9,
						occurred_at: backend.sent_at,
						payload: {
							type: "assistant.message_completed",
							message_id: "message_9",
							text: "Safe completed response.",
						},
					},
				],
			},
		};
		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(append)),
		).resolves.toMatchObject({
			kind: "thread.transcript.append",
			payload: { entries: [{ journal_sequence: 9 }] },
		});
		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					...append,
					payload: {
						entries: [
							{
								...append.payload.entries[0],
								payload: {
									type: "filesystem.mutation",
									operation: "write",
									path: "C:/secret",
								},
							},
						],
					},
				}),
			),
		).rejects.toBeDefined();
	});

	it("preserves explicit erased and unavailable transcript snapshots", async () => {
		for (const status of ["erased", "unavailable"] as const) {
			await expect(
				Effect.runPromise(
					DecodeOutboundControlEnvelope({
						...backend,
						message_id: `projection_${status}`,
						kind: "thread.transcript.snapshot",
						journal_sequence: 12,
						sequence: 0,
						stream_id: "projection:thread.transcript:thread_1:sub_1",
						subscription_id: "sub_1",
						payload: { status, journal_sequence: 12, entries: [] },
					}),
				),
			).resolves.toMatchObject({ payload: { status, journal_sequence: 12 } });
		}
	});

	it("decodes a live group-list replacement patch", async () => {
		const group = {
			coordinator_agent_id: "agent_1",
			created_at: backend.sent_at,
			group_id: "group_1",
			max_concurrency: 2,
			state: "complete" as const,
			thread_id: "thread_1",
			updated_at: backend.sent_at,
			version: 3,
		};
		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					...backend,
					message_id: "group_projection_1",
					kind: "orchestration.group.list.patch",
					journal_sequence: 15,
					sequence: 2,
					stream_id: "projection:orchestration.group.list:thread_1:sub_1",
					subscription_id: "sub_1",
					payload: { groups: [group], journal_sequence: 15 },
				}),
			),
		).resolves.toMatchObject({
			kind: "orchestration.group.list.patch",
			payload: { groups: [{ state: "complete" }] },
		});
	});
});
