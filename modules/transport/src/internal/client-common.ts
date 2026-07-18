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
			| "artisan.tool.registry.list.query"
			| "artisan.tool.execute"
			| "artisan.approval.resolve"
			| "artisan.tool.invocation.list.query"
			| "artisan.approval.list.query"
			| "git.diff.query"
			| "git.index.stage.request"
			| "git.index.unstage.request"
			| "git.mutation.resolve"
			| "git.workspace.query"
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
			| "orchestration.group.list.query"
			| "preview.asset.metadata.query"
			| "preview.browser.launch"
			| "preview.inspection.close"
			| "preview.inspection.inspect"
			| "preview.inspection.open"
			| "preview.rich_link.resolve.query"
			| "preview.target.get.query"
			| "preview.target.list.query"
			| "preview.target.probe"
			| "preview.target.register"
			| "preview.target.remove"
			| "preview.target.state"
			| "terminal.list.query"
			| "thread.list.query"
			| "thread.transcript.query"
			| "thread.retention.query"
			| "thread.retention.update"
			| "thread.work.query"
			| "workspace.file.read.query"
			| "workspace.file.discovery.query"
			| "workspace.language.capabilities.query"
			| "workspace.file.replace"
			| "workspace.change.list.query"
			| "workspace.change.diff.query"
			| "workspace.change.review"
			| "workspace.change.rollback";
	}
>;

/** Narrows backend results completed through the request coordinator. */
export type PendingResultEnvelope = Extract<
	OutboundControlEnvelope,
	{
		readonly kind:
			| "command.receipt"
			| "artisan.tool.registry.list.query.result"
			| "artisan.tool.invocation.list.query.result"
			| "artisan.approval.list.query.result"
			| "git.diff.query.result"
			| "git.workspace.query.result"
			| "guidance.query.result"
			| "model_behaviour.query.result"
			| "orchestration.graph.query.result"
			| "orchestration.group.list.query.result"
			| "preview.asset.metadata.query.result"
			| "preview.browser.launch.result"
			| "preview.inspection.close.result"
			| "preview.inspection.inspect.result"
			| "preview.inspection.open.result"
			| "preview.rich_link.resolve.query.result"
			| "preview.target.get.query.result"
			| "preview.target.list.query.result"
			| "preview.target.mutation.result"
			| "terminal.list.query.result"
			| "thread.list.query.result"
			| "thread.transcript.query.result"
			| "thread.retention.query.result"
			| "thread.work.query.result"
			| "workspace.file.read.query.result"
			| "workspace.file.discovery.query.result"
			| "workspace.language.capabilities.query.result"
			| "workspace.change.list.query.result"
			| "workspace.change.diff.query.result";
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
