import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeCommandEnvelope,
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	type ContentIdentity,
	type WorkspaceChange,
} from "@artisan/protocol";

const timestamp = "2026-07-11T08:00:00.000Z";
const content_hash = "a".repeat(64);

function frontend_envelope(kind: string, payload: unknown) {
	return {
		kind,
		message_id: `message_${kind}`,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: timestamp,
	};
}

function identity(byte_count = 0): ContentIdentity {
	return {
		algorithm: "sha256",
		byte_count,
		content_hash,
	};
}

function workspace_change(overrides: Partial<WorkspaceChange> = {}): WorkspaceChange {
	return {
		after_identity: identity(),
		agent_id: "agent_1",
		before_identity: identity(),
		change_id: "change_1",
		created_at: timestamp,
		path: "src/main.ts",
		review_state: "needs_review",
		rollback_state: "available",
		run_id: "run_1",
		source_command_id: "command_1",
		thread_id: "thread_1",
		updated_at: timestamp,
		version: 1,
		workspace_id: "workspace_1",
		...overrides,
	};
}

describe("workspace changes protocol codec", () => {
	it("roundtrips every workspace-change envelope through the public codecs", async () => {
		const inbound_envelopes = [
			frontend_envelope("workspace.file.read.query", {
				path: "src/main.ts",
				workspace_id: "workspace_1",
			}),
			{
				...frontend_envelope("workspace.file.replace", {
					change_id: "change_1",
					content: "export {};\n",
					expected_before: identity(10),
					path: "src/main.ts",
					workspace_id: "workspace_1",
				}),
				agent_id: "agent_1",
				raw_origin: { provider: "codex", reference: "item_1" },
				run_id: "run_1",
				thread_id: "thread_1",
			},
			{
				...frontend_envelope("workspace.change.review", { change_id: "change_1" }),
				thread_id: "thread_1",
			},
			{
				...frontend_envelope("workspace.change.rollback", {
					change_id: "change_1",
					expected_after: identity(10),
				}),
				thread_id: "thread_1",
			},
			frontend_envelope("workspace.change.list.query", {
				thread_id: "thread_1",
				workspace_id: "workspace_1",
			}),
		];
		const outbound_envelopes = [
			{
				correlation_id: "read_1",
				kind: "workspace.file.read.query.result",
				message_id: "read_result_1",
				origin: "backend",
				payload: {
					content: "export {};\n",
					identity: identity(10),
					path: "src/main.ts",
					workspace_id: "workspace_1",
				},
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
			{
				correlation_id: "list_1",
				kind: "workspace.change.list.query.result",
				message_id: "list_result_1",
				origin: "backend",
				payload: { changes: [workspace_change()], journal_sequence: 1 },
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
			{
				causation_id: "command_1",
				correlation_id: "replace_1",
				journal_sequence: 2,
				kind: "event",
				message_id: "workspace_change_event_1",
				origin: "backend",
				payload: {
					action: "recorded",
					change: workspace_change(),
					type: "workspace.change.updated",
				},
				protocol_version: 1,
				schema_version: 1,
				sequence: 1,
				sent_at: timestamp,
				stream_id: "thread_1",
				thread_id: "thread_1",
			},
		];

		for (const envelope of inbound_envelopes) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}

		for (const envelope of outbound_envelopes) {
			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}
	});

	it.each([
		"/src/main.ts",
		"C:/repo/src/main.ts",
		"C:src/main.ts",
		"../src/main.ts",
		"src/../main.ts",
		"src\\main.ts",
		"src/\u0000main.ts",
	])("rejects the invalid workspace path %j", async (path) => {
		const envelope = frontend_envelope("workspace.file.read.query", {
			path,
			workspace_id: "workspace_1",
		});

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it("rejects workspace text larger than the V1 UTF-8 control-frame bound", async () => {
		const envelope = {
			...frontend_envelope("workspace.file.replace", {
				change_id: "change_1",
				content: "\u{1F600}".repeat(1_048_577),
				expected_before: identity(),
				path: "src/main.ts",
				workspace_id: "workspace_1",
			}),
			agent_id: "agent_1",
			run_id: "run_1",
			thread_id: "thread_1",
		};

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it("rejects malformed content hashes", async () => {
		const envelope = {
			...frontend_envelope("workspace.file.replace", {
				change_id: "change_1",
				content: "",
				expected_before: { ...identity(), content_hash: "A".repeat(64) },
				path: "src/main.ts",
				workspace_id: "workspace_1",
			}),
			agent_id: "agent_1",
			run_id: "run_1",
			thread_id: "thread_1",
		};

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it.each(["thread_id", "run_id", "agent_id"] as const)(
		"requires replacement attribution field %s",
		async (field) => {
			const envelope = {
				...frontend_envelope("workspace.file.replace", {
					change_id: "change_1",
					content: "",
					expected_before: identity(),
					path: "src/main.ts",
					workspace_id: "workspace_1",
				}),
				agent_id: "agent_1",
				run_id: "run_1",
				thread_id: "thread_1",
			};

			delete envelope[field];

			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).rejects.toBeDefined();
		},
	);

	it("rejects excess workspace properties and source text in command payloads", async () => {
		const excess_envelope = frontend_envelope("workspace.file.read.query", {
			extra: "nope",
			path: "src/main.ts",
			workspace_id: "workspace_1",
		});
		const command_envelope = {
			...frontend_envelope("command", {
				content: "private source text",
				title: "Workspace change",
				type: "thread.create",
			}),
			thread_id: "thread_1",
		};

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(excess_envelope)),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(DecodeCommandEnvelope(command_envelope)),
		).rejects.toBeDefined();
	});
});
