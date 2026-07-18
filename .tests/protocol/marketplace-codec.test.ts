import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	CapabilityDetail,
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	EncodeOutboundControlEnvelope,
	RoutineInstallPreviewRequest,
	RoutineDetail,
} from "@artisan/protocol";

const timestamp = "2026-07-18T12:00:00.000Z";

const frontend_envelope = (kind: string, payload: unknown) => ({
	kind,
	message_id: `message_${kind}`,
	origin: "frontend",
	payload,
	protocol_version: 1,
	schema_version: 1,
	sent_at: timestamp,
});

const decode_routine = Schema.decodeUnknownSync(RoutineDetail, { onExcessProperty: "error" });
const decode_capability = Schema.decodeUnknownSync(CapabilityDetail, { onExcessProperty: "error" });
const decode_routine_preview = Schema.decodeUnknownSync(RoutineInstallPreviewRequest, {
	onExcessProperty: "error",
});

describe("Marketplace protocol", () => {
	it("keeps canonical routines detailed only after progressive discovery", () => {
		const routine = decode_routine({
			author: "Artisan",
			compatibility: [{ engine_id: "codex", state: "native" }],
			description: "Review a patch with a bounded checklist.",
			display_name: "Patch review",
			enabled: true,
			exported_commands: [{ description: "Review current change", name: "/review" }],
			files: [{ path: "SKILL.md", required: true }],
			id: "routine_review",
			permissions: [
				{ description: "Reads selected workspace files", kind: "filesystem_read" },
			],
			scope: { kind: "project", project_id: "project_1" },
			status: "enabled",
			source: {
				kind: "package_manager",
				locator: "npx skills artisan-review",
				revision: "1.2.0",
			},
			sync: [{ engine_id: "codex", status: "runtime_only", updated_at: timestamp }],
			trust: "known",
			version: "1.2.0",
			instructions: "Use the review checklist after inspecting the patch.",
		});

		expect(routine.exported_commands[0]?.name).toBe("/review");
		expect(routine.source.kind).toBe("package_manager");
	});

	it("models stdio and HTTP MCPs without exposing token material", () => {
		const stdio = decode_capability({
			auth: { kind: "none" },
			compatibility: [],
			display_name: "Local files",
			enabled: false,
			health: { status: "unknown" },
			id: "cap_local_files",
			lifecycle: "disconnected",
			permissions: [],
			policy: [],
			resources: [],
			scope: { kind: "workspace", workspace_id: "workspace_1" },
			status: "disabled",
			source: { kind: "local", locator: "C:/tools/files" },
			sync: [],
			tools: [],
			transport: {
				args: ["server.js"],
				command: "node",
				kind: "stdio",
				startup_timeout_ms: 5000,
			},
			trust: "local",
		});
		expect(stdio.transport.kind).toBe("stdio");

		const http = decode_capability({
			auth: {
				kind: "oauth",
				scopes: ["calendar.read"],
				token_ref: { provider: "keychain", secret_id: "oauth_calendar" },
				token_status: "refresh_required",
			},
			compatibility: [],
			display_name: "Calendar",
			enabled: true,
			health: { status: "auth_required" },
			id: "cap_calendar",
			lifecycle: "disconnected",
			permissions: [{ description: "Reads calendar", kind: "account" }],
			policy: [
				{
					approval: "always",
					enabled: true,
					name: "events.create",
					sensitive_label: "Create calendar event",
				},
			],
			resources: [],
			scope: { kind: "global" },
			status: "disabled",
			server_instructions: "Confirm before creating events.",
			source: { kind: "catalog", locator: "artisan:calendar" },
			sync: [],
			tools: [{ name: "events.create" }],
			transport: { kind: "streamable_http", url: "https://calendar.example.test/mcp" },
			trust: "verified",
		});
		expect(http.auth.kind).toBe("oauth");
		expect(http.transport.kind).toBe("streamable_http");
	});

	it("rejects plaintext secret/token fields and unknown provider blobs", () => {
		const base = {
			auth: { kind: "bearer", secret_ref: { provider: "keychain", secret_id: "calendar" } },
			compatibility: [],
			display_name: "Calendar",
			enabled: false,
			health: { status: "unknown" },
			id: "cap_calendar",
			lifecycle: "disconnected",
			permissions: [],
			policy: [],
			resources: [],
			scope: { kind: "global" },
			source: { kind: "catalog", locator: "artisan:calendar" },
			sync: [],
			tools: [],
			transport: { kind: "streamable_http", url: "https://calendar.example.test/mcp" },
			trust: "verified",
		};
		expect(() =>
			decode_capability({ ...base, auth: { ...base.auth, token: "plaintext-secret" } }),
		).toThrow();
		expect(() =>
			decode_capability({ ...base, raw_provider_config: "token=plaintext-secret" }),
		).toThrow();
		expect(() =>
			decode_capability({
				...base,
				transport: { kind: "streamable_http", url: "https://token@example.test/mcp" },
			}),
		).toThrow();
	});

	it("uses contextual scopes and previews before allocating approval", () => {
		expect(
			decode_routine_preview({
				scope: { kind: "workspace", workspace_id: "workspace_1" },
				source: { kind: "package_manager", locator: "npx skills artisan-review" },
			}),
		).toMatchObject({ scope: { workspace_id: "workspace_1" } });
		expect(() =>
			decode_routine_preview({
				approval_id: "must_not_be_allocated",
				scope: { kind: "workspace", workspace_id: "workspace_1" },
				source: { kind: "package_manager", locator: "npx skills artisan-review" },
			}),
		).toThrow();
	});

	it("rejects approval decisions detached from their immutable preview", async () => {
		await expect(
			Effect.runPromise(
				DecodeInboundControlEnvelope(
					frontend_envelope("marketplace.routine.install.decision", {
						approval_id: "approval_routine",
						approved: true,
					}),
				),
			),
		).rejects.toBeDefined();
	});

	it("decodes approval-gated routine and capability lifecycle requests", async () => {
		const frames = [
			frontend_envelope("marketplace.routine.install.request", {
				approval_id: "approval_routine",
				requested_by: "agent",
				scope: { kind: "project", project_id: "project_1" },
				source: { kind: "package_manager", locator: "npx skills artisan-review" },
				preview_fingerprint: "preview_routine",
			}),
			frontend_envelope("marketplace.routine.install.decision", {
				approval_id: "approval_routine",
				approved: true,
				preview_fingerprint: "preview_routine",
			}),
			frontend_envelope("marketplace.npx_skills.discover", {
				package_spec: "@acme/routines",
				scope: { kind: "global" },
			}),
			frontend_envelope("marketplace.npx_skills.import.request", {
				candidate_name: "review",
				package_spec: "@acme/routines",
				preview_fingerprint: "preview_npx_review",
				scope: { kind: "global" },
			}),
			frontend_envelope("marketplace.capability.connect.request", {
				approval_id: "approval_mcp",
				requested_by: "agent",
				scope: { kind: "workspace", workspace_id: "workspace_1" },
				source: { kind: "catalog", locator: "artisan:files" },
				transport: {
					args: [],
					command: "files-mcp",
					kind: "stdio",
					startup_timeout_ms: 5000,
				},
				preview_fingerprint: "preview_mcp",
			}),
			frontend_envelope("marketplace.capability.connect.decision", {
				approval_id: "approval_mcp",
				approved: true,
				preview_fingerprint: "preview_mcp",
			}),
			frontend_envelope("marketplace.capability.oauth.begin", {
				capability_id: "cap_calendar",
			}),
			frontend_envelope("marketplace.capability.oauth.complete", {
				callback_reference: "callback_1",
				capability_id: "cap_calendar",
			}),
			frontend_envelope("marketplace.capability.oauth.refresh", {
				capability_id: "cap_calendar",
			}),
			frontend_envelope("marketplace.capability.oauth.revoke", {
				capability_id: "cap_calendar",
			}),
			...["start", "reconnect", "health", "disconnect", "restart", "uninstall"].map(
				(action) =>
					frontend_envelope(`marketplace.capability.${action}`, {
						capability_id: "cap_files",
					}),
			),
			frontend_envelope("marketplace.capability.invoke", {
				arguments_json: "{}",
				capability_id: "cap_files",
				tool_name: "read_file",
			}),
		];
		await expect(
			Promise.all(
				frames.map((frame) => Effect.runPromise(DecodeInboundControlEnvelope(frame))),
			),
		).resolves.toEqual(frames);
	});

	it("encodes invocation metadata without an inline tool result", async () => {
		const frame = {
			correlation_id: "invoke_1",
			kind: "marketplace.capability.invoke.result",
			message_id: "result_1",
			origin: "backend",
			payload: {
				approval_required: true,
				capability_id: "cap_files",
				invocation_id: "invocation_1",
				status: "approved",
				tool_name: "read_file",
			},
			protocol_version: 1,
			schema_version: 1,
			sent_at: timestamp,
		};
		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(frame))).resolves.toEqual(
			frame,
		);
		await expect(Effect.runPromise(EncodeOutboundControlEnvelope(frame))).resolves.toEqual(
			frame,
		);
	});

	it("carries Marketplace lifecycle facts through the canonical event ledger", async () => {
		const frame = {
			agent_id: "agent_1",
			causation_id: "cause_1",
			correlation_id: "correlation_1",
			journal_sequence: 42,
			kind: "event",
			message_id: "event_1",
			origin: "backend",
			payload: {
				approval_id: "approval_1",
				artifact_id: "artifact_1",
				capability_health: "healthy",
				item_id: "cap_files",
				item_kind: "capability",
				operation: "invoked",
				status: "enabled",
				sync_status: "runtime_only",
				tool_name: "read_file",
				type: "marketplace.lifecycle",
			},
			protocol_version: 1,
			run_id: "run_1",
			schema_version: 1,
			sequence: 7,
			sent_at: timestamp,
			stream_id: "stream_1",
			thread_id: "thread_1",
		};
		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(frame))).resolves.toEqual(
			frame,
		);
		await expect(Effect.runPromise(EncodeOutboundControlEnvelope(frame))).resolves.toEqual(
			frame,
		);
	});
});
