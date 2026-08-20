import { Context, Data, Effect } from "effect";
import type {
	CapabilityConnectPreview,
	CapabilityDetail,
	CapabilityInvocationApprovalDecision,
	CapabilityInvocationApprovalRequest,
	CapabilityInvocationMetadata,
	CapabilityInvocationRequest,
	MarketplaceScope,
} from "@artisan/protocol";

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
			input: CapabilityInvocationRequest & { readonly operation_id: string },
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
			readonly scope?: MarketplaceScope;
		}) => Effect.Effect<CapabilityDetail, unknown>;
		readonly Disconnect: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
			readonly scope?: MarketplaceScope;
		}) => Effect.Effect<void, unknown>;
		readonly Enable: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
			readonly scope?: MarketplaceScope;
		}) => Effect.Effect<void, unknown>;
		readonly Disable: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
			readonly scope?: MarketplaceScope;
		}) => Effect.Effect<void, unknown>;
		readonly Remove: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
			readonly scope?: MarketplaceScope;
		}) => Effect.Effect<void, unknown>;
		readonly Uninstall: (input: {
			readonly capability_id: string;
			readonly operation_id: string;
			readonly scope?: MarketplaceScope;
		}) => Effect.Effect<void, unknown>;
		readonly SessionAction: (input: {
			readonly action: "start" | "reconnect" | "restart";
			readonly capability_id: string;
			readonly operation_id: string;
			readonly scope?: MarketplaceScope;
		}) => Effect.Effect<CapabilityDetail, unknown>;
	}
>()("Artisan/Marketplace/CapabilityService") {}
