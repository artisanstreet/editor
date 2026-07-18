import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	ArtisanApprovalRequest,
	ArtisanApprovalResolveRequest,
	ArtisanAssumptionEvent,
	ArtisanNativeActionEvent,
	ArtisanToolDescriptor,
	ArtisanToolExecutionRequest,
	ArtisanToolInvocation,
	ArtisanToolInvocationListQuery,
	ArtisanToolRegistryListQueryResult,
	WorkspaceFileDiscoveryQueryResult,
	WorkspaceLanguageCapabilitiesQueryResult,
} from "@artisan/protocol";

const timestamp = "2026-07-18T08:00:00.000Z";

const Decode = <A>(schema: Schema.Codec<A, A>, input: unknown) =>
	Effect.runPromise(Schema.decodeUnknownEffect(schema, { onExcessProperty: "error" })(input));

function invocation(overrides: Record<string, unknown> = {}) {
	return {
		input_summary: "Read terminal output.",
		invocation_id: "invocation_1",
		lifecycle: "succeeded",
		outcome: { code: "terminal.completed", status: "succeeded" },
		permission: {
			decision: "allowed",
			policy: {
				allow_engine_observation: true,
				allow_git_index_write: false,
				allow_preview_control: true,
				allow_process_control: true,
				allow_workspace_read: true,
				allow_workspace_write: false,
				approval: "on_request",
			},
			requirements: ["process_control"],
			tool_id: "terminal.read",
		},
		requested_at: timestamp,
		thread_id: "thread_1",
		tool_id: "terminal.read",
		updated_at: timestamp,
		...overrides,
	};
}

describe("Artisan built-in tool protocol schemas", () => {
	it("decodes an attributable controlled invocation with workspace evidence", async () => {
		const value = invocation({
			agent_id: "agent_1",
			completed_at: timestamp,
			raw_origin: { provider: "codex", reference: "item_1" },
			run_id: "run_1",
			workspace_evidence: {
				operation_id: "operation_1",
				recorded_event_type: "process.ownership",
				recorder: "workspace_evidence",
			},
		});

		await expect(Decode(ArtisanToolInvocation, value)).resolves.toEqual(value);
	});

	it("keeps question, approval, assumption, and native actions in canonical bounded shapes", async () => {
		await expect(
			Decode(ArtisanApprovalRequest, {
				approval_id: "approval_1",
				description: "Stage the selected exact paths.",
				invocation_id: "invocation_1",
				permission_requirements: ["git_index_write"],
				requested_at: timestamp,
			}),
		).resolves.toMatchObject({ approval_id: "approval_1" });
		await expect(
			Decode(ArtisanAssumptionEvent, {
				assumption_id: "assumption_1",
				invocation_id: "invocation_1",
				statement: "The existing workspace is the intended target.",
				type: "artisan.assumption.recorded",
			}),
		).resolves.toMatchObject({ type: "artisan.assumption.recorded" });
		await expect(
			Decode(ArtisanNativeActionEvent, {
				action: "web_search",
				invocation_id: "invocation_1",
				raw_origin: { provider: "codex", reference: "item_1" },
				tool_id: "engine.native_action.record",
				type: "engine.native_action",
			}),
		).resolves.toMatchObject({ type: "engine.native_action" });
	});

	it("declares only bounded built-in registry entries", async () => {
		const value = {
			availability: [
				{
					state: "available",
					tool_id: "workspace.file.read",
				},
			],
			declarations: [
				{
						descriptor: {
							approval_behavior: "never",
							description: "Reads an exact controlled workspace file.",
						id: "workspace.file.read",
						kind: "workspace_file",
						permission_requirements: ["workspace_read"],
						schema_version: 1,
						title: "Read workspace file",
					},
					input_schema_version: 1,
					output_schema_version: 1,
				},
			],
			journal_sequence: 1,
			usage: [
				{
					active_invocation_count: 0,
					tool_id: "workspace.file.read",
					total_invocation_count: 3,
				},
			],
		};

		await expect(Decode(ArtisanToolRegistryListQueryResult, value)).resolves.toEqual(value);
	});

	it("defines renderer-safe execution, resolution, and invocation-history requests", async () => {
		await expect(
			Decode(ArtisanToolExecutionRequest, {
				input: {
					path: "src/main.ts",
					tool_id: "workspace.file.read",
					workspace_id: "workspace_1",
				},
				invocation_id: "invocation_2",
			}),
		).resolves.toMatchObject({ input: { tool_id: "workspace.file.read" } });
		await expect(
			Decode(ArtisanApprovalResolveRequest, {
				approval_id: "approval_1",
				approved: true,
				invocation_id: "invocation_1",
				resolution_id: "resolution_1",
			}),
		).resolves.toMatchObject({ approved: true });
		await expect(
			Decode(ArtisanToolInvocationListQuery, {
				limit: 100,
				thread_id: "thread_1",
				tool_id: "terminal.read",
			}),
		).resolves.toMatchObject({ limit: 100 });
	});

	it("keeps file discovery content-free and language capabilities truthful", async () => {
		await expect(
			Decode(WorkspaceFileDiscoveryQueryResult, {
				entries: [
					{
						kind: "file",
						modified_at: timestamp,
						path: "src/main.ts",
						size: 128,
					},
				],
				truncated: false,
				workspace_id: "workspace_1",
			}),
		).resolves.toMatchObject({ entries: [{ path: "src/main.ts" }] });
		await expect(
			Decode(WorkspaceLanguageCapabilitiesQueryResult, {
				capabilities: [
					{
						feature: "diagnostics",
						reason: "No language service is configured.",
						source: "unavailable",
						state: "unavailable",
					},
				],
				workspace_id: "workspace_1",
			}),
		).resolves.toMatchObject({ capabilities: [{ state: "unavailable" }] });
	});

	it("rejects impossible outcome and policy-sensitive shape drift", async () => {
		await expect(
			Decode(ArtisanToolInvocation, invocation({ tool_id: "git.index.stage" })),
		).rejects.toBeDefined();
		await expect(
			Decode(ArtisanApprovalRequest, {
				approval_id: "approval_1",
				description: "x",
				invocation_id: "invocation_1",
				permission_requirements: [],
				requested_at: timestamp,
			}),
		).rejects.toBeDefined();
		await expect(
			Decode(ArtisanNativeActionEvent, {
				action: "search",
				extra: true,
				invocation_id: "invocation_1",
				tool_id: "engine.native_action.record",
				type: "engine.native_action",
			}),
		).rejects.toBeDefined();
		await expect(
			Decode(ArtisanToolDescriptor, {
				description: "Not a terminal action.",
				id: "git.status.read",
				kind: "terminal",
				permission_requirements: ["git_read"],
				schema_version: 1,
				title: "Invalid descriptor",
			}),
		).rejects.toBeDefined();
	});
});
