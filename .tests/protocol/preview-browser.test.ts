import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeInboundControlEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

const timestamp = "2026-07-15T12:00:00.000Z";

const frontend_trace = {
	message_id: "message_1",
	origin: "frontend" as const,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: timestamp,
};

const backend_trace = {
	message_id: "message_2",
	origin: "backend" as const,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: timestamp,
};

const launch_command = {
	project_id: "project_1",
	target_id: "target_1",
	type: "preview.browser.open" as const,
	workspace_id: "workspace_1",
};

const attach_command = {
	connector_id: "connector_1",
	inspection_id: "inspection_1",
	project_id: "project_1",
	target_id: "target_1",
	type: "preview.inspection.attach" as const,
	workspace_id: "workspace_1",
};

const detach_command = {
	inspection_id: "inspection_1",
	project_id: "project_1",
	type: "preview.inspection.detach" as const,
	workspace_id: "workspace_1",
};

const launch = {
	initiator: { kind: "user" as const },
	launch_id: "launch_1",
	project_id: "project_1",
	requested_at_ms: 1_000,
	state: "dispatched" as const,
	target_generation_id: "generation_1",
	target_id: "target_1",
	updated_at_ms: 2_000,
	url: "http://127.0.0.1:4173/app",
	workspace_id: "workspace_1",
};

const inspection = {
	connector_id: "connector_1",
	initiator: { agent_id: "agent_1", kind: "agent" as const },
	inspection_id: "inspection_1",
	project_id: "project_1",
	requested_at_ms: 1_000,
	state: "attached" as const,
	target_generation_id: "generation_1",
	target_id: "target_1",
	updated_at_ms: 2_000,
	url: "https://localhost:5173/app",
	workspace_id: "workspace_1",
};

function command_envelope(payload: unknown) {
	return {
		...frontend_trace,
		causation_id: "cause_1",
		kind: "command" as const,
		payload,
		thread_id: "thread_1",
	};
}

function lifecycle_query() {
	return {
		...frontend_trace,
		kind: "preview.browser.lifecycle.query" as const,
		payload: { project_id: "project_1", workspace_id: "workspace_1" },
	};
}

function lifecycle_result(overrides: Record<string, unknown> = {}) {
	return {
		...backend_trace,
		correlation_id: "query_1",
		kind: "preview.browser.lifecycle.query.result" as const,
		payload: {
			inspections: [inspection],
			launches: [launch],
			project_id: "project_1",
			workspace_id: "workspace_1",
			...overrides,
		},
	};
}

function lifecycle_event(payload: unknown) {
	return {
		...backend_trace,
		causation_id: "cause_1",
		correlation_id: "query_1",
		journal_sequence: 1,
		kind: "event" as const,
		payload,
		sequence: 1,
		stream_id: "preview_1",
		thread_id: "thread_1",
	};
}

async function expect_inbound_rejection(value: unknown) {
	await expect(Effect.runPromise(DecodeInboundControlEnvelope(value))).rejects.toBeDefined();
}

async function expect_outbound_rejection(value: unknown) {
	await expect(Effect.runPromise(DecodeOutboundControlEnvelope(value))).rejects.toBeDefined();
}

describe("preview browser protocol codec", () => {
	it("decodes browser commands, lifecycle query/result, and lifecycle events", async () => {
		for (const payload of [launch_command, attach_command, detach_command]) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(command_envelope(payload))),
			).resolves.toEqual(command_envelope(payload));
		}

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(lifecycle_query())),
		).resolves.toEqual(lifecycle_query());
		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(lifecycle_result())),
		).resolves.toEqual(lifecycle_result());

		for (const payload of [
			{
				action: "dispatched" as const,
				launch,
				type: "preview.browser.launch.updated" as const,
			},
			{
				action: "attached" as const,
				inspection,
				type: "preview.inspection.updated" as const,
			},
		]) {
			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope(lifecycle_event(payload))),
			).resolves.toBeDefined();
		}
	});

	it("rejects private browser transport and page data fields", async () => {
		for (const field of ["endpoint", "token", "cookies", "page_content", "screenshot"]) {
			await expect_inbound_rejection(
				command_envelope({ ...launch_command, [field]: "private" }),
			);
			await expect_outbound_rejection(
				lifecycle_result({ launches: [{ ...launch, [field]: "private" }] }),
			);
		}
	});

	it("rejects incoherent lifecycle states and reasons", async () => {
		for (const invalid_launch of [
			{ ...launch, state: "accepted" as const, reason: "interrupted" as const },
			{ ...launch, state: "rejected" as const },
			{ ...launch, state: "outcome_unknown" as const, reason: "target_changed" as const },
			{ ...launch, updated_at_ms: 999 },
		]) {
			await expect_outbound_rejection(lifecycle_result({ launches: [invalid_launch] }));
		}

		for (const invalid_inspection of [
			{ ...inspection, state: "attached" as const, reason: "detached" as const },
			{ ...inspection, state: "failed" as const },
			{
				...inspection,
				state: "disconnected" as const,
				reason: "connector_unavailable" as const,
			},
			{ ...inspection, updated_at_ms: 999 },
		]) {
			await expect_outbound_rejection(
				lifecycle_result({ inspections: [invalid_inspection] }),
			);
		}

		await expect_outbound_rejection(
			lifecycle_event({
				action: "rejected",
				launch,
				type: "preview.browser.launch.updated",
			}),
		);
		await expect_outbound_rejection(
			lifecycle_event({
				action: "disconnected",
				inspection,
				type: "preview.inspection.updated",
			}),
		);
	});

	it("rejects foreign scopes and duplicate lifecycle identities", async () => {
		for (const overrides of [
			{ launches: [{ ...launch, project_id: "project_2" }] },
			{ inspections: [{ ...inspection, workspace_id: "workspace_2" }] },
			{ launches: [launch, launch] },
			{ inspections: [inspection, inspection] },
		]) {
			await expect_outbound_rejection(lifecycle_result(overrides));
		}
	});

	it("rejects non-loopback URLs in lifecycle records", async () => {
		for (const url of [
			"https://example.com/",
			"http://user:pass@localhost:5173/",
			"http://preview.localhost:5173/",
			"http://127.1.2.999:5173/",
		]) {
			await expect_outbound_rejection(
				lifecycle_result({
					launches: [{ ...launch, url }],
					inspections: [{ ...inspection, url }],
				}),
			);
		}
	});
});
