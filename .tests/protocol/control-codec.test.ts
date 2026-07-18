import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeInboundControlEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

const timestamp = "2026-07-18T08:00:00.000Z";

const frontend = (kind: string, payload: unknown) => ({
	kind,
	message_id: `message_${kind}`,
	origin: "frontend" as const,
	payload,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: timestamp,
});

const backend = (kind: string, correlation_id: string, payload: unknown) => ({
	correlation_id,
	kind,
	message_id: `result_${kind}`,
	origin: "backend" as const,
	payload,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: timestamp,
});

describe("tool control-plane protocol codec", () => {
	it("decodes all built-in registry, invocation, approval, and editor capability requests", async () => {
		const envelopes = [
			frontend("artisan.tool.registry.list.query", {
				policy: {
					allow_engine_observation: true,
					allow_git_index_write: false,
					allow_preview_control: false,
					allow_process_control: true,
					allow_workspace_read: true,
					allow_workspace_write: false,
					approval: "on_request",
				},
				workspace_id: "workspace_1",
			}),
			{
				...frontend("artisan.tool.execute", {
					input: {
						assumption_id: "assumption_1",
						statement: "Use the existing formatter.",
						tool_id: "assumption.record",
					},
					invocation_id: "invocation_1",
				}),
				thread_id: "thread_1",
			},
			{
				...frontend("artisan.approval.resolve", {
					approval_id: "approval_1",
					approved: true,
					invocation_id: "invocation_1",
					resolution_id: "resolution_1",
				}),
				thread_id: "thread_1",
			},
			frontend("artisan.tool.invocation.list.query", { limit: 20, thread_id: "thread_1" }),
			frontend("artisan.approval.list.query", { state: "pending", thread_id: "thread_1" }),
			frontend("workspace.file.discovery.query", { limit: 20, workspace_id: "workspace_1" }),
			frontend("workspace.language.capabilities.query", { workspace_id: "workspace_1" }),
		];

		await expect(
			Promise.all(
				envelopes.map((envelope) =>
					Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
				),
			),
		).resolves.toEqual(envelopes);
	});

	it("decodes bounded renderer-safe tool registry, discovery, and language results", async () => {
		const policy = {
			allow_engine_observation: true,
			allow_git_index_write: false,
			allow_preview_control: false,
			allow_process_control: true,
			allow_workspace_read: true,
			allow_workspace_write: false,
			approval: "on_request" as const,
		};
		const results = [
			backend("artisan.tool.registry.list.query.result", "registry_1", {
				availability: [{ state: "available", tool_id: "workspace.file.list" }],
				declarations: [
					{
						descriptor: {
							approval_behavior: "never",
							description: "Lists bounded paths.",
							id: "workspace.file.list",
							kind: "workspace_file",
							permission_requirements: ["workspace_read"],
							schema_version: 1,
							title: "List workspace files",
						},
						input_schema_version: 1,
						output_schema_version: 1,
					},
				],
				journal_sequence: 1,
				usage: [
					{
						active_invocation_count: 0,
						tool_id: "workspace.file.list",
						total_invocation_count: 0,
					},
				],
			}),
			backend("workspace.file.discovery.query.result", "files_1", {
				entries: [{ kind: "file", modified_at: timestamp, path: "src/main.ts", size: 12 }],
				truncated: false,
				workspace_id: "workspace_1",
			}),
			backend("workspace.language.capabilities.query.result", "language_1", {
				capabilities: [
					{ feature: "diagnostics", source: "unavailable", state: "unavailable" },
				],
				workspace_id: "workspace_1",
			}),
		];

		await expect(
			Promise.all(
				results.map((result) => Effect.runPromise(DecodeOutboundControlEnvelope(result))),
			),
		).resolves.toEqual(results);
		expect(policy.allow_workspace_read).toBe(true);
	});
});
