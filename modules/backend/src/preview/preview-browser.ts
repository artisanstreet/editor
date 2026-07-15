import { Context, Data, Effect, Layer, Scope, Stream } from "effect";

import type {
	CommandEnvelope,
	EventEnvelope,
	PreviewBrowserInitiator,
	PreviewBrowserLaunchRecord,
	PreviewBrowserLifecycleQueryResult,
	PreviewInspectionSessionRecord,
	PreviewTargetRecord,
} from "@artisan/protocol";

/** Reports a source-safe external URL handoff failure. */
export class ExternalUrlLauncherError extends Data.TaggedError("ExternalUrlLauncherError")<{
	readonly reason: "outcome_unknown" | "rejected" | "unavailable";
}> {}

/** Hands one validated local URL to the desktop shell's configured external browser. */
export class ExternalUrlLauncher extends Context.Service<
	ExternalUrlLauncher,
	{
		readonly Open: (url: string) => Effect.Effect<void, ExternalUrlLauncherError>;
	}
>()("Artisan/ExternalUrlLauncher") {}

/** Supplies an explicit failure until the desktop shell installs its launcher adapter. */
export const UnavailableExternalUrlLauncherLive = Layer.succeed(ExternalUrlLauncher, {
	Open: () => Effect.fail(new ExternalUrlLauncherError({ reason: "unavailable" })),
});

/** Reports a source-safe explicit inspection attachment failure. */
export class BrowserInspectionConnectorError extends Data.TaggedError(
	"BrowserInspectionConnectorError",
)<{
	readonly reason: "rejected" | "unavailable";
}> {}

/** Owns one live, process-local inspection connection without exposing its credentials. */
export interface BrowserInspectionSession {
	/** Requests a graceful connector shutdown; scope release remains the hard authority boundary. */
	readonly Detach: Effect.Effect<void>;
	readonly Disconnected: Effect.Effect<void>;
}

/** Identifies the connector authority that must be fenced before durable terminal state. */
export interface PreviewInspectionRevocation {
	readonly connector_id: string;
	readonly inspection_id: string;
}

/**
 * Attaches through an external-browser connector with two equivalent hard-revocation paths.
 * Scope release must revoke local control, while `Revoke` must idempotently fence the inspection
 * identifier against existing and late attachment authority across backend runtimes.
 */
export class BrowserInspectionConnector extends Context.Service<
	BrowserInspectionConnector,
	{
		readonly Attach: (input: {
			readonly connector_id: string;
			readonly inspection_id: string;
			readonly target: PreviewTargetRecord;
		}) => Effect.Effect<BrowserInspectionSession, BrowserInspectionConnectorError, Scope.Scope>;
		readonly Revoke: (
			input: PreviewInspectionRevocation,
		) => Effect.Effect<void, BrowserInspectionConnectorError>;
	}
>()("Artisan/BrowserInspectionConnector") {}

/** Supplies an explicit failure until a user-configured connector is installed. */
export const UnavailableBrowserInspectionConnectorLive = Layer.succeed(BrowserInspectionConnector, {
	Attach: () => Effect.fail(new BrowserInspectionConnectorError({ reason: "unavailable" })),
	Revoke: () => Effect.fail(new BrowserInspectionConnectorError({ reason: "unavailable" })),
});

/** Returns the canonical lifecycle event and whether this invocation created it. */
export interface PreviewBrowserAcceptance {
	readonly event: EventEnvelope;
	readonly status: "accepted" | "duplicate";
}

/** Identifies one private process-owned browser operation lease. */
export interface PreviewBrowserOperationClaim {
	readonly claim_token: string;
	readonly lease_expires_at_ms: number;
	readonly owner_instance_id: string;
}

/** Identifies the generation a target-removal operation may settle. */
export type PreviewTargetRemovalSubject =
	| { readonly _tag: "Current"; readonly target_generation_id: string }
	| { readonly _tag: "Missing" };

/** Owns one exact target-removal lease and its immutable target subject. */
export interface PreviewTargetRemovalClaim {
	readonly claim_token: string;
	readonly lease_expires_at_ms: number;
	readonly owner_instance_id: string;
	readonly project_id: string;
	readonly subject: PreviewTargetRemovalSubject;
	readonly target_id: string;
	readonly workspace_id: string;
}

/** Identifies one committed removal whose exact-generation inspection fence remains owed. */
export interface PendingPreviewTargetRemovalFence {
	readonly committed_at_ms: number;
	readonly message_id: string;
	readonly project_id: string;
	readonly target_generation_id: string;
	readonly target_id: string;
	readonly thread_id: string;
	readonly workspace_id: string;
}

/** Couples a durable removal-fence obligation to its process-owned claim. */
export interface OwnedPreviewTargetRemovalFence {
	readonly claim: PreviewTargetRemovalClaim;
	readonly fence: PendingPreviewTargetRemovalFence;
}

/** Tells removal coordination whether this invocation committed work or replayed it. */
export interface PreviewTargetRemovalSettlement {
	readonly status: "accepted" | "duplicate";
}

/** Reports a source-safe external-browser lifecycle failure. */
export class PreviewBrowserLifecycleError extends Data.TaggedError("PreviewBrowserLifecycleError")<{
	readonly code: "conflict" | "invalid_request" | "invariant" | "not_found" | "unavailable";
	readonly subject_id: string;
}> {}

/** Carries one prepared browser handoff across the irreversible adapter boundary. */
export interface PreparedPreviewBrowserLaunch {
	readonly claim: PreviewBrowserOperationClaim;
	readonly command: CommandEnvelope;
	readonly launch: PreviewBrowserLaunchRecord;
}

/** Carries one prepared inspection attachment across its live connector boundary. */
export interface PreparedPreviewInspection {
	readonly claim: PreviewBrowserOperationClaim;
	readonly command: CommandEnvelope;
	readonly inspection: PreviewInspectionSessionRecord;
	readonly target: PreviewTargetRecord;
}

/** Attributes one browser lifecycle command from its canonical envelope metadata. */
export function preview_browser_initiator(command: CommandEnvelope): PreviewBrowserInitiator {
	return command.agent_id === undefined
		? { kind: "user" }
		: { agent_id: command.agent_id, kind: "agent" };
}

/** Owns durable browser handoffs and explicit process-local inspection sessions. */
export class PreviewBrowserLifecycle extends Context.Service<
	PreviewBrowserLifecycle,
	{
		readonly SlidingEvents: Stream.Stream<EventEnvelope>;
		readonly Attach: (
			command: CommandEnvelope,
		) => Effect.Effect<PreviewBrowserAcceptance, PreviewBrowserLifecycleError>;
		readonly Detach: (
			command: CommandEnvelope,
		) => Effect.Effect<PreviewBrowserAcceptance, PreviewBrowserLifecycleError>;
		readonly Open: (
			command: CommandEnvelope,
		) => Effect.Effect<PreviewBrowserAcceptance, PreviewBrowserLifecycleError>;
		readonly Query: (input: {
			readonly project_id: string;
			readonly workspace_id: string;
		}) => Effect.Effect<PreviewBrowserLifecycleQueryResult, PreviewBrowserLifecycleError>;
		readonly QuiesceThread: (
			thread_id: string,
		) => Effect.Effect<void, PreviewBrowserLifecycleError>;
		readonly SettleTargetRemovalFence: (
			message_id: string,
		) => Effect.Effect<void, PreviewBrowserLifecycleError>;
		readonly SynchronizeTargetRemoval: <A extends PreviewTargetRemovalSettlement, E, R>(
			input: {
				readonly project_id: string;
				readonly target_id: string;
				readonly workspace_id: string;
			},
			remove: (claim: PreviewTargetRemovalClaim) => Effect.Effect<A, E, R>,
		) => Effect.Effect<A, E | PreviewBrowserLifecycleError, R>;
	}
>()("Artisan/PreviewBrowserLifecycle") {}
