import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	workspace_git_fetch_default_enabled,
} from "@artisan/protocol";

const timestamp = "2026-07-14T15:00:00.000Z";

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

describe("local Git fetch protocol codec", () => {
	it("keeps the global policy disabled by default", () => {
		expect(workspace_git_fetch_default_enabled).toBe(false);
	});

	it("roundtrips the policy command, manual request, and correlated query result", async () => {
		const update = frontend_envelope("workspace.git.fetch.policy.update", {
			enabled: true,
		});
		const request = {
			...frontend_envelope("workspace.git.fetch.request", {
				workspace_id: "workspace_1",
			}),
			thread_id: "thread_1",
		};
		const query = frontend_envelope("workspace.git.fetch.query", {});
		const result = {
			correlation_id: "query_1",
			kind: "workspace.git.fetch.query.result",
			message_id: "result_1",
			origin: "backend",
			payload: {
				enabled: true,
				workspaces: [
					{ workspace_id: "workspace_1" },
					{
						last_attempt: { attempted_at: timestamp, result: "succeeded" },
						workspace_id: "workspace_2",
					},
				],
			},
			protocol_version: 1,
			schema_version: 1,
			sent_at: timestamp,
		};

		await expect(Effect.runPromise(DecodeInboundControlEnvelope(update))).resolves.toEqual(
			update,
		);
		await expect(Effect.runPromise(DecodeInboundControlEnvelope(request))).resolves.toEqual(
			request,
		);
		await expect(Effect.runPromise(DecodeInboundControlEnvelope(query))).resolves.toEqual(
			query,
		);
		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(result))).resolves.toEqual(
			result,
		);
	});

	it("roundtrips source-safe policy, request, and manual completion events", async () => {
		const payloads = [
			{ enabled: true, type: "workspace.git.fetch.policy.updated" },
			{ type: "workspace.git.fetch.requested", workspace_id: "workspace_1" },
			{
				attempt: { attempted_at: timestamp, result: "failed" },
				type: "workspace.git.fetch.completed",
				workspace_id: "workspace_1",
			},
		];

		for (const [index, payload] of payloads.entries()) {
			const event = {
				causation_id: `cause_${index}`,
				correlation_id: `correlation_${index}`,
				journal_sequence: index + 1,
				kind: "event",
				message_id: `event_${index}`,
				origin: "backend",
				payload,
				protocol_version: 1,
				schema_version: 1,
				sequence: index + 1,
				sent_at: timestamp,
				stream_id: "stream_git_fetch",
				thread_id: "thread_1",
			};

			await expect(Effect.runPromise(DecodeOutboundControlEnvelope(event))).resolves.toEqual(
				event,
			);
		}
	});

	it("rejects cadence, native paths, endpoints, credentials, raw output, and provider fields", async () => {
		const result = {
			correlation_id: "query_1",
			kind: "workspace.git.fetch.query.result",
			message_id: "result_1",
			origin: "backend",
			payload: {
				enabled: false,
				interval_ms: 60_000,
				workspaces: [
					{
						last_attempt: {
							attempted_at: timestamp,
							endpoint: "https://example.test/repo.git",
							password: "secret",
							result: "failed",
							stderr: "raw git output",
						},
						provider_id: "github",
						workspace_id: "workspace_1",
					},
				],
			},
			protocol_version: 1,
			schema_version: 1,
			sent_at: timestamp,
		};

		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(result)),
		).rejects.toBeDefined();
	});

	it("requires the manual request to carry both its thread and workspace targets", async () => {
		const request = frontend_envelope("workspace.git.fetch.request", {
			workspace_id: "workspace_1",
		});

		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(request)),
		).rejects.toBeDefined();
	});
});
