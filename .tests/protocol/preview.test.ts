import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeInboundControlEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

const timestamp = "2026-07-15T12:00:00.000Z";

const trace = {
	message_id: "message_1",
	origin: "frontend" as const,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: timestamp,
};

const register_payload = {
	project_id: "project_1",
	source: { kind: "terminal" as const, terminal_id: "terminal_1" },
	target_id: "target_1",
	type: "preview.target.register" as const,
	url: "http://localhost:5173/app",
	workspace_id: "workspace_1",
};

const record = {
	created_at_ms: 1_000,
	health: {
		checked_at_ms: 2_000,
		latency_ms: 12,
		message: "ready",
		status: "healthy" as const,
		status_code: 200,
	},
	project_id: "project_1",
	source: { kind: "process" as const, process_id: "process_1" },
	state: "healthy" as const,
	target_id: "target_1",
	updated_at_ms: 2_000,
	url: "http://127.0.0.1:4173/",
	workspace_id: "workspace_1",
};

function command_envelope(payload: unknown) {
	return {
		...trace,
		causation_id: "cause_1",
		kind: "command" as const,
		payload,
		thread_id: "thread_1",
	};
}

describe("preview protocol codec", () => {
	it("decodes source-safe registration, probe, removal, query, result, and update frames", async () => {
		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(command_envelope(register_payload))),
		).resolves.toEqual(command_envelope(register_payload));

		for (const url of [
			"http://localhost:5173/",
			"https://127.0.0.1:4173/",
			"http://127.255.255.255:8080/",
			"http://[::1]:3000/",
		]) {
			await expect(
				Effect.runPromise(
					DecodeInboundControlEnvelope(command_envelope({ ...register_payload, url })),
				),
			).resolves.toBeDefined();
		}

		for (const payload of [
			{
				project_id: "project_1",
				target_id: "target_1",
				type: "preview.target.probe" as const,
				workspace_id: "workspace_1",
			},
			{
				project_id: "project_1",
				target_id: "target_1",
				type: "preview.target.remove" as const,
				workspace_id: "workspace_1",
			},
		]) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(command_envelope(payload))),
			).resolves.toEqual(command_envelope(payload));
		}

		const query = {
			...trace,
			kind: "preview.targets.query" as const,
			payload: { project_id: "project_1", workspace_id: "workspace_1" },
		};
		const result = {
			message_id: "result_1",
			origin: "backend" as const,
			protocol_version: 1 as const,
			schema_version: 1 as const,
			sent_at: timestamp,
			correlation_id: "query_1",
			kind: "preview.targets.query.result" as const,
			payload: {
				project_id: "project_1",
				targets: [record],
				workspace_id: "workspace_1",
			},
		};

		await expect(Effect.runPromise(DecodeInboundControlEnvelope(query))).resolves.toEqual(
			query,
		);
		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(result))).resolves.toEqual(
			result,
		);
		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					...result,
					kind: "event",
					causation_id: "cause_1",
					correlation_id: "query_1",
					journal_sequence: 1,
					payload: {
						action: "removed",
						target: { ...record, state: "removed" },
						type: "preview.target.updated",
					},
					sequence: 1,
					stream_id: "preview_1",
					thread_id: "thread_1",
				}),
			),
		).resolves.toBeDefined();
	});

	it("rejects credentials, non-local URLs, malformed identities, unsafe health, and excess fields", async () => {
		const invalid_payloads = [
			{ ...register_payload, url: "http://user:pass@localhost:5173/" },
			{ ...register_payload, url: "http://preview.localhost:5173/" },
			{ ...register_payload, url: "https://example.com/" },
			{ ...register_payload, url: "http://127.999.1.1:5173/" },
			{ ...register_payload, url: "http://127.1.2.999:5173/" },
			{ ...register_payload, target_id: "target id" },
			{ ...register_payload, source: { kind: "terminal", terminal_id: "terminal id" } },
			{ ...register_payload, extra: true },
		];

		for (const payload of invalid_payloads) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(command_envelope(payload))),
			).rejects.toBeDefined();
		}

		for (const envelope of [
			{ ...command_envelope(register_payload), message_id: "message id" },
			{ ...command_envelope(register_payload), origin: "backend" },
			{ ...command_envelope(register_payload), protocol_version: 2 },
		]) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).rejects.toBeDefined();
		}

		const invalid_records = [
			{ ...record, health: { ...record.health, status_code: 99 } },
			{ ...record, health: { ...record.health, message: "\u0000unsafe" } },
			{ ...record, updated_at_ms: 4_102_444_800_001 },
			{ ...record, extra: true },
		];

		for (const target of invalid_records) {
			await expect(
				Effect.runPromise(
					DecodeOutboundControlEnvelope({
						correlation_id: "query_1",
						kind: "preview.targets.query.result",
						message_id: "result_1",
						origin: "backend",
						payload: {
							project_id: "project_1",
							targets: [target],
							workspace_id: "workspace_1",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: timestamp,
					}),
				),
			).rejects.toBeDefined();
		}

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					correlation_id: "query_1",
					kind: "preview.targets.query.result",
					message_id: "result_1",
					origin: "backend",
					payload: {
						project_id: "project_1",
						targets: [{ ...record, state: "removed" }],
						workspace_id: "workspace_1",
					},
					protocol_version: 1,
					schema_version: 1,
					sent_at: timestamp,
				}),
			),
		).rejects.toBeDefined();

		for (const targets of [
			[record, record],
			[record, { ...record, project_id: "project_2", target_id: "target_2" }],
			[record, { ...record, target_id: "target_2", workspace_id: "workspace_2" }],
			Array.from({ length: 257 }, (_, index) => ({
				...record,
				target_id: `target_${index}`,
			})),
		]) {
			await expect(
				Effect.runPromise(
					DecodeOutboundControlEnvelope({
						correlation_id: "query_1",
						kind: "preview.targets.query.result",
						message_id: "result_1",
						origin: "backend",
						payload: {
							project_id: "project_1",
							targets,
							workspace_id: "workspace_1",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: timestamp,
					}),
				),
			).rejects.toBeDefined();
		}

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					...trace,
					causation_id: "cause_1",
					correlation_id: "query_1",
					kind: "event",
					journal_sequence: 1,
					origin: "backend",
					payload: {
						action: "removed",
						target: record,
						type: "preview.target.updated",
					},
					sequence: 1,
					stream_id: "preview_1",
					thread_id: "thread_1",
				}),
			),
		).rejects.toBeDefined();
	});

	it("keeps command envelope identity outside the generic receipt payload", async () => {
		const envelope = command_envelope(register_payload);

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope({ ...envelope, message_id: "retry_2" })),
		).resolves.toMatchObject({ message_id: "retry_2", payload: register_payload });

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					correlation_id: "retry_2",
					causation_id: "retry_2",
					kind: "command.receipt",
					message_id: "receipt_1",
					origin: "backend",
					payload: { journal_sequence: 9, status: "accepted" },
					protocol_version: 1,
					schema_version: 1,
					sent_at: timestamp,
					thread_id: "thread_1",
				}),
			),
		).resolves.toMatchObject({ payload: { status: "accepted" } });
	});
});
