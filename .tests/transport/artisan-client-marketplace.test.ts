import { Deferred, Effect, Queue, Stream } from "effect";
import { describe, expect, it } from "vitest";

import { ProtocolServer, type ProtocolConnection } from "@artisan/backend";
import {
	DecodeInboundControlEnvelope,
	type CapabilityConnectRequestEnvelope,
	type InboundControlEnvelope,
	type OutboundControlEnvelope,
	type RoutineRegistryQueryEnvelope,
} from "@artisan/protocol";

import { MarketplacePendingResultKinds } from "../../modules/transport/src/internal/client-connection";

import { make_transport_test_harness_with_protocol_server } from "./message-channel-harness";

const timestamp = "2026-07-18T08:00:00.000Z";

function make_marketplace_protocol_server() {
	const routine_queries: Array<RoutineRegistryQueryEnvelope> = [];
	const capability_connect_requests: Array<CapabilityConnectRequestEnvelope> = [];
	const capability_actions: Array<InboundControlEnvelope> = [];
	const authority_requests: Array<InboundControlEnvelope> = [];
	let backend_id = 0;

	const backend_trace = () => ({
		message_id: `marketplace_backend_${++backend_id}`,
		origin: "backend" as const,
		protocol_version: 1 as const,
		schema_version: 1 as const,
		sent_at: timestamp,
	});
	const open = Effect.gen(function* () {
		const outbound = yield* Effect.acquireRelease(
			Queue.unbounded<OutboundControlEnvelope>(),
			Queue.shutdown,
		);
		const closed = yield* Deferred.make<void>();
		let negotiated = false;
		const enqueue = (envelope: OutboundControlEnvelope) =>
			Queue.offer(outbound, envelope).pipe(Effect.asVoid);
		const close = Effect.gen(function* () {
			yield* Queue.shutdown(outbound);
			yield* Deferred.succeed(closed, undefined);
		});

		yield* Effect.addFinalizer(() => close);
		const handle = (input: InboundControlEnvelope) => {
			if (input.kind === "hello") {
				negotiated = true;
				return enqueue({
					...backend_trace(),
					correlation_id: input.message_id,
					kind: "welcome",
					payload: {
						connection_id: "marketplace_connection",
						current_event_cursors: [],
						heartbeat_interval_ms: 15_000,
						heartbeat_timeout_ms: 45_000,
						journal_sequence: 4,
						stream_ticket: "marketplace_stream",
					},
				});
			}

			if (!negotiated) return Effect.void;
			if (input.kind === "marketplace.routine.list.query") {
				routine_queries.push(input);
				return enqueue({
					...backend_trace(),
					correlation_id: input.message_id,
					kind: "marketplace.routine.list.query.result",
					payload: {
						registry_version: 1,
						routines: [
							{
								description: "A safe routine",
								display_name: "Routine",
								enabled: true,
								id: "routine_1",
								scope: { kind: "global" },
								status: "enabled",
								version: "1.0.0",
							},
						],
					},
				});
			}
			if (input.kind === "marketplace.capability.connect.request") {
				capability_connect_requests.push(input);
				return enqueue({
					...backend_trace(),
					causation_id: input.message_id,
					correlation_id: input.message_id,
					kind: "command.receipt",
					payload: { journal_sequence: 5, status: "accepted" },
					thread_id: "marketplace",
				});
			}
			if (input.kind === "marketplace.capability.invoke") {
				capability_actions.push(input);
				return enqueue({
					...backend_trace(),
					correlation_id: input.message_id,
					kind: "marketplace.capability.invoke.result",
					payload: {
						approval_required: false,
						capability_id: input.payload.capability_id,
						invocation_id: "invocation_visible",
						status: "completed",
						tool_name: input.payload.tool_name,
					},
				});
			}
			if (
				input.kind === "marketplace.capability.invoke.request" ||
				input.kind === "marketplace.capability.invoke.decision"
			) {
				authority_requests.push(input);
				return enqueue({
					...backend_trace(),
					correlation_id: input.message_id,
					kind: "marketplace.capability.invoke.result",
					payload: {
						approval_required: true,
						capability_id: input.payload.capability_id,
						invocation_id: "invocation_approval",
						status:
							input.kind === "marketplace.capability.invoke.request"
								? "requested"
								: input.payload.approved
									? "completed"
									: "denied",
						tool_name: input.payload.tool_name,
					},
				});
			}
			if (input.kind === "marketplace.capability.oauth.status.query") {
				capability_actions.push(input);
				return enqueue({
					...backend_trace(),
					correlation_id: input.message_id,
					kind: "marketplace.capability.oauth.status.query.result",
					payload: {
						capability_id: input.payload.capability_id,
						status: "authorized",
					},
				});
			}
			if (input.kind === "marketplace.capability.oauth.begin") {
				capability_actions.push(input);
				return enqueue({
					...backend_trace(),
					correlation_id: input.message_id,
					kind: "marketplace.capability.oauth.begin.result",
					payload: {
						authorization_url: "https://auth.example.test/authorize",
						continuation_reference: "oauth-continuation",
					},
				});
			}
			if (
				input.kind === "marketplace.capability.start" ||
				input.kind === "marketplace.capability.reconnect" ||
				input.kind === "marketplace.capability.restart" ||
				input.kind === "marketplace.capability.oauth.complete" ||
				input.kind === "marketplace.capability.oauth.refresh" ||
				input.kind === "marketplace.capability.oauth.revoke"
			) {
				capability_actions.push(input);
				return enqueue({
					...backend_trace(),
					causation_id: input.message_id,
					correlation_id: input.message_id,
					kind: "command.receipt",
					payload: { journal_sequence: 7, status: "accepted" },
					thread_id: "marketplace",
				});
			}
			if (
				input.kind === "marketplace.routine.drift.overwrite.request" ||
				input.kind === "marketplace.routine.drift.overwrite.decision" ||
				input.kind === "marketplace.capability.drift.overwrite.request" ||
				input.kind === "marketplace.capability.drift.overwrite.decision"
			) {
				authority_requests.push(input);
				return enqueue({
					...backend_trace(),
					causation_id: input.message_id,
					correlation_id: input.message_id,
					kind: "command.receipt",
					payload: { journal_sequence: 6, status: "accepted" },
					thread_id: "marketplace",
				});
			}

			return Effect.void;
		};
		const connection: ProtocolConnection = {
			Close: close,
			Closed: Deferred.await(closed),
			Outbound: Stream.fromQueue(outbound),
			Receive: (input) =>
				DecodeInboundControlEnvelope(input).pipe(
					Effect.flatMap(handle),
					Effect.catch(() => Effect.void),
				),
		};

		return connection;
	});

	return {
		server: { Open: open } satisfies typeof ProtocolServer.Service,
		snapshot: () => ({
			authority_requests: [...authority_requests],
			capability_actions: [...capability_actions],
			capability_connect_requests: [...capability_connect_requests],
			routine_queries: [...routine_queries],
		}),
	};
}

describe("ArtisanClient Marketplace surface", () => {
	it.each([
		"marketplace.routine.list.query.result",
		"marketplace.routine.detail.query.result",
		"marketplace.routine.install.preview.result",
		"marketplace.routine.invoke.result",
		"marketplace.npx_skills.discover.result",
		"marketplace.capability.list.query.result",
		"marketplace.capability.detail.query.result",
		"marketplace.capability.connect.preview.result",
		"marketplace.capability.invoke.result",
		"marketplace.capability.oauth.status.query.result",
	] as const)("routes the correlated %s envelope through the request coordinator", (kind) => {
		expect(MarketplacePendingResultKinds).toContain(kind);
	});

	it("sends filtered browse and approval-bound connection envelopes with exact correlations", async () => {
		const protocol = make_marketplace_protocol_server();
		const harness = await make_transport_test_harness_with_protocol_server(protocol.server);

		try {
			const routines = await Effect.runPromise(
				harness.client.ListRoutines({
					enabled: true,
					scope: { kind: "global" },
					text: "safe",
				}),
			);
			const receipt = await Effect.runPromise(
				harness.client.RequestCapabilityConnect({
					approval_id: "approval_1",
					auth: { kind: "none" },
					preview_fingerprint: "preview_1",
					requested_by: "user",
					scope: { kind: "global" },
					source: { kind: "catalog", locator: "https://catalog.example/capability" },
					transport: { kind: "streamable_http", url: "https://mcp.example" },
				}),
			);
			const snapshot = protocol.snapshot();

			expect(routines.routines).toHaveLength(1);
			expect(snapshot.routine_queries[0]).toMatchObject({
				kind: "marketplace.routine.list.query",
				payload: { enabled: true, scope: { kind: "global" }, text: "safe" },
			});
			expect(snapshot.capability_connect_requests[0]).toMatchObject({
				kind: "marketplace.capability.connect.request",
				payload: {
					approval_id: "approval_1",
					preview_fingerprint: "preview_1",
					requested_by: "user",
				},
			});
			expect(receipt).toMatchObject({
				command_id: snapshot.capability_connect_requests[0]?.message_id,
				journal_sequence: 5,
				status: "accepted",
			});
			expect(JSON.stringify(snapshot)).not.toMatch(/secret|token|process|registry/i);
		} finally {
			await harness.dispose();
		}
	});

	it("sends immutable two-phase invocation and drift-overwrite intents", async () => {
		const protocol = make_marketplace_protocol_server();
		const harness = await make_transport_test_harness_with_protocol_server(protocol.server);
		const scope = { kind: "workspace" as const, workspace_id: "workspace_1" };

		try {
			await Effect.runPromise(
				harness.client.RequestRoutineDriftOverwrite({
					approval_id: "approval_routine_drift",
					engine_id: "codex",
					intent_fingerprint: "routine_drift_fingerprint",
					observed_revision: "revision_1",
					requested_by: "user",
					routine_id: "routine_1",
					scope,
				}),
			);
			const requested = await Effect.runPromise(
				harness.client.RequestCapabilityInvocation({
					approval_id: "approval_invoke",
					arguments_json: '{"path":"README.md"}',
					capability_id: "cap_files",
					intent_fingerprint: "invoke_fingerprint",
					requested_by: "user",
					scope,
					tool_name: "read_file",
				}),
			);
			const decided = await Effect.runPromise(
				harness.client.DecideCapabilityInvocation({
					approval_id: "approval_invoke",
					approved: true,
					arguments_json: '{"path":"README.md"}',
					capability_id: "cap_files",
					intent_fingerprint: "invoke_fingerprint",
					scope,
					tool_name: "read_file",
				}),
			);
			expect(requested).toMatchObject({
				approval_required: true,
				invocation_id: "invocation_approval",
				status: "requested",
			});
			expect(decided).toMatchObject({
				approval_required: true,
				invocation_id: "invocation_approval",
				status: "completed",
			});

			expect(protocol.snapshot().authority_requests).toMatchObject([
				{
					kind: "marketplace.routine.drift.overwrite.request",
					payload: { approval_id: "approval_routine_drift", scope },
				},
				{
					kind: "marketplace.capability.invoke.request",
					payload: {
						arguments_json: '{"path":"README.md"}',
						intent_fingerprint: "invoke_fingerprint",
						scope,
					},
				},
				{
					kind: "marketplace.capability.invoke.decision",
					payload: {
						arguments_json: '{"path":"README.md"}',
						intent_fingerprint: "invoke_fingerprint",
						scope,
					},
				},
			]);
		} finally {
			await harness.dispose();
		}
	});

	it("correlates capability lifecycle, invocation visibility, and opaque OAuth operations", async () => {
		const protocol = make_marketplace_protocol_server();
		const harness = await make_transport_test_harness_with_protocol_server(protocol.server);
		const scope = { kind: "workspace" as const, workspace_id: "workspace_1" };

		try {
			const [
				start,
				reconnect,
				restart,
				invoked,
				begin,
				complete,
				refreshed,
				revoked,
				status,
			] = await Effect.runPromise(
				Effect.all([
					harness.client.StartCapability({ capability_id: "cap_files", scope }),
					harness.client.ReconnectCapability({ capability_id: "cap_files", scope }),
					harness.client.RestartCapability({ capability_id: "cap_files", scope }),
					harness.client.InvokeCapability({
						arguments_json: '{"path":"README.md"}',
						capability_id: "cap_files",
						scope,
						tool_name: "read_file",
					}),
					harness.client.BeginCapabilityOAuth({ capability_id: "cap_files", scope }),
					harness.client.CompleteCapabilityOAuth({
						callback_reference: "callback_opaque",
						capability_id: "cap_files",
						scope,
					}),
					harness.client.RefreshCapabilityOAuth({ capability_id: "cap_files", scope }),
					harness.client.RevokeCapabilityOAuth({ capability_id: "cap_files", scope }),
					harness.client.GetCapabilityOAuthStatus({ capability_id: "cap_files", scope }),
				]),
			);
			const snapshot = protocol.snapshot();

			expect([start, reconnect, restart, complete, refreshed, revoked]).toEqual(
				expect.arrayContaining([
					expect.objectContaining({ journal_sequence: 7, status: "accepted" }),
				]),
			);
			expect(begin).toEqual({
				authorization_url: "https://auth.example.test/authorize",
				continuation_reference: "oauth-continuation",
			});
			expect(invoked).toEqual({
				approval_required: false,
				capability_id: "cap_files",
				invocation_id: "invocation_visible",
				status: "completed",
				tool_name: "read_file",
			});
			expect(status).toEqual({ capability_id: "cap_files", status: "authorized" });
			expect(snapshot.capability_actions.map((action) => action.kind)).toEqual(
				expect.arrayContaining([
					"marketplace.capability.start",
					"marketplace.capability.reconnect",
					"marketplace.capability.restart",
					"marketplace.capability.invoke",
					"marketplace.capability.oauth.begin",
					"marketplace.capability.oauth.complete",
					"marketplace.capability.oauth.refresh",
					"marketplace.capability.oauth.revoke",
					"marketplace.capability.oauth.status.query",
				]),
			);
			expect(JSON.stringify(snapshot)).not.toMatch(
				/(access_token|authorization_code|refresh_token|secret_ref)/i,
			);
		} finally {
			await harness.dispose();
		}
	});
});
