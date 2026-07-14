import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeInboundControlEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

const timestamp = "2026-07-14T15:00:00.000Z";
const head = "a".repeat(40);
const fingerprint = "b".repeat(64);

function frontend_envelope(kind: string, payload: unknown, thread_id?: string) {
	return {
		kind,
		message_id: `message_${kind}`,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: timestamp,
		...(thread_id === undefined ? {} : { thread_id }),
	};
}

function snapshot() {
	return {
		baseline_fingerprint: fingerprint,
		created_at: timestamp,
		gates: [
			{ _tag: "required_checks_terminal" },
			{ _tag: "selected_checks_terminal", check_names: ["lint", "test"] },
			{ _tag: "review_decision_changed" },
		],
		generation: 1,
		maximum_generation: 3,
		owner: {
			_tag: "assignment_run",
			agent_id: "agent_1",
			assignment_id: "assignment_1",
			engine_id: "engine_1",
			group_id: "group_1",
			run_id: "run_1",
		},
		project_id: "project_1",
		state: {
			_tag: "wake_pending",
			trigger: {
				_tag: "checks_terminal",
				check_summaries: [
					{ name: "lint", required: true, state: "passed", workflow_name: "CI" },
				],
				truncated: false,
			},
		},
		target: {
			branch: "feature/wait",
			expected_head_commit: head,
			pull_request_number: 42,
			pull_request_origin: {
				native_id: "PR_42",
				provider_id: "github",
				resource_kind: "pull_request",
			},
			repository: {
				host: "github.com",
				name: "editor",
				owner: "artisan",
				provider_id: "github",
			},
		},
		thread_id: "thread_1",
		updated_at: timestamp,
		version: 1,
		wait_id: "wait_1",
		workspace_id: "workspace_1",
		journal_sequence: 7,
	};
}

describe("External wait protocol codec", () => {
	it("roundtrips every external-wait envelope and the durable event", async () => {
		const inbound = [
			frontend_envelope(
				"external_wait.request",
				{
					expected_head_commit: head,
					gates: [{ _tag: "required_checks_terminal" }],
					pull_request_number: 42,
					source_run_id: "run_1",
					workspace_id: "workspace_1",
				},
				"thread_1",
			),
			frontend_envelope("external_wait.cancel", { wait_id: "wait_1" }, "thread_1"),
			frontend_envelope("external_wait.manual_resume", { wait_id: "wait_1" }, "thread_1"),
			frontend_envelope("external_wait.query", { thread_id: "thread_1" }),
		];
		const outbound = [
			{
				correlation_id: "query_1",
				kind: "external_wait.query.result",
				message_id: "query_result_1",
				origin: "backend",
				payload: { snapshots: [snapshot()], truncated: false },
				protocol_version: 1,
				schema_version: 1,
				sent_at: timestamp,
			},
			{
				causation_id: "request_1",
				correlation_id: "request_1",
				journal_sequence: 7,
				kind: "event",
				message_id: "event_1",
				origin: "backend",
				payload: { snapshot: snapshot(), type: "external_wait.updated" },
				protocol_version: 1,
				schema_version: 1,
				sequence: 1,
				sent_at: timestamp,
				stream_id: "thread:thread_1",
				thread_id: "thread_1",
			},
		];

		for (const envelope of inbound) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}

		for (const envelope of outbound) {
			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope(envelope)),
			).resolves.toEqual(envelope);
		}
	});

	it("rejects empty and duplicate gates and selected checks", async () => {
		const request = {
			expected_head_commit: head,
			gates: [{ _tag: "required_checks_terminal" }],
			pull_request_number: 42,
			source_run_id: "run_1",
			workspace_id: "workspace_1",
		};

		for (const gates of [
			[],
			[{ _tag: "required_checks_terminal" }, { _tag: "required_checks_terminal" }],
			[
				{ _tag: "selected_checks_terminal", check_names: ["lint"] },
				{ _tag: "selected_checks_terminal", check_names: ["test"] },
			],
			[{ _tag: "selected_checks_terminal", check_names: ["lint", "lint"] }],
		]) {
			await expect(
				Effect.runPromise(
					DecodeInboundControlEnvelope({
						...frontend_envelope(
							"external_wait.request",
							{ ...request, gates },
							"thread_1",
						),
					}),
				),
			).rejects.toBeDefined();
		}
	});

	it("rejects nonterminal or empty check-transition evidence", async () => {
		for (const trigger of [
			{ _tag: "checks_terminal", check_summaries: [], truncated: false },
			{
				_tag: "checks_terminal",
				check_summaries: [{ name: "lint", required: true, state: "running" }],
				truncated: false,
			},
			{
				_tag: "checks_terminal",
				check_summaries: [{ name: "lint", required: true, state: "passed" }],
				truncated: true,
			},
		]) {
			const invalid_snapshot = {
				...snapshot(),
				state: {
					_tag: "wake_pending",
					trigger,
				},
			};

			await expect(
				Effect.runPromise(
					DecodeOutboundControlEnvelope({
						correlation_id: "query_1",
						kind: "external_wait.query.result",
						message_id: "query_result_1",
						origin: "backend",
						payload: { snapshots: [invalid_snapshot], truncated: false },
						protocol_version: 1,
						schema_version: 1,
						sent_at: timestamp,
					}),
				),
			).rejects.toBeDefined();
		}
	});

	it("rejects malformed head and baseline fingerprint values", async () => {
		await expect(
			Effect.runPromise(
				DecodeInboundControlEnvelope(
					frontend_envelope(
						"external_wait.request",
						{
							expected_head_commit: "A".repeat(40),
							gates: [{ _tag: "required_checks_terminal" }],
							pull_request_number: 42,
							source_run_id: "run_1",
							workspace_id: "workspace_1",
						},
						"thread_1",
					),
				),
			),
		).rejects.toBeDefined();

		const invalid_snapshot = { ...snapshot(), baseline_fingerprint: "g".repeat(64) };

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					correlation_id: "query_1",
					kind: "external_wait.query.result",
					message_id: "query_result_1",
					origin: "backend",
					payload: { snapshots: [invalid_snapshot], truncated: false },
					protocol_version: 1,
					schema_version: 1,
					sent_at: timestamp,
				}),
			),
		).rejects.toBeDefined();
	});

	it("rejects provider-derived owner and target fields in a request", async () => {
		const request = {
			expected_head_commit: head,
			gates: [{ _tag: "required_checks_terminal" }],
			owner: {
				_tag: "thread_run",
				agent_id: "agent_1",
				engine_id: "engine_1",
				run_id: "run_1",
			},
			pull_request_number: 42,
			source_run_id: "run_1",
			target: { branch: "main" },
			workspace_id: "workspace_1",
		};

		await expect(
			Effect.runPromise(
				DecodeInboundControlEnvelope(
					frontend_envelope("external_wait.request", request, "thread_1"),
				),
			),
		).rejects.toBeDefined();
	});

	it("rejects a target whose origin and repository providers disagree", async () => {
		const invalid_snapshot = {
			...snapshot(),
			target: {
				...snapshot().target,
				pull_request_origin: {
					...snapshot().target.pull_request_origin,
					provider_id: "gitlab",
				},
			},
		};

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					correlation_id: "query_1",
					kind: "external_wait.query.result",
					message_id: "query_result_1",
					origin: "backend",
					payload: { snapshots: [invalid_snapshot], truncated: false },
					protocol_version: 1,
					schema_version: 1,
					sent_at: timestamp,
				}),
			),
		).rejects.toBeDefined();
	});

	it("rejects duplicate waits or snapshots from different threads in one query result", async () => {
		for (const snapshots of [
			[snapshot(), { ...snapshot(), version: 2 }],
			[snapshot(), { ...snapshot(), thread_id: "thread_2", wait_id: "wait_2" }],
		]) {
			await expect(
				Effect.runPromise(
					DecodeOutboundControlEnvelope({
						correlation_id: "query_1",
						kind: "external_wait.query.result",
						message_id: "query_result_1",
						origin: "backend",
						payload: { snapshots, truncated: false },
						protocol_version: 1,
						schema_version: 1,
						sent_at: timestamp,
					}),
				),
			).rejects.toBeDefined();
		}
	});
});
