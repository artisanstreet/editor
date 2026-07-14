import { Effect } from "effect";

import type {
	InboundControlEnvelope,
	OutboundControlEnvelope,
	ProtocolErrorDetail,
} from "@artisan/protocol";

import {
	ArtisanClientError,
	type ArtisanClientErrorCode,
	type ArtisanClientOptions,
} from "../client-contract";
import type { MessagePortConnection } from "../connector";
import type { MessagePortError } from "../message-port";

/** Carries the currently negotiated ids and isolated port pair. */
export interface ActiveClientSession {
	readonly connection_id: string;
	readonly ports: MessagePortConnection;
	readonly protocol_connection_id: string;
	readonly stream_ticket: string;
}

/** Sends one already-built envelope if a connection is currently ready. */
export type SendCurrent = (envelope: InboundControlEnvelope) => Effect.Effect<void>;

/** Waits for the current negotiated session without exposing connection state. */
export type AwaitActive = Effect.Effect<ActiveClientSession, ArtisanClientError>;

/** Supplies one frontend trace reused when building a single envelope. */
export interface FrontendTrace {
	readonly message_id: string;
	readonly origin: "frontend";
	readonly protocol_version: 1;
	readonly schema_version: 1;
	readonly sent_at: string;
}

/** Creates a fresh trace once; reconnect retries retain the completed envelope. */
export type MakeTrace = Effect.Effect<FrontendTrace>;

/** Narrows client requests that expect exactly one correlated result. */
export type PendingRequestEnvelope = Extract<
	InboundControlEnvelope,
	{
		readonly kind:
			| "command"
			| "external_wait.request"
			| "external_wait.cancel"
			| "external_wait.manual_resume"
			| "external_wait.query"
			| "hosted.project.clone.request"
			| "hosted.project.clone.approval.query"
			| "hosted.project.clone.approval.respond"
			| "guidance.drift.resolve"
			| "guidance.query"
			| "guidance.selection"
			| "guidance.sync.retry"
			| "guidance.update"
			| "model_behaviour.drift.resolve"
			| "model_behaviour.query"
			| "model_behaviour.sync.retry"
			| "model_behaviour.update"
			| "orchestration.graph.query"
			| "terminal.list.query"
			| "thread.list.query"
			| "thread.retention.query"
			| "thread.retention.update"
			| "thread.work.query"
			| "workspace.file.read.query"
			| "workspace.file.replace"
			| "workspace.change.list.query"
			| "workspace.change.diff.query"
			| "workspace.replace.approval.query"
			| "workspace.replace.approval.respond"
			| "workspace.change.review"
			| "workspace.change.rollback"
			| "workspace.git.session.query"
			| "workspace.git.session.refresh"
			| "workspace.git.fetch.query"
			| "workspace.git.fetch.policy.update"
			| "workspace.git.fetch.request"
			| "hosted.git.snapshot.query"
			| "hosted.git.check_failure_detail.query"
			| "hosted.git.snapshot.refresh"
			| "workspace.git.checkout.request"
			| "workspace.git.checkout.approval.query"
			| "workspace.git.checkout.approval.respond"
			| "workspace.git.mutation.request"
			| "workspace.git.mutation.approval.query"
			| "workspace.git.mutation.approval.respond";
	}
>;

/** Narrows backend results completed through the request coordinator. */
export type PendingResultEnvelope = Extract<
	OutboundControlEnvelope,
	{
		readonly kind:
			| "command.receipt"
			| "external_wait.query.result"
			| "hosted.project.clone.approval.query.result"
			| "guidance.query.result"
			| "model_behaviour.query.result"
			| "orchestration.graph.query.result"
			| "terminal.list.query.result"
			| "thread.list.query.result"
			| "thread.retention.query.result"
			| "thread.work.query.result"
			| "workspace.file.read.query.result"
			| "workspace.change.list.query.result"
			| "workspace.change.diff.query.result"
			| "workspace.replace.approval.query.result"
			| "workspace.git.session.query.result"
			| "workspace.git.fetch.query.result"
			| "hosted.git.snapshot.query.result"
			| "hosted.git.check_failure_detail.query.result"
			| "workspace.git.checkout.approval.query.result"
			| "workspace.git.mutation.approval.query.result";
	}
>;

/** Identifies the expected result kind for one pending request. */
export type PendingResultKind = PendingResultEnvelope["kind"];

/** Creates one consistently shaped typed client failure. */
export function client_error(
	code: ArtisanClientErrorCode,
	message: string,
	cause: unknown,
	retryable = false,
	protocol_code = "",
) {
	return new ArtisanClientError({ cause, code, message, protocol_code, retryable });
}

/** Converts a provider-neutral protocol error into the client error channel. */
export function protocol_client_error(detail: ProtocolErrorDetail, cause: unknown = detail) {
	return client_error("protocol", detail.message, cause, detail.retryable, detail.code);
}

/** Converts a normalized port failure into a retryable connection error. */
export function map_port_error(cause: MessagePortError) {
	return client_error("connection", "The MessagePort connection closed.", cause, true);
}

/** Produces deterministic stream cursor ordering for hello and ACK envelopes. */
export function record_to_cursors(cursors: Readonly<Record<string, number>>) {
	return Object.entries(cursors)
		.map(([stream_id, sequence]) => ({ sequence, stream_id }))
		.sort((left, right) => left.stream_id.localeCompare(right.stream_id));
}

/** Validates all client limits before any scoped fibers or queues are acquired. */
export function validate_client_options(options: Required<ArtisanClientOptions>) {
	return (
		Number.isSafeInteger(options.error_capacity) &&
		options.error_capacity > 0 &&
		Number.isSafeInteger(options.event_capacity) &&
		options.event_capacity > 0 &&
		Number.isSafeInteger(options.max_pending_requests) &&
		options.max_pending_requests > 0 &&
		Number.isSafeInteger(options.reconnect_delay_ms) &&
		options.reconnect_delay_ms >= 0 &&
		Number.isSafeInteger(options.stream_capacity) &&
		options.stream_capacity > 0 &&
		Number.isSafeInteger(options.subscription_capacity) &&
		options.subscription_capacity > 0
	);
}
