import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	PreviewInspectionRequest,
	PreviewInspectionSession,
} from "@artisan/protocol";

const timestamp = "2026-07-18T08:00:00.000Z";

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

function backend_envelope(kind: string, payload: unknown) {
	return {
		correlation_id: `correlation_${kind}`,
		kind,
		message_id: `message_${kind}`,
		origin: "backend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: timestamp,
	};
}

function preview_event(payload: unknown) {
	return {
		causation_id: "preview_command_1",
		correlation_id: "preview_request_1",
		journal_sequence: 8,
		kind: "event",
		message_id: "preview_event_1",
		origin: "backend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: timestamp,
		sequence: 1,
		stream_id: "thread:thread_1",
		thread_id: "thread_1",
	};
}

function target(overrides: Record<string, unknown> = {}) {
	return {
		created_at: timestamp,
		id: "preview_1",
		journal_sequence: 8,
		launch_state: "idle",
		port: 5173,
		project_id: "project_1",
		routes: ["/"],
		state: "healthy",
		thread_id: "thread_1",
		updated_at: timestamp,
		url: "http://127.0.0.1:5173/",
		workspace_id: "workspace_1",
		...overrides,
	};
}

describe("preview protocol codec", () => {
	it("accepts only loopback targets with explicit port and routes", async () => {
		const decoded = await Effect.runPromise(
			DecodeInboundControlEnvelope(
				frontend_envelope("preview.target.register", {
					id: "preview_1",
					port: 5173,
					project_id: "project_1",
					routes: ["/", "/status"],
					source: { kind: "terminal", terminal_id: "terminal_1" },
					thread_id: "thread_1",
					url: "http://localhost:5173/",
					workspace_id: "workspace_1",
				}),
			),
		);

		expect(decoded.kind).toBe("preview.target.register");
		await expect(
			Effect.runPromise(
				DecodeInboundControlEnvelope(
					frontend_envelope("preview.target.register", {
						id: "preview_localhost",
						port: 5173,
						project_id: "project_1",
						routes: ["/"],
						thread_id: "thread_1",
						url: "http://app.localhost:5173/",
						workspace_id: "workspace_1",
					}),
				),
			),
		).resolves.toMatchObject({ kind: "preview.target.register" });
		await expect(
			Effect.runPromise(
				DecodeInboundControlEnvelope(
					frontend_envelope("preview.target.register", {
						id: "preview_1",
						port: 5173,
						project_id: "project_1",
						routes: ["/"],
						thread_id: "thread_1",
						url: "https://example.com/",
						workspace_id: "workspace_1",
					}),
				),
			),
		).rejects.toBeDefined();
	});

	it("permits binary asset metadata only for SHA-256 ids", async () => {
		const asset_id = "a".repeat(64);
		const decoded = await Effect.runPromise(
			DecodeInboundControlEnvelope(
				frontend_envelope("preview.asset.metadata.query", { asset_id }),
			),
		);

		expect(decoded.kind).toBe("preview.asset.metadata.query");
		await expect(
			Effect.runPromise(
				DecodeInboundControlEnvelope(
					frontend_envelope("preview.asset.metadata.query", { asset_id: "asset_1" }),
				),
			),
		).rejects.toBeDefined();
	});

	it("keeps inspection explicit and rejects arbitrary browser commands", async () => {
		const health = await Effect.runPromise(
			Schema.decodeUnknownEffect(PreviewInspectionRequest)({
				operation: "health",
				session_id: "inspection_1",
			}),
		);

		expect(health.operation).toBe("health");
		await expect(
			Effect.runPromise(
				Schema.decodeUnknownEffect(PreviewInspectionRequest)({
					command: "document.cookie",
					session_id: "inspection_1",
				}),
			),
		).rejects.toBeDefined();
	});

	it("accepts target projections that keep browser content out of control frames", async () => {
		const decoded = await Effect.runPromise(
			DecodeOutboundControlEnvelope(
				backend_envelope("preview.target.get.query.result", {
					created_at: timestamp,
					id: "preview_1",
					journal_sequence: 7,
					launch_state: "unavailable",
					port: 5173,
					project_id: "project_1",
					routes: ["/"],
					state: "healthy",
					thread_id: "thread_1",
					updated_at: timestamp,
					url: "http://127.0.0.1:5173/",
					workspace_id: "workspace_1",
				}),
			),
		);

		expect(decoded.kind).toBe("preview.target.get.query.result");
	});

	it("persists transport-safe inspection lifecycle and reconnect state", async () => {
		const session = await Effect.runPromise(
			Schema.decodeUnknownEffect(PreviewInspectionSession)({
				closed_at: timestamp,
				connector_id: "connector_1",
				last_error: "Connector unavailable after restart",
				opened_at: timestamp,
				reconnect_state: "unavailable",
				session_id: "inspection_1",
				state: "abandoned",
				target_id: "preview_1",
				updated_at: timestamp,
			}),
		);

		expect(session).toMatchObject({ reconnect_state: "unavailable", state: "abandoned" });
	});

	it("accepts removed and unhealthy target events with canonical ISO health", async () => {
		const removed = await Effect.runPromise(
			DecodeOutboundControlEnvelope(
				preview_event({
					target: target({ launch_state: "unavailable", state: "removed" }),
					type: "preview.target.updated",
				}),
			),
		);
		const unhealthy = await Effect.runPromise(
			DecodeOutboundControlEnvelope(
				preview_event({
					target: target({
						health: {
							checked_at: timestamp,
							latency_ms: 17,
							message: "Connection refused",
							status: "unhealthy",
						},
						last_error: "Preview server is unavailable",
						launch_state: "error",
						state: "unhealthy",
					}),
					type: "preview.target.updated",
				}),
			),
		);

		expect(removed.kind).toBe("event");
		expect(unhealthy.kind).toBe("event");
	});

	it("accepts reconnecting inspection-session events", async () => {
		const decoded = await Effect.runPromise(
			DecodeOutboundControlEnvelope(
				preview_event({
					session: {
						connector_id: "connector_1",
						last_error: "Connector reconnect pending",
						opened_at: timestamp,
						reconnect_state: "reconnecting",
						session_id: "inspection_1",
						state: "open",
						target_id: "preview_1",
						updated_at: timestamp,
					},
					type: "preview.inspection.updated",
				}),
			),
		);

		expect(decoded.kind).toBe("event");
	});
});
