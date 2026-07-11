import { Context, Data, Effect, Option, Scope, Stream } from "effect";

import type {
	CommandPayload,
	EventEnvelope,
	GlobalGuidanceDriftResolutionRequest,
	GlobalGuidanceProvider,
	GlobalGuidanceSelectionRequest,
	GlobalGuidanceSnapshot,
	OrchestrationGraph,
	StreamCursor,
	TerminalSession,
	ThreadListItem,
	ThreadRetentionPolicy,
	ThreadWorkItem,
} from "@artisan/protocol";

/** Identifies a typed frontend client failure. */
export type ArtisanClientErrorCode =
	| "configuration"
	| "connection"
	| "correlation_conflict"
	| "disposed"
	| "event_overflow"
	| "malformed"
	| "protocol"
	| "request_overflow"
	| "stream_closed"
	| "stream_gap"
	| "stream_overflow"
	| "subscription_overflow";

/** Reports a transport, protocol, request, subscription, or stream client failure. */
export class ArtisanClientError extends Data.TaggedError("ArtisanClientError")<{
	readonly cause: unknown;
	readonly code: ArtisanClientErrorCode;
	readonly message: string;
	readonly protocol_code: string;
	readonly retryable: boolean;
}> {}

/** Supplies command intent while the client owns envelope ids and trace metadata. */
export interface ArtisanCommandInput {
	readonly agent_id?: string;
	readonly causation_id?: string;
	readonly command_id?: string;
	readonly payload: CommandPayload;
	readonly run_id?: string;
	readonly thread_id: string;
}

/** Records the durable command outcome returned after acceptance or deduplication. */
export interface ArtisanCommandReceipt {
	readonly command_id: string;
	readonly journal_sequence: number;
	readonly status: "accepted" | "duplicate";
}

/** Supplies the public retention setting and optional durable retry identity. */
export interface ArtisanThreadRetentionUpdateInput {
	readonly command_id?: string;
	readonly enabled: boolean;
	readonly inactivity_days: number;
}

/** Supplies canonical guidance content and an optional durable retry identity. */
export interface ArtisanGlobalGuidanceUpdateInput {
	readonly command_id?: string;
	readonly content: string;
}

/** Selects one first-run provider value while hiding internal settings scope ids. */
export interface ArtisanGlobalGuidanceSelectionInput extends GlobalGuidanceSelectionRequest {
	readonly command_id?: string;
}

/** Resolves one exact drift observation through an explicit recovery action. */
export interface ArtisanGlobalGuidanceDriftInput extends GlobalGuidanceDriftResolutionRequest {
	readonly command_id?: string;
}

/** Retries one provider's fixed sync policy without exposing a provider toggle. */
export interface ArtisanGlobalGuidanceRetryInput {
	readonly command_id?: string;
	readonly provider: GlobalGuidanceProvider;
}

/** Exposes the last event positions applied before an ACK or reconnect hello. */
export interface ArtisanClientCursors {
	readonly event_cursors: ReadonlyArray<StreamCursor>;
	readonly last_journal_sequence: number;
}

/** Delivers a complete graph snapshot or one ordered replacement patch. */
export type OrchestrationGraphUpdate =
	| {
			readonly graph: OrchestrationGraph;
			readonly journal_sequence: number;
			readonly type: "snapshot";
	  }
	| {
			readonly graph: OrchestrationGraph;
			readonly journal_sequence: number;
			readonly type: "patch";
	  };

/** Delivers a race-safe thread-list snapshot, upsert, or removal. */
export type ThreadListUpdate =
	| {
			readonly journal_sequence: number;
			readonly threads: ReadonlyArray<ThreadListItem>;
			readonly type: "snapshot";
	  }
	| {
			readonly journal_sequence: number;
			readonly thread: ThreadListItem;
			readonly type: "upsert";
	  }
	| {
			readonly journal_sequence: number;
			readonly thread_id: string;
			readonly type: "remove";
	  };

/** Configures bounded client queues, reconnect timing, and request concurrency. */
export interface ArtisanClientOptions {
	readonly error_capacity?: number;
	readonly event_capacity?: number;
	readonly max_pending_requests?: number;
	readonly reconnect_delay_ms?: number;
	readonly stream_capacity?: number;
	readonly subscription_capacity?: number;
}

/**
 * Provides typed frontend operations while hiding protocol envelopes and cursors.
 * MessagePorts are reliable while alive: commands retry only after reconnect,
 * using the exact original envelope and id until a durable receipt arrives.
 */
export class ArtisanClient extends Context.Service<
	ArtisanClient,
	{
		readonly Command: (
			input: ArtisanCommandInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly Cursors: Effect.Effect<ArtisanClientCursors>;
		readonly Dispose: Effect.Effect<void>;
		readonly Errors: Stream.Stream<ArtisanClientError>;
		readonly Events: Stream.Stream<EventEnvelope, ArtisanClientError>;
		readonly GetOrchestrationGraph: (
			group_id: string,
		) => Effect.Effect<OrchestrationGraph, ArtisanClientError>;
		readonly GetGlobalGuidance: Effect.Effect<GlobalGuidanceSnapshot, ArtisanClientError>;
		readonly GetThreadRetentionPolicy: Effect.Effect<ThreadRetentionPolicy, ArtisanClientError>;
		readonly GetThreadWork: (
			thread_id: string,
		) => Effect.Effect<Option.Option<ThreadWorkItem>, ArtisanClientError>;
		readonly ListTerminals: (
			thread_id: string,
			workspace_id: string,
		) => Effect.Effect<ReadonlyArray<TerminalSession>, ArtisanClientError>;
		readonly ListThreads: Effect.Effect<ReadonlyArray<ThreadListItem>, ArtisanClientError>;
		readonly OpenAsset: (
			asset_id: string,
		) => Effect.Effect<
			Stream.Stream<Uint8Array, ArtisanClientError>,
			ArtisanClientError,
			Scope.Scope
		>;
		readonly OpenTerminalOutput: (
			terminal_id: string,
		) => Effect.Effect<
			Stream.Stream<Uint8Array, ArtisanClientError>,
			ArtisanClientError,
			Scope.Scope
		>;
		readonly SubscribeThreadList: Effect.Effect<
			Stream.Stream<ThreadListUpdate, ArtisanClientError>,
			ArtisanClientError,
			Scope.Scope
		>;
		readonly SubscribeOrchestrationGraph: (
			group_id: string,
		) => Effect.Effect<
			Stream.Stream<OrchestrationGraphUpdate, ArtisanClientError>,
			ArtisanClientError,
			Scope.Scope
		>;
		readonly ResolveGlobalGuidanceDrift: (
			input: ArtisanGlobalGuidanceDriftInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly RetryGlobalGuidanceSync: (
			input: ArtisanGlobalGuidanceRetryInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly SelectGlobalGuidance: (
			input: ArtisanGlobalGuidanceSelectionInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly UpdateGlobalGuidance: (
			input: ArtisanGlobalGuidanceUpdateInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
		readonly UpdateThreadRetentionPolicy: (
			input: ArtisanThreadRetentionUpdateInput,
		) => Effect.Effect<ArtisanCommandReceipt, ArtisanClientError>;
	}
>()("Artisan/ArtisanClient") {}
