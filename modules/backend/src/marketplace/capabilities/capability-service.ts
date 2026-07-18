import { createHash } from "node:crypto";

import { Context, Data, Effect, Layer, Schema, Scope } from "effect";
import {
	CapabilityDetail,
	CapabilityConnectPreview,
	CapabilityInvocationApprovalDecision,
	CapabilityInvocationApprovalRequest,
	CapabilityInvocationMetadata,
	CapabilityInvocationRequest,
	MarketplaceScope,
} from "@artisan/protocol";

import { CapabilityTransportRegistry, type McpClientSession } from "./mcp-transport";
import { inspect_http_mcp_endpoint } from "./http-transport";
import { CapabilityRepository } from "./capability-repository";
import {
	OAuth,
	type OAuthBeginInput,
	type OAuthCompletionInput,
	type OAuthTokenStatus,
} from "./oauth";

export class CapabilityServiceError extends Data.TaggedError("CapabilityServiceError")<{
	readonly code:
		| "approval_required"
		| "connection_in_progress"
		| "disabled"
		| "invocation_in_progress"
		| "not_connected"
		| "policy_denied"
		| "preview_changed"
		| "removed";
	readonly message: string;
}> {}

const Fingerprint = (value: unknown) =>
	createHash("sha256").update(JSON.stringify(value)).digest("hex");

const ScopeMatches = (left: MarketplaceScope, right: MarketplaceScope) =>
	JSON.stringify(left) === JSON.stringify(right);

/** Approval-gated live capability coordinator. It intentionally has no startup behavior. */
export class CapabilityService extends Context.Service<
	CapabilityService,
	{
		readonly Preview: (
			input: Pick<CapabilityDetail, "auth" | "scope" | "source" | "transport">,
		) => Effect.Effect<CapabilityConnectPreview>;
		readonly RequestConnect: (input: {
			readonly approval_id: string;
			readonly detail: CapabilityDetail;
			readonly operation_id: string;
			readonly preview_fingerprint: string;
			readonly request_fingerprint: string;
		}) => Effect.Effect<void, unknown>;
		readonly DecideConnect: (input: {
			readonly approval_id: string;
			readonly approved: boolean;
			readonly approval_fingerprint: string;
		}) => Effect.Effect<CapabilityDetail, unknown>;
		readonly Invoke: (
			input: CapabilityInvocationRequest & {
				readonly operation_id: string;
			},
		) => Effect.Effect<CapabilityInvocationMetadata, unknown>;
		readonly RequestInvocation: (
			input: CapabilityInvocationApprovalRequest & { readonly operation_id: string },
		) => Effect.Effect<CapabilityInvocationMetadata, unknown>;
		readonly DecideInvocation: (
			input: CapabilityInvocationApprovalDecision,
		) => Effect.Effect<CapabilityInvocationMetadata, unknown>;
		readonly Health: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<CapabilityDetail, unknown>;
		readonly Disconnect: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<void, unknown>;
		readonly Enable: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<void, unknown>;
		readonly Disable: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<void, unknown>;
		readonly Remove: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<void, unknown>;
		readonly Uninstall: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<void, unknown>;
		readonly SessionAction: (input: {
			readonly action: "start" | "reconnect" | "restart";
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<CapabilityDetail, unknown>;
	}
>()("Artisan/Marketplace/CapabilityService") {}

export const CapabilityServiceLive = Layer.effect(
	CapabilityService,
	Effect.gen(function* () {
		const repository = yield* CapabilityRepository;
		const transports = yield* CapabilityTransportRegistry;
		const scope = yield* Scope.Scope;
		const sessions = new Map<string, McpClientSession>();
		const RequireMutable = (capability_id: string) =>
			repository.ReadDetail(capability_id).pipe(
				Effect.filterOrFail(
					(detail) => detail.lifecycle !== "removed" && detail.status !== "removed",
					() =>
						new CapabilityServiceError({
							code: "removed",
							message: "Removed capabilities cannot be mutated or reconnected",
						}),
				),
			);
		const MonitorSession = (
			capability_id: string,
			operation_id: string,
			session: McpClientSession,
		) =>
			Effect.forever(
				Effect.sleep(1_000).pipe(
					Effect.andThen(
						Effect.gen(function* () {
							if (sessions.get(capability_id) !== session)
								return yield* Effect.fail("session_replaced");
							const health = yield* session.Health.pipe(
								Effect.catch(() => Effect.succeed("crashed" as const)),
							);
							if (health === "connected") return;
							sessions.delete(capability_id);
							const detail = yield* repository.ReadDetail(capability_id);
							yield* repository.Transition({
								capability_id,
								health: { status: health === "unhealthy" ? "degraded" : "crashed" },
								lifecycle: "crashed",
								operation: "health_checked",
								operation_id: `${operation_id}:crash`,
								status: detail.status,
							});
							return yield* Effect.fail("session_ended");
						}),
					),
				),
			).pipe(
				Effect.catch(() => Effect.void),
				Effect.forkIn(scope),
			);
		const Preview = (
			input: Pick<CapabilityDetail, "auth" | "scope" | "source" | "transport">,
		) => {
			const reviewed = {
				auth: input.auth,
				scope: input.scope,
				source: input.source,
				transport: input.transport,
			};
			return Effect.succeed({
				auth: input.auth,
				candidate_id: `capability_${Fingerprint(reviewed).slice(0, 24)}`,
				candidate_name: input.source.locator,
				compatibility: [{ engine_id: "codex", state: "runtime_only" }],
				discovery_status: "requires_connection",
				permissions: [
					input.transport.kind === "stdio"
						? {
								description: "Starts the reviewed local MCP command.",
								kind: "process" as const,
							}
						: {
								description: `Connects to ${new URL(input.transport.url).origin}.`,
								kind: "network" as const,
							},
					...(input.transport.kind === "stdio" && input.transport.env?.length
						? [
								{
									description: "Reads approved environment secret references.",
									kind: "environment" as const,
								},
							]
						: []),
					...(input.auth.kind === "none"
						? []
						: [
								{
									description: "Uses an approved account credential reference.",
									kind: "account" as const,
								},
							]),
				],
				preview_fingerprint: Fingerprint(reviewed),
				rollback_available: false,
				scope: input.scope,
				source: input.source,
				tools: [],
				transport: input.transport,
				...(input.transport.kind === "streamable_http"
					? {
							transport_policy: inspect_http_mcp_endpoint({
								max_response_bytes: input.transport.max_response_bytes ?? 1_048_576,
								timeout_ms: input.transport.timeout_ms ?? 30_000,
								url: input.transport.url,
							}),
						}
					: {}),
				trust:
					input.source.kind === "local" || input.source.kind === "plugin_bundle"
						? "local"
						: "unverified",
			} satisfies CapabilityConnectPreview);
		};
		const RequestConnect = (input: {
			readonly approval_id: string;
			readonly detail: CapabilityDetail;
			readonly operation_id: string;
			readonly preview_fingerprint: string;
			readonly request_fingerprint: string;
		}) =>
			Effect.gen(function* () {
				const preview = yield* Preview(input.detail);
				if (preview.preview_fingerprint !== input.preview_fingerprint)
					return yield* new CapabilityServiceError({
						code: "preview_changed",
						message: "Capability preview changed; approval must be renewed",
					});
				yield* repository.RecordConnectRequest({
					approval_fingerprint: input.preview_fingerprint,
					approval_id: input.approval_id,
					capability_id: input.detail.id,
					detail: input.detail,
					operation_id: input.operation_id,
					request_fingerprint: input.request_fingerprint,
				});
			});
		const ExecuteApprovedConnect = (operation_id: string, reviewed_detail: CapabilityDetail) =>
			Effect.gen(function* () {
				const claim = yield* repository.ClaimConnect(operation_id);
				if (claim === "connected") return yield* repository.ReadDetail(reviewed_detail.id);
				if (claim === "connecting")
					return yield* new CapabilityServiceError({
						code: "connection_in_progress",
						message: "A connection with this operation id is already being established",
					});
				if (claim === "denied")
					return yield* new CapabilityServiceError({
						code: "approval_required",
						message: "Capability connection was denied before any transport action",
					});
				const session = yield* transports
					.Connect(reviewed_detail)
					.pipe(Effect.provideService(Scope.Scope, scope));
				sessions.set(reviewed_detail.id, session);
				const initialized = yield* session.Initialize;
				const [tools, resources] = yield* Effect.all([
					session.ListTools,
					session.ListResources,
				]);
				const permissions = [
					...(reviewed_detail.transport.kind === "stdio"
						? [
								{
									description: "Start the configured MCP server process",
									kind: "process" as const,
								},
							]
						: [
								{
									description: "Connect to the configured MCP server endpoint",
									kind: "network" as const,
								},
							]),
					...(reviewed_detail.auth.kind === "none"
						? []
						: [
								{
									description: "Use the configured account credential",
									kind: "account" as const,
								},
							]),
				];
				const discovered_tools = new Map(
					reviewed_detail.tools.map((tool) => [tool.name, tool] as const),
				);
				for (const tool of tools)
					discovered_tools.set(tool.name, {
						...(tool.description ? { description: tool.description } : {}),
						input_schema: tool.input_schema,
						name: tool.name,
					});
				const discovered_resources = new Map(
					reviewed_detail.resources.map((resource) => [resource.uri, resource] as const),
				);
				for (const resource of resources)
					discovered_resources.set(resource.uri, {
						...(resource.description ? { description: resource.description } : {}),
						uri: resource.uri,
					});
				const discovered_detail = yield* Schema.decodeUnknownEffect(CapabilityDetail)({
					...reviewed_detail,
					health: { status: "healthy" },
					lifecycle: "connected",
					permissions,
					...(initialized.instructions === undefined
						? {}
						: { server_instructions: initialized.instructions }),
					resources: [...discovered_resources.values()],
					status: reviewed_detail.enabled ? "enabled" : "disabled",
					tools: [...discovered_tools.values()],
				});
				yield* repository
					.Create({
						detail: discovered_detail,
						operation_id,
						request_fingerprint: operation_id,
						server_metadata: {
							protocol_version: initialized.protocol_version,
							server_name: initialized.server_name,
							...(initialized.server_version === undefined
								? {}
								: { server_version: initialized.server_version }),
						},
					})
					.pipe(
						Effect.catch((error) =>
							session.Close.pipe(Effect.ignore, Effect.andThen(Effect.fail(error))),
						),
					);
				const persisted_after_create = yield* repository.ReadDetail(reviewed_detail.id);
				if (persisted_after_create.lifecycle !== "connected")
					yield* repository.Transition({
						capability_id: reviewed_detail.id,
						health: { status: "healthy" },
						lifecycle: "connected",
						operation: "reconnected",
						operation_id: `${operation_id}:reconnected`,
						status: reviewed_detail.enabled ? "enabled" : "disabled",
					});
				sessions.set(reviewed_detail.id, session);
				yield* MonitorSession(reviewed_detail.id, operation_id, session);
				return yield* repository.ReadDetail(reviewed_detail.id);
			}).pipe(
				Effect.catch((error) =>
					Effect.gen(function* () {
						const active = sessions.get(reviewed_detail.id);
						if (active) sessions.delete(reviewed_detail.id);
						if (active) yield* active.Close.pipe(Effect.ignore);
						if (active !== undefined)
							yield* repository.ReadDetail(reviewed_detail.id).pipe(
								Effect.flatMap((persisted) =>
									repository.Transition({
										capability_id: reviewed_detail.id,
										health: { status: "crashed" },
										lifecycle: "crashed",
										operation: "health_checked",
										operation_id: `${operation_id}:failed`,
										status: persisted.status,
									}),
								),
								Effect.ignore,
							);
						return yield* Effect.fail(error);
					}),
				),
			);
		const DecideConnect = (input: {
			readonly approval_id: string;
			readonly approved: boolean;
			readonly approval_fingerprint: string;
		}) =>
			Effect.gen(function* () {
				const decision = yield* repository.DecideConnect(input);
				if (decision === "denied" || !input.approved)
					return yield* new CapabilityServiceError({
						code: "approval_required",
						message: "Capability connection was denied before any transport action",
					});
				const reviewed = yield* repository.ReadConnectApproval(input.approval_id);
				if (decision === "connected")
					return yield* repository.ReadDetail(reviewed.detail.id);
				return yield* ExecuteApprovedConnect(reviewed.operation_id, reviewed.detail);
			});
		const SessionAction = (input: {
			readonly action: "start" | "reconnect" | "restart";
			readonly capability_id: string;
			readonly operation_id: string;
		}) =>
			Effect.gen(function* () {
				const detail = yield* RequireMutable(input.capability_id);
				const approved = yield* repository.ReadApprovedConnect(input.capability_id);
				const authority_fingerprint = Fingerprint({
					auth: detail.auth,
					scope: detail.scope,
					source: detail.source,
					transport: detail.transport,
				});
				if (
					authority_fingerprint !==
					Fingerprint({
						auth: approved.auth,
						scope: approved.scope,
						source: approved.source,
						transport: approved.transport,
					})
				)
					return yield* new CapabilityServiceError({
						code: "approval_required",
						message: "Canonical connection settings changed and require new approval",
					});
				yield* repository.RecordSessionAction({
					action: input.action,
					capability_id: input.capability_id,
					operation_id: input.operation_id,
					request_fingerprint: authority_fingerprint,
				});
				const claim = yield* repository.ClaimSessionAction(input.operation_id);
				if (claim === "completed") return yield* repository.ReadDetail(input.capability_id);
				if (claim === "executing")
					return yield* new CapabilityServiceError({
						code: "connection_in_progress",
						message: "Session action has an ambiguous in-progress transport outcome",
					});
				const active = sessions.get(input.capability_id);
				if (input.action === "restart" && active) {
					yield* active.Close;
					sessions.delete(input.capability_id);
				} else if (active) {
					yield* repository.CompleteSessionAction({
						action: input.action,
						detail,
						operation_id: input.operation_id,
					});
					return detail;
				}
				const session = yield* transports
					.Connect(detail)
					.pipe(Effect.provideService(Scope.Scope, scope));
				sessions.set(input.capability_id, session);
				yield* MonitorSession(input.capability_id, input.operation_id, session);
				const initialized = yield* session.Initialize;
				const [tools, resources] = yield* Effect.all([
					session.ListTools,
					session.ListResources,
				]);
				const refreshed = yield* Schema.decodeUnknownEffect(CapabilityDetail)({
					...detail,
					health: { status: "healthy" },
					lifecycle: "connected",
					resources: resources.map((resource) => ({
						...(resource.description === undefined
							? {}
							: { description: resource.description }),
						uri: resource.uri,
					})),
					...(initialized.instructions === undefined
						? {}
						: { server_instructions: initialized.instructions }),
					tools: tools.map((tool) => ({
						...(tool.description === undefined
							? {}
							: { description: tool.description }),
						input_schema: tool.input_schema,
						name: tool.name,
					})),
				});
				yield* repository.CompleteSessionAction({
					action: input.action,
					detail: refreshed,
					operation_id: input.operation_id,
					server_metadata: {
						protocol_version: initialized.protocol_version,
						server_name: initialized.server_name,
						...(initialized.server_version === undefined
							? {}
							: { server_version: initialized.server_version }),
					},
				});
				return refreshed;
			}).pipe(
				Effect.catch((error) =>
					Effect.gen(function* () {
						const active = sessions.get(input.capability_id);
						if (active) {
							sessions.delete(input.capability_id);
							yield* active.Close.pipe(Effect.ignore);
						}
						return yield* Effect.fail(error);
					}),
				),
			);
		const InvocationPolicy = (input: CapabilityInvocationRequest) =>
			Effect.gen(function* () {
				const detail = yield* repository.ReadDetail(input.capability_id);
				if (!ScopeMatches(detail.scope, input.scope))
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message: "Capability invocation scope does not match the registry record",
					});
				const policy = detail.policy.find((entry) => entry.name === input.tool_name);
				const declared_tool = detail.tools.some((tool) => tool.name === input.tool_name);
				if (!detail.enabled || !declared_tool || policy?.enabled === false)
					return yield* new CapabilityServiceError({
						code: "disabled",
						message: "Capability tool is disabled",
					});
				/** Unconfigured tools default to approval, never to silent execution. */
				const required =
					policy === undefined ||
					policy.approval === "always" ||
					(policy.approval === "sensitive_only" && policy.sensitive_label !== undefined);
				return { detail, required } as const;
			});
		const ExecuteInvocation = (
			input: CapabilityInvocationRequest & {
				readonly approval_required: boolean;
				readonly operation_id: string;
			},
		) =>
			Effect.gen(function* () {
				const { detail } = yield* InvocationPolicy(input);
				const result_artifact_id = `artifact_${Fingerprint({
					capability_id: input.capability_id,
					invocation_id: input.operation_id,
					tool_name: input.tool_name,
				}).slice(0, 24)}`;
				const claim = yield* repository.ClaimInvocation(input.operation_id);
				if (claim === "completed")
					return yield* repository.CompleteInvocation({
						approval_required: input.approval_required,
						artifact_id: result_artifact_id,
						capability_id: input.capability_id,
						operation_id: input.operation_id,
						result_json: "",
						status: detail.status,
						tool_name: input.tool_name,
					});
				if (claim === "executing")
					return yield* new CapabilityServiceError({
						code: "invocation_in_progress",
						message: "Invocation recovery will not repeat an ambiguous tool call",
					});
				if (claim === "denied")
					return yield* new CapabilityServiceError({
						code: "approval_required",
						message: "Tool invocation was denied before any transport action",
					});
				const session = sessions.get(input.capability_id);
				if (!session)
					return yield* new CapabilityServiceError({
						code: "not_connected",
						message: "Capability is not connected",
					});
				const arguments_value = yield* Schema.decodeUnknownEffect(
					Schema.UnknownFromJsonString,
				)(input.arguments_json).pipe(
					Effect.flatMap(
						Schema.decodeUnknownEffect(Schema.Record(Schema.String, Schema.Unknown)),
					),
					Effect.mapError(
						() =>
							new CapabilityServiceError({
								code: "policy_denied",
								message: "Tool arguments must be a JSON object",
							}),
					),
				);
				const outcome = yield* session
					.CallTool({ name: input.tool_name, arguments: arguments_value })
					.pipe(
						Effect.flatMap((result) =>
							Effect.try({
								try: () => {
									const result_json = JSON.stringify(result);
									if (
										result_json === undefined ||
										Buffer.byteLength(result_json, "utf8") > 1_048_576
									)
										throw new Error("MCP result is not bounded JSON");
									return { _tag: "completed" as const, result_json };
								},
								catch: () =>
									new CapabilityServiceError({
										code: "policy_denied",
										message:
											"MCP result could not be stored as a bounded artifact",
									}),
							}),
						),
						Effect.catch(() => Effect.succeed({ _tag: "failed" as const })),
					);
				if (outcome._tag === "failed")
					return yield* repository.FailInvocation({
						approval_required: input.approval_required,
						capability_id: input.capability_id,
						operation_id: input.operation_id,
						status: detail.status,
						tool_name: input.tool_name,
					});
				return yield* repository.CompleteInvocation({
					approval_required: input.approval_required,
					artifact_id: result_artifact_id,
					capability_id: input.capability_id,
					operation_id: input.operation_id,
					result_json: outcome.result_json,
					status: detail.status,
					tool_name: input.tool_name,
				});
			});
		const Invoke = (input: CapabilityInvocationRequest & { readonly operation_id: string }) =>
			Effect.gen(function* () {
				const { required } = yield* InvocationPolicy(input);
				if (required)
					return {
						approval_required: true,
						capability_id: input.capability_id,
						invocation_id: input.operation_id,
						status: "requested",
						tool_name: input.tool_name,
					} satisfies CapabilityInvocationMetadata;
				const request_fingerprint = Fingerprint({
					arguments_json: input.arguments_json,
					capability_id: input.capability_id,
					scope: input.scope,
					tool_name: input.tool_name,
				});
				yield* repository.RecordInvocation({
					capability_id: input.capability_id,
					operation_id: input.operation_id,
					request_fingerprint,
					status: (yield* repository.ReadDetail(input.capability_id)).status,
					tool_name: input.tool_name,
				});
				yield* repository.DecideInvocation({
					approved: true,
					operation_id: input.operation_id,
					status: (yield* repository.ReadDetail(input.capability_id)).status,
				});
				return yield* ExecuteInvocation({ ...input, approval_required: false });
			});
		const RequestInvocation = (
			input: CapabilityInvocationApprovalRequest & { readonly operation_id: string },
		) =>
			Effect.gen(function* () {
				const { required } = yield* InvocationPolicy(input);
				if (!required)
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message: "This tool does not require an approval request",
					});
				const request_fingerprint = Fingerprint({
					arguments_json: input.arguments_json,
					capability_id: input.capability_id,
					scope: input.scope,
					tool_name: input.tool_name,
				});
				if (request_fingerprint !== input.intent_fingerprint)
					return yield* new CapabilityServiceError({
						code: "preview_changed",
						message:
							"Invocation intent fingerprint does not match the reviewed request",
					});
				yield* repository.RecordInvocation({
					approval_fingerprint: input.intent_fingerprint,
					approval_id: input.approval_id,
					capability_id: input.capability_id,
					operation_id: input.operation_id,
					request_fingerprint,
					status: (yield* repository.ReadDetail(input.capability_id)).status,
					tool_name: input.tool_name,
				});
				return {
					approval_required: true,
					capability_id: input.capability_id,
					invocation_id: input.operation_id,
					status: "requested",
					tool_name: input.tool_name,
				} satisfies CapabilityInvocationMetadata;
			});
		const DecideInvocation = (input: CapabilityInvocationApprovalDecision) =>
			Effect.gen(function* () {
				const reviewed = yield* repository.ReadInvocationApproval(input.approval_id);
				const request_fingerprint = Fingerprint({
					arguments_json: input.arguments_json,
					capability_id: input.capability_id,
					scope: input.scope,
					tool_name: input.tool_name,
				});
				if (
					input.intent_fingerprint !== reviewed.approval_fingerprint ||
					request_fingerprint !== reviewed.request_fingerprint ||
					reviewed.capability_id !== input.capability_id ||
					reviewed.tool_name !== input.tool_name
				)
					return yield* new CapabilityServiceError({
						code: "preview_changed",
						message: "Invocation decision does not match the durable reviewed intent",
					});
				const { required } = yield* InvocationPolicy(input);
				if (!required)
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message: "Invocation policy changed; approval must not execute",
					});
				const decision = yield* repository.DecideInvocation({
					approval_fingerprint: input.intent_fingerprint,
					approval_id: input.approval_id,
					approved: input.approved,
					operation_id: reviewed.operation_id,
					status: (yield* repository.ReadDetail(input.capability_id)).status,
				});
				if (!input.approved || decision === "denied")
					return {
						approval_required: true,
						capability_id: input.capability_id,
						invocation_id: reviewed.operation_id,
						status: "denied",
						tool_name: input.tool_name,
					} satisfies CapabilityInvocationMetadata;
				return yield* ExecuteInvocation({
					...input,
					approval_required: true,
					operation_id: reviewed.operation_id,
				});
			});
		const Health = (input: { readonly capability_id: string; readonly operation_id: string }) =>
			Effect.gen(function* () {
				const detail = yield* RequireMutable(input.capability_id);
				const session = sessions.get(input.capability_id);
				const health = session ? yield* session.Health : "closed";
				const next =
					health === "connected"
						? "healthy"
						: health === "crashed"
							? "crashed"
							: "offline";
				if (next === "crashed") sessions.delete(input.capability_id);
				yield* repository.Transition({
					capability_id: input.capability_id,
					health: { status: next },
					/** A fresh runtime never claims a persisted live session; recovery is inert. */
					lifecycle:
						next === "crashed"
							? "crashed"
							: session === undefined
								? "stopped"
								: detail.lifecycle,
					operation: "health_checked",
					operation_id: input.operation_id,
					status: detail.status,
				});
				return yield* repository.ReadDetail(input.capability_id);
			});
		const Disconnect = (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) =>
			Effect.gen(function* () {
				const detail = yield* RequireMutable(input.capability_id);
				const session = sessions.get(input.capability_id);
				if (session) {
					yield* session.Close.pipe(Effect.ignore);
					sessions.delete(input.capability_id);
				}
				yield* repository.Transition({
					capability_id: input.capability_id,
					health: { status: "offline" },
					lifecycle: "disconnected",
					operation: "disconnected",
					operation_id: input.operation_id,
					status: detail.enabled ? "disconnect_available" : "disabled",
				});
			});
		const Enable = (input: { readonly capability_id: string; readonly operation_id: string }) =>
			RequireMutable(input.capability_id).pipe(
				Effect.andThen(
					repository.Transition({
						capability_id: input.capability_id,
						enabled: true,
						operation: "enabled",
						operation_id: input.operation_id,
						status: "enabled",
					}),
				),
			);
		const Disable = (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) =>
			Effect.gen(function* () {
				yield* RequireMutable(input.capability_id);
				const active = sessions.get(input.capability_id);
				if (active) {
					yield* active.Close;
					sessions.delete(input.capability_id);
				}
				yield* repository.Transition({
					capability_id: input.capability_id,
					enabled: false,
					health: { status: "offline" },
					lifecycle: "stopped",
					operation: "disabled",
					operation_id: input.operation_id,
					status: "disabled",
				});
			});
		const Remove = (input: { readonly capability_id: string; readonly operation_id: string }) =>
			RequireMutable(input.capability_id).pipe(
				Effect.andThen(
					Disconnect({
						capability_id: input.capability_id,
						operation_id: `${input.operation_id}:disconnect`,
					}),
				),
				Effect.andThen(
					repository.Transition({
						capability_id: input.capability_id,
						enabled: false,
						lifecycle: "removed",
						operation: "removed",
						operation_id: input.operation_id,
						status: "removed",
					}),
				),
			);
		const Uninstall = (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) =>
			Effect.gen(function* () {
				yield* repository.RecordUninstall(input);
				const claim = yield* repository.ClaimUninstall(input.operation_id);
				if (claim === "uninstalled") return;
				if (claim === "closing") {
					/** A persisted removal proves the close returned before a crash; only finalization is replayed. */
					const detail = yield* repository.ReadDetail(input.capability_id);
					if (detail.lifecycle === "removed" && detail.status === "removed")
						return yield* repository.CompleteUninstall(input.operation_id);
					return yield* new CapabilityServiceError({
						code: "connection_in_progress",
						message:
							"Capability uninstall is recovering an ambiguous close and will not close twice",
					});
				}
				const session = sessions.get(input.capability_id);
				if (session) {
					yield* session.Close;
					sessions.delete(input.capability_id);
				}
				yield* repository.CompleteUninstall(input.operation_id);
			});
		/**
		 * A new backend process owns no live MCP sessions. Reconcile persisted live or
		 * ambiguous states without invoking a connector; reconnect remains explicit.
		 */
		const persisted = yield* repository.ReadSummaries;
		yield* Effect.forEach(
			persisted.filter(
				(summary) =>
					summary.lifecycle === "connecting" || summary.lifecycle === "connected",
			),
			(summary) =>
				repository.Transition({
					capability_id: summary.id,
					health: { status: summary.lifecycle === "connecting" ? "crashed" : "offline" },
					lifecycle: summary.lifecycle === "connecting" ? "crashed" : "stopped",
					operation: "health_checked",
					operation_id: `startup_recovery_${Fingerprint({ id: summary.id, lifecycle: summary.lifecycle }).slice(0, 24)}`,
					status: summary.status,
				}),
			{ discard: true },
		);
		return {
			DecideConnect,
			DecideInvocation,
			Disable,
			Disconnect,
			Enable,
			Health,
			Invoke,
			Preview,
			RequestConnect,
			RequestInvocation,
			Remove,
			SessionAction,
			Uninstall,
		};
	}),
);

/** OAuth lifecycle ownership keeps provider credentials at the injected vault boundary. */
export class CapabilityOAuthLifecycle extends Context.Service<
	CapabilityOAuthLifecycle,
	{
		readonly Begin: (
			input: OAuthBeginInput & { readonly operation_id: string },
		) => Effect.Effect<
			{
				readonly _tag: "started";
				readonly authorization_url: string;
				readonly state: string;
			},
			unknown
		>;
		readonly Complete: (
			input: OAuthCompletionInput & { readonly operation_id: string },
		) => Effect.Effect<OAuthTokenStatus, unknown>;
		readonly Refresh: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<OAuthTokenStatus, unknown>;
		readonly Revoke: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<void, unknown>;
		readonly Status: (capability_id: string) => Effect.Effect<OAuthTokenStatus, unknown>;
	}
>()("Artisan/Marketplace/CapabilityOAuthLifecycle") {}

export const CapabilityOAuthLifecycleLive = Layer.effect(
	CapabilityOAuthLifecycle,
	Effect.gen(function* () {
		const repository = yield* CapabilityRepository;
		const oauth = yield* OAuth;
		const OAuthDetail = (capability_id: string) =>
			Effect.gen(function* () {
				const detail = yield* repository.ReadDetail(capability_id);
				if (detail.auth.kind !== "oauth")
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message: "Capability does not use OAuth",
					});
				return detail;
			});
		/** The canonical record, not a provider probe, makes completed retries deterministic. */
		const TokenStatusFromDetail = (detail: CapabilityDetail): OAuthTokenStatus => {
			if (detail.auth.kind !== "oauth") return { capability_id: detail.id, state: "absent" };
			return {
				capability_id: detail.id,
				...(detail.auth.token_ref === undefined
					? {}
					: { secret_reference: detail.auth.token_ref }),
				state:
					detail.auth.token_status === "authorized"
						? "active"
						: detail.auth.token_status === "not_started"
							? "absent"
							: "expired",
			};
		};
		const Begin = (input: OAuthBeginInput & { readonly operation_id: string }) =>
			Effect.gen(function* () {
				const detail = yield* OAuthDetail(input.capability_id);
				yield* repository.RecordOAuthOperation({
					capability_id: input.capability_id,
					kind: "oauth_begin",
					operation_id: input.operation_id,
					request_fingerprint: Fingerprint({
						authorization_url: input.authorization_url,
						scopes: input.scopes,
					}),
				});
				const claim = yield* repository.ClaimOAuthOperation(input.operation_id);
				if (claim === "completed")
					return {
						_tag: "started" as const,
						...(yield* repository.ReadOAuthBeginResult(input.operation_id)),
					};
				if (claim === "executing")
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message:
							"OAuth begin outcome is ambiguous and requires explicit reconciliation",
					});
				const result = yield* oauth.Begin({
					authorization_url: input.authorization_url,
					capability_id: input.capability_id,
					scopes: input.scopes,
				});
				yield* repository.CompleteOAuthOperation({
					begin_result: result,
					operation: "oauth_started",
					operation_id: input.operation_id,
					status: detail.status,
				});
				return { _tag: "started" as const, ...result };
			});
		const Complete = (input: OAuthCompletionInput & { readonly operation_id: string }) =>
			Effect.gen(function* () {
				const detail = yield* OAuthDetail(input.capability_id);
				yield* repository.RecordOAuthOperation({
					capability_id: input.capability_id,
					kind: "oauth_complete",
					operation_id: input.operation_id,
					request_fingerprint: Fingerprint(input.callback_reference),
				});
				const claim = yield* repository.ClaimOAuthOperation(input.operation_id);
				if (claim === "completed")
					return TokenStatusFromDetail(yield* OAuthDetail(input.capability_id));
				if (claim === "executing")
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message:
							"OAuth completion outcome is ambiguous and requires explicit reconciliation",
					});
				const result = yield* oauth.Complete({
					capability_id: input.capability_id,
					callback_reference: input.callback_reference,
				});
				yield* repository.CompleteOAuthOperation({
					operation: "oauth_completed",
					operation_id: input.operation_id,
					status: detail.status,
					token_status: result,
				});
				return result;
			});
		const Refresh = (input: {
			readonly capability_id: string;
			readonly operation_id: string;
		}) =>
			Effect.gen(function* () {
				const detail = yield* OAuthDetail(input.capability_id);
				yield* repository.RecordOAuthOperation({
					capability_id: input.capability_id,
					kind: "oauth_refresh",
					operation_id: input.operation_id,
					request_fingerprint: Fingerprint({ capability_id: input.capability_id }),
				});
				const claim = yield* repository.ClaimOAuthOperation(input.operation_id);
				if (claim === "completed")
					return TokenStatusFromDetail(yield* OAuthDetail(input.capability_id));
				if (claim === "executing")
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message:
							"OAuth refresh outcome is ambiguous and requires explicit reconciliation",
					});
				const result = yield* oauth.Refresh(input.capability_id);
				yield* repository.CompleteOAuthOperation({
					operation: "oauth_refreshed",
					operation_id: input.operation_id,
					status: detail.status,
					token_status: result,
				});
				return result;
			});
		const Revoke = (input: { readonly capability_id: string; readonly operation_id: string }) =>
			Effect.gen(function* () {
				const detail = yield* OAuthDetail(input.capability_id);
				yield* repository.RecordOAuthOperation({
					capability_id: input.capability_id,
					kind: "oauth_revoke",
					operation_id: input.operation_id,
					request_fingerprint: Fingerprint({ capability_id: input.capability_id }),
				});
				const claim = yield* repository.ClaimOAuthOperation(input.operation_id);
				if (claim === "completed") return;
				if (claim === "executing")
					return yield* new CapabilityServiceError({
						code: "policy_denied",
						message:
							"OAuth revoke outcome is ambiguous and requires explicit reconciliation",
					});
				yield* oauth.Revoke(input.capability_id);
				yield* repository.CompleteOAuthOperation({
					operation: "oauth_revoked",
					operation_id: input.operation_id,
					status: detail.status,
					token_status: { capability_id: input.capability_id, state: "revoked" },
				});
			});
		const Status = (capability_id: string) =>
			OAuthDetail(capability_id).pipe(Effect.map(TokenStatusFromDetail));
		return { Begin, Complete, Refresh, Revoke, Status };
	}),
);
