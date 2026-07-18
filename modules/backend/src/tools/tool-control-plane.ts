import { Context, Crypto, Data, Effect, Encoding, Layer } from "effect";

import type {
	ArtisanApprovalListQuery,
	ArtisanApprovalListQueryResult,
	ArtisanApprovalResolveRequest,
	ArtisanToolExecutionRequest,
	ArtisanToolInvocationListQuery,
	ArtisanToolInvocationListQueryResult,
	ArtisanToolPermissionPolicy,
	ArtisanToolRegistryListQueryResult,
} from "@artisan/protocol";

import { JournalStore } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { WorkspaceFileDiscovery } from "../workspace/workspace-file-discovery";
import { ArtisanToolApprovalPolicy } from "./approval-policy";
import { ArtisanToolRegistry } from "./artisan-tool-registry";
import { ToolInvocationRepository } from "./tool-invocation-repository";
import { ExecuteTool } from "./tool-handlers";

/** Reports a sanitized executor error without exposing host implementation details. */
export class ToolControlPlaneError extends Data.TaggedError("ToolControlPlaneError")<{
	readonly reason: "conflict" | "failed" | "invalid" | "unavailable";
}> {}

/** Coordinates decode, policy, durable lifecycle, approval routing, and concrete built-in execution. */
export class ToolControlPlane extends Context.Service<
	ToolControlPlane,
	{
		readonly Execute: (input: {
			readonly agent_id?: string | undefined;
			readonly request: typeof ArtisanToolExecutionRequest.Type;
			readonly run_id?: string | undefined;
			readonly thread_id: string;
		}) => Effect.Effect<unknown, ToolControlPlaneError>;
		readonly ResolveApproval: (input: {
			readonly request: typeof ArtisanApprovalResolveRequest.Type;
			readonly thread_id: string;
		}) => Effect.Effect<unknown, ToolControlPlaneError>;
		readonly Registry: (input: {
			readonly policy: typeof ArtisanToolPermissionPolicy.Type;
			readonly thread_id?: string | undefined;
			readonly workspace_id?: string | undefined;
		}) => Effect.Effect<ArtisanToolRegistryListQueryResult, ToolControlPlaneError>;
		readonly Invocations: (
			query: typeof ArtisanToolInvocationListQuery.Type,
		) => Effect.Effect<ArtisanToolInvocationListQueryResult, ToolControlPlaneError>;
		readonly Approvals: (
			query: typeof ArtisanApprovalListQuery.Type,
		) => Effect.Effect<ArtisanApprovalListQueryResult, ToolControlPlaneError>;
		readonly Discover: (
			query: Parameters<(typeof WorkspaceFileDiscovery.Service)["Discover"]>[0],
		) => ReturnType<(typeof WorkspaceFileDiscovery.Service)["Discover"]>;
		readonly LanguageCapabilities: (
			query: Parameters<(typeof WorkspaceFileDiscovery.Service)["LanguageCapabilities"]>[0],
		) => ReturnType<(typeof WorkspaceFileDiscovery.Service)["LanguageCapabilities"]>;
	}
>()("Artisan/ToolControlPlane") {}

/** Composes the policy-aware durable router; specialized handlers remain behind this single boundary. */
export const ToolControlPlaneLive = Layer.effect(
	ToolControlPlane,
	Effect.gen(function* () {
		const approval_policy = yield* ArtisanToolApprovalPolicy;
		const crypto = yield* Crypto.Crypto;
		const discovery = yield* WorkspaceFileDiscovery;
		const executor = yield* ExecuteTool;
		const journal = yield* JournalStore;
		const metadata = yield* RuntimeMetadata;
		const registry = yield* ArtisanToolRegistry;
		const repository = yield* ToolInvocationRepository;
		const fingerprint = (value: unknown) =>
			crypto.digest("SHA-256", new TextEncoder().encode(JSON.stringify(value))).pipe(
				Effect.map(Encoding.encodeHex),
				Effect.mapError(() => new ToolControlPlaneError({ reason: "failed" })),
			);
		const Execute = (input: {
			readonly agent_id?: string | undefined;
			readonly request: typeof ArtisanToolExecutionRequest.Type;
			readonly run_id?: string | undefined;
			readonly thread_id: string;
		}) =>
			Effect.gen(function* () {
				const declaration = yield* registry
					.Find(input.request.input.tool_id)
					.pipe(
						Effect.mapError(() => new ToolControlPlaneError({ reason: "unavailable" })),
					);
				const decision = yield* approval_policy
					.Decide(declaration.descriptor, input.request.policy)
					.pipe(Effect.mapError(() => new ToolControlPlaneError({ reason: "invalid" })));
				const workspace_id =
					"workspace_id" in input.request.input
						? input.request.input.workspace_id
						: undefined;
				const availability = yield* registry
					.Availability({
						policy: input.request.policy,
						...(workspace_id === undefined ? {} : { workspace_id }),
					})
					.pipe(
						Effect.mapError(() => new ToolControlPlaneError({ reason: "unavailable" })),
					);
				const capability = availability.find(
					(entry) => entry.tool_id === input.request.input.tool_id,
				)!;
				const now = yield* metadata.Now;
				const explicit_approval =
					input.request.input.tool_id === "approval.request"
						? input.request.input
						: undefined;
				const approval_id =
					explicit_approval?.approval_id ??
					(decision.decision === "approval_required"
						? `${input.request.invocation_id}_approval`
						: undefined);
				const denied = decision.decision === "denied";
				const unsupported = !denied && capability.state === "unavailable";
				const invocation = {
					...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
					...(approval_id ? { approval_id } : {}),
					input_summary: input.request.input.tool_id,
					invocation_id: input.request.invocation_id,
					lifecycle: unsupported
						? ("unsupported" as const)
						: denied
							? ("denied" as const)
							: approval_id
								? ("awaiting_approval" as const)
								: ("requested" as const),
					...(unsupported
						? {
								completed_at: now,
								outcome: {
									code: "capability_unavailable",
									status: "unsupported" as const,
								},
							}
						: denied
							? {
									completed_at: now,
									outcome: {
										code: "permission_denied",
										status: "denied" as const,
									},
								}
							: {}),
					permission: decision,
					...(input.request.raw_origin === undefined
						? {}
						: { raw_origin: input.request.raw_origin }),
					requested_at: now,
					...(input.run_id === undefined ? {} : { run_id: input.run_id }),
					thread_id: input.thread_id,
					tool_id: input.request.input.tool_id,
					updated_at: now,
				};
				const approval = approval_id
					? {
							approval_id,
							description:
								explicit_approval?.description ??
								`Approve ${declaration.descriptor.title}`,
							invocation_id: invocation.invocation_id,
							permission_requirements:
								explicit_approval?.permission_requirements ?? decision.requirements,
							...(input.request.raw_origin === undefined
								? {}
								: { raw_origin: input.request.raw_origin }),
							requested_at: now,
						}
					: undefined;
				const accepted = yield* repository
					.Begin({
						approval,
						execution_input: input.request.input,
						invocation,
						request_fingerprint: yield* fingerprint({
							...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
							execution_input: input.request.input,
							invocation_id: input.request.invocation_id,
							policy: input.request.policy,
							...(input.request.raw_origin === undefined
								? {}
								: { raw_origin: input.request.raw_origin }),
							...(input.run_id === undefined ? {} : { run_id: input.run_id }),
							thread_id: input.thread_id,
						}),
					})
					.pipe(Effect.mapError(() => new ToolControlPlaneError({ reason: "conflict" })));
				if (accepted.lifecycle !== "requested") return accepted;
				const claim = yield* repository
					.Claim(accepted.invocation_id)
					.pipe(Effect.mapError(() => new ToolControlPlaneError({ reason: "conflict" })));
				if (claim.status !== "claimed") return claim.invocation;
				const outcome = yield* executor
					.Execute({
						...(input.agent_id === undefined ? {} : { agent_id: input.agent_id }),
						input: input.request.input,
						invocation_id: accepted.invocation_id,
						...(input.request.raw_origin === undefined
							? {}
							: { raw_origin: input.request.raw_origin }),
						...(input.run_id === undefined ? {} : { run_id: input.run_id }),
						thread_id: input.thread_id,
					})
					.pipe(
						Effect.catch(() =>
							Effect.succeed({ code: "tool_failed", status: "failed" as const }),
						),
					);
				return yield* repository
					.Finalize(accepted.invocation_id, outcome)
					.pipe(Effect.mapError(() => new ToolControlPlaneError({ reason: "failed" })));
			});
		const ResolveApproval = (input: {
			readonly request: typeof ArtisanApprovalResolveRequest.Type;
			readonly thread_id: string;
		}) =>
			Effect.gen(function* () {
				const existing = yield* repository
					.ReadInvocation(input.request.invocation_id)
					.pipe(Effect.mapError(() => new ToolControlPlaneError({ reason: "conflict" })));
				if (existing.thread_id !== input.thread_id)
					return yield* Effect.fail(new ToolControlPlaneError({ reason: "conflict" }));
				const resolved_at = yield* metadata.Now;
				yield* repository
					.ResolveApproval({ ...input.request, resolved_at })
					.pipe(Effect.mapError(() => new ToolControlPlaneError({ reason: "conflict" })));
				if (!input.request.approved) return undefined;
				const claim = yield* repository
					.Claim(input.request.invocation_id)
					.pipe(Effect.mapError(() => new ToolControlPlaneError({ reason: "conflict" })));
				if (claim.status !== "claimed") return claim.invocation;
				const execution_input = yield* repository
					.ReadExecutionInput(input.request.invocation_id)
					.pipe(Effect.mapError(() => new ToolControlPlaneError({ reason: "failed" })));
				const outcome = yield* executor
					.Execute({
						...(claim.invocation.agent_id === undefined
							? {}
							: { agent_id: claim.invocation.agent_id }),
						input: execution_input,
						invocation_id: claim.invocation.invocation_id,
						...(claim.invocation.raw_origin === undefined
							? {}
							: { raw_origin: claim.invocation.raw_origin }),
						...(claim.invocation.run_id === undefined
							? {}
							: { run_id: claim.invocation.run_id }),
						thread_id: claim.invocation.thread_id,
					})
					.pipe(
						Effect.catch(() =>
							Effect.succeed({ code: "tool_failed", status: "failed" as const }),
						),
					);
				return yield* repository
					.Finalize(input.request.invocation_id, outcome)
					.pipe(Effect.mapError(() => new ToolControlPlaneError({ reason: "failed" })));
			});
		const Registry = (input: {
			readonly policy: typeof ArtisanToolPermissionPolicy.Type;
			readonly thread_id?: string | undefined;
			readonly workspace_id?: string | undefined;
		}) =>
			Effect.all([
				registry.Availability({
					policy: input.policy,
					...(input.workspace_id === undefined
						? {}
						: { workspace_id: input.workspace_id }),
				}),
				journal.ReadWatermark(),
				input.thread_id === undefined
					? Effect.succeed([])
					: repository.Usage(input.thread_id),
			]).pipe(
				Effect.map(([availability, journal_sequence, usage]) => ({
					availability,
					declarations: registry.Declarations,
					journal_sequence,
					usage,
				})),
				Effect.mapError(() => new ToolControlPlaneError({ reason: "failed" })),
			);
		const Invocations = (query: typeof ArtisanToolInvocationListQuery.Type) =>
			Effect.all([
				repository.ListInvocations({
					thread_id: query.thread_id,
					...(query.after_journal_sequence === undefined
						? {}
						: { after_journal_sequence: query.after_journal_sequence }),
					...(query.lifecycle === undefined ? {} : { lifecycle: query.lifecycle }),
					...(query.limit === undefined ? {} : { limit: query.limit }),
					...(query.run_id === undefined ? {} : { run_id: query.run_id }),
					...(query.tool_id === undefined ? {} : { tool_id: query.tool_id }),
				}),
				journal.ReadWatermark(),
			]).pipe(
				Effect.map(([invocations, journal_sequence]) => ({
					invocations,
					journal_sequence,
				})),
				Effect.mapError(() => new ToolControlPlaneError({ reason: "failed" })),
			);
		const Approvals = (query: typeof ArtisanApprovalListQuery.Type) =>
			Effect.all([
				repository.ListApprovals(query.thread_id, query.state),
				journal.ReadWatermark(),
			]).pipe(
				Effect.map(([approvals, journal_sequence]) => ({ approvals, journal_sequence })),
				Effect.mapError(() => new ToolControlPlaneError({ reason: "failed" })),
			);
		return {
			Approvals,
			Discover: discovery.Discover,
			Execute,
			Invocations,
			LanguageCapabilities: discovery.LanguageCapabilities,
			Registry,
			ResolveApproval,
		};
	}),
);
