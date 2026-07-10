import { Schema } from "effect";

import {
	Identifier,
	IsoDateTime,
	JournalSequence,
	NegotiatedProtocolVersion,
	PositiveInt,
	ProtocolVersion,
	RawOrigin,
	SchemaVersion,
	StreamCursor,
	StreamSequence,
} from "./common";

const FrontendTraceMetadata = {
	message_id: Identifier,
	origin: Schema.Literal("frontend"),
	schema_version: SchemaVersion,
	sent_at: IsoDateTime,
};

const BackendTraceMetadata = {
	message_id: Identifier,
	origin: Schema.Literal("backend"),
	schema_version: SchemaVersion,
	sent_at: IsoDateTime,
};

const NegotiatedFrontendTraceMetadata = {
	...FrontendTraceMetadata,
	protocol_version: NegotiatedProtocolVersion,
};

const NegotiatedBackendTraceMetadata = {
	...BackendTraceMetadata,
	protocol_version: NegotiatedProtocolVersion,
};

const EnvironmentVariableName = Schema.String.check(
	Schema.makeFilter<string>((name) =>
		name.length === 0 || name.includes("=") || name.includes(String.fromCharCode(0))
			? "Expected a non-empty environment variable name without equals or null"
			: undefined,
	),
);

/** Describes the thread projection sent in list results and subscriptions. */
export const ThreadListItem = Schema.Struct({
	created_at: IsoDateTime,
	thread_id: Identifier,
	title: Schema.NonEmptyString,
	updated_at: IsoDateTime,
});

export type ThreadListItem = typeof ThreadListItem.Type;

/** Defines the currently supported command payloads. */
export const ThreadCreateCommand = Schema.Struct({
	type: Schema.Literal("thread.create"),
	title: Schema.NonEmptyString,
});

export type ThreadCreateCommand = typeof ThreadCreateCommand.Type;

/** Queues user text for a thread or steers its active capable run. */
export const ThreadSendMessageCommand = Schema.Struct({
	type: Schema.Literal("thread.send_message"),
	engine_id: Identifier,
	text: Schema.NonEmptyString,
	working_directory: Schema.NonEmptyString,
});

export type ThreadSendMessageCommand = typeof ThreadSendMessageCommand.Type;

/** Opens a durable pseudoterminal owned by a thread workspace. */
export const TerminalOpenCommand = Schema.Struct({
	type: Schema.Literal("terminal.open"),
	terminal_id: Identifier,
	workspace_id: Identifier,
	working_directory: Schema.NonEmptyString,
	executable: Schema.NonEmptyString,
	args: Schema.Array(Schema.String),
	env: Schema.optional(Schema.Record(EnvironmentVariableName, Schema.String)),
	cols: PositiveInt,
	rows: PositiveInt,
});

/** Writes UTF-8 text to one active terminal. */
export const TerminalWriteCommand = Schema.Struct({
	type: Schema.Literal("terminal.write"),
	terminal_id: Identifier,
	data: Schema.String,
});

/** Changes the visible dimensions of one active terminal. */
export const TerminalResizeCommand = Schema.Struct({
	type: Schema.Literal("terminal.resize"),
	terminal_id: Identifier,
	cols: PositiveInt,
	rows: PositiveInt,
});

/** Clears the visible screen buffer of one active terminal. */
export const TerminalClearCommand = Schema.Struct({
	type: Schema.Literal("terminal.clear"),
	terminal_id: Identifier,
});

/** Sends a termination signal to one active terminal. */
export const TerminalKillCommand = Schema.Struct({
	type: Schema.Literal("terminal.kill"),
	terminal_id: Identifier,
	signal: Schema.optional(Schema.String),
});

/** Closes one terminal and releases its native PTY resource. */
export const TerminalCloseCommand = Schema.Struct({
	type: Schema.Literal("terminal.close"),
	terminal_id: Identifier,
});

/** Starts a new PTY generation from a closed or failed terminal's saved configuration. */
export const TerminalRestartCommand = Schema.Struct({
	type: Schema.Literal("terminal.restart"),
	terminal_id: Identifier,
});

/** Steers the currently active run using provider-neutral user text. */
export const RunSteerCommand = Schema.Struct({
	type: Schema.Literal("run.steer"),
	text: Schema.NonEmptyString,
});

/** Requests cancellation of the currently active run. */
export const RunCancelCommand = Schema.Struct({ type: Schema.Literal("run.cancel") });

/** Closes the currently active run and releases its live resources. */
export const RunCloseCommand = Schema.Struct({ type: Schema.Literal("run.close") });

/** Resolves one durable approval interaction. */
export const RunRespondApprovalCommand = Schema.Struct({
	type: Schema.Literal("run.respond_approval"),
	approval_id: Identifier,
	approved: Schema.Boolean,
});

/** Resolves one durable question interaction. */
export const RunRespondQuestionCommand = Schema.Struct({
	type: Schema.Literal("run.respond_question"),
	answers: Schema.Record(Identifier, Schema.NonEmptyArray(Schema.NonEmptyString)),
});

/** Unions every command payload accepted by the V1 control channel. */
export const CommandPayload = Schema.Union([
	ThreadCreateCommand,
	ThreadSendMessageCommand,
	TerminalOpenCommand,
	TerminalWriteCommand,
	TerminalResizeCommand,
	TerminalClearCommand,
	TerminalKillCommand,
	TerminalCloseCommand,
	TerminalRestartCommand,
	RunSteerCommand,
	RunCancelCommand,
	RunCloseCommand,
	RunRespondApprovalCommand,
	RunRespondQuestionCommand,
]);

export type CommandPayload = typeof CommandPayload.Type;

/** Describes a typed protocol error that can be safely shown or retried by a client. */
export const ProtocolErrorDetail = Schema.Struct({
	code: Identifier,
	message: Schema.NonEmptyString,
	retryable: Schema.Boolean,
});

export type ProtocolErrorDetail = typeof ProtocolErrorDetail.Type;

/** Defines a pre-negotiation client hello without a selected protocol version. */
export const HelloEnvelope = Schema.Struct({
	...FrontendTraceMetadata,
	kind: Schema.Literal("hello"),
	payload: Schema.Struct({
		event_cursors: Schema.Array(StreamCursor),
		last_journal_sequence: JournalSequence,
		supported_protocol_versions: Schema.NonEmptyArray(ProtocolVersion),
	}),
});

export type HelloEnvelope = typeof HelloEnvelope.Type;

/** Confirms the negotiated version and exposes the backend's current replay cursors. */
export const WelcomeEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("welcome"),
	payload: Schema.Struct({
		connection_id: Identifier,
		current_event_cursors: Schema.Array(StreamCursor),
		heartbeat_interval_ms: PositiveInt,
		heartbeat_timeout_ms: PositiveInt,
		journal_sequence: JournalSequence,
		stream_ticket: Identifier,
	}),
});

export type WelcomeEnvelope = typeof WelcomeEnvelope.Type;

/** Carries a durable frontend command into the backend command router. */
export const CommandEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	causation_id: Schema.optional(Identifier),
	kind: Schema.Literal("command"),
	payload: CommandPayload,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type CommandEnvelope = typeof CommandEnvelope.Type;

/** Captures an accepted or duplicate command receipt persisted by the journal. */
export const AcceptedCommandReceiptPayload = Schema.Struct({
	journal_sequence: JournalSequence,
	status: Schema.Literals(["accepted", "duplicate"]),
});

/** Captures a command rejection that was not durably accepted. */
export const RejectedCommandReceiptPayload = Schema.Struct({
	error: ProtocolErrorDetail,
	status: Schema.Literal("rejected"),
});

/** Unions every possible command receipt outcome. */
export const CommandReceiptPayload = Schema.Union([
	AcceptedCommandReceiptPayload,
	RejectedCommandReceiptPayload,
]);

export type CommandReceiptPayload = typeof CommandReceiptPayload.Type;

/** Returns the durable acceptance status for a correlated command. */
export const CommandReceiptEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	causation_id: Identifier,
	correlation_id: Identifier,
	kind: Schema.Literal("command.receipt"),
	payload: CommandReceiptPayload,
	run_id: Schema.optional(Identifier),
	thread_id: Identifier,
});

export type CommandReceiptEnvelope = typeof CommandReceiptEnvelope.Type;

/** Defines the event payload emitted when a thread is created. */
export const ThreadCreatedEvent = Schema.Struct({
	type: Schema.Literal("thread.created"),
	title: Schema.NonEmptyString,
});

export type ThreadCreatedEvent = typeof ThreadCreatedEvent.Type;

/** Records user text that is durably queued for a future run. */
export const ThreadMessageQueuedEvent = Schema.Struct({
	type: Schema.Literal("thread.message_queued"),
	message_id: Identifier,
	reason: Schema.Literals(["no_active_run", "steering_rejected", "unsupported"]),
	text: Schema.NonEmptyString,
});

/** Records user text accepted as a steering request for a live run. */
export const ThreadMessageSteeringEvent = Schema.Struct({
	type: Schema.Literal("thread.message_steering"),
	message_id: Identifier,
	text: Schema.NonEmptyString,
});

/** Records an authoritative lifecycle state for one durable run. */
export const RunLifecycleEvent = Schema.Struct({
	type: Schema.Literal("run.lifecycle"),
	state: Schema.Literals([
		"queued",
		"running",
		"waiting",
		"interrupted",
		"completed",
		"cancelled",
		"failed",
		"closed",
	]),
});

/** Persists the complete assistant response while omitting transient deltas. */
export const AssistantMessageCompletedEvent = Schema.Struct({
	type: Schema.Literal("assistant.message_completed"),
	message_id: Identifier,
	text: Schema.NonEmptyString,
});

/** Persists a provider-neutral approval request or its response. */
export const ApprovalInteractionEvent = Schema.Struct({
	type: Schema.Literal("interaction.approval"),
	approval_id: Identifier,
	approved: Schema.optional(Schema.Boolean),
	description: Schema.NonEmptyString,
	state: Schema.Literals(["requested", "resolved"]),
});

/** Persists a provider-neutral question request or its response. */
export const QuestionInteractionEvent = Schema.Struct({
	type: Schema.Literal("interaction.question"),
	answers: Schema.optional(
		Schema.Record(Identifier, Schema.NonEmptyArray(Schema.NonEmptyString)),
	),
	question_id: Identifier,
	state: Schema.Literals(["requested", "resolved"]),
	text: Schema.NonEmptyString,
});

/** Describes durable metadata for a terminal session. */
export const TerminalSession = Schema.Struct({
	terminal_id: Identifier,
	thread_id: Identifier,
	workspace_id: Identifier,
	working_directory: Schema.NonEmptyString,
	executable: Schema.NonEmptyString,
	args: Schema.Array(Schema.String),
	cols: PositiveInt,
	generation: PositiveInt,
	rows: PositiveInt,
	pid: Schema.optional(PositiveInt),
	state: Schema.Literals(["opening", "active", "closed", "failed"]),
	exit_code: Schema.optional(Schema.Int),
	exit_signal: Schema.optional(Schema.Int),
	exit_reason: Schema.optional(
		Schema.Literals(["closed", "exited", "killed", "output_overflow"]),
	),
	failure: Schema.optional(Schema.NonEmptyString),
	created_at: IsoDateTime,
	updated_at: IsoDateTime,
	closed_at: Schema.optional(IsoDateTime),
});

export type TerminalSession = typeof TerminalSession.Type;

/** Records a durable terminal lifecycle transition. */
export const TerminalLifecycleEvent = Schema.Struct({
	type: Schema.Literal("terminal.lifecycle"),
	action: Schema.Literals([
		"opened",
		"written",
		"resized",
		"cleared",
		"killed",
		"closed",
		"restarted",
		"exited",
		"failed",
		"recovered",
	]),
	terminal: TerminalSession,
});

export type TerminalLifecycleEvent = typeof TerminalLifecycleEvent.Type;

/** Unions every durable event payload emitted by the V1 backend. */
export const EventPayload = Schema.Union([
	ThreadCreatedEvent,
	ThreadMessageQueuedEvent,
	ThreadMessageSteeringEvent,
	RunLifecycleEvent,
	AssistantMessageCompletedEvent,
	ApprovalInteractionEvent,
	QuestionInteractionEvent,
	TerminalLifecycleEvent,
]);

export type EventPayload = typeof EventPayload.Type;

/** Delivers one durable fact with global and stream-local ordering metadata. */
export const EventEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	agent_id: Schema.optional(Identifier),
	causation_id: Identifier,
	correlation_id: Identifier,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("event"),
	payload: EventPayload,
	raw_origin: Schema.optional(RawOrigin),
	run_id: Schema.optional(Identifier),
	sequence: StreamSequence,
	stream_id: Identifier,
	thread_id: Identifier,
});

export type EventEnvelope = typeof EventEnvelope.Type;

/** Reports a malformed or otherwise unprocessable control frame. */
export const ProtocolErrorEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	causation_id: Schema.optional(Identifier),
	correlation_id: Schema.optional(Identifier),
	kind: Schema.Literal("protocol.error"),
	payload: ProtocolErrorDetail,
	thread_id: Schema.optional(Identifier),
});

export type ProtocolErrorEnvelope = typeof ProtocolErrorEnvelope.Type;

/** Reports a negotiation failure before a protocol version can be selected. */
export const PreNegotiationProtocolErrorEnvelope = Schema.Struct({
	...BackendTraceMetadata,
	causation_id: Schema.optional(Identifier),
	correlation_id: Schema.optional(Identifier),
	kind: Schema.Literal("protocol.error"),
	payload: ProtocolErrorDetail,
});

export type PreNegotiationProtocolErrorEnvelope = typeof PreNegotiationProtocolErrorEnvelope.Type;

/** Requests the current thread-list projection from the backend. */
export const ThreadListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.list.query"),
	payload: Schema.Struct({}),
});

export type ThreadListQueryEnvelope = typeof ThreadListQueryEnvelope.Type;

/** Returns the current thread-list projection for a correlated query. */
export const ThreadListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.list.query.result"),
	payload: Schema.Struct({
		journal_sequence: JournalSequence,
		threads: Schema.Array(ThreadListItem),
	}),
});

export type ThreadListQueryResultEnvelope = typeof ThreadListQueryResultEnvelope.Type;

/** Describes the durable work state coordinated for one thread. */
export const ThreadWorkItem = Schema.Struct({
	agent_id: Identifier,
	display_name: Schema.NonEmptyString,
	engine_id: Identifier,
	native_thread_id: Schema.optional(Identifier),
	role: Schema.NonEmptyString,
	run_id: Identifier,
	status: Schema.Literals([
		"queued",
		"running",
		"waiting",
		"interrupted",
		"completed",
		"cancelled",
		"failed",
		"closed",
	]),
});

export type ThreadWorkItem = typeof ThreadWorkItem.Type;

/** Requests the current durable coordinator work for one thread. */
export const ThreadWorkQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("thread.work.query"),
	payload: Schema.Struct({ thread_id: Identifier }),
});

export type ThreadWorkQueryEnvelope = typeof ThreadWorkQueryEnvelope.Type;

/** Returns the durable coordinator work for one thread. */
export const ThreadWorkQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("thread.work.query.result"),
	payload: Schema.Struct({ work: Schema.optional(ThreadWorkItem) }),
});

export type ThreadWorkQueryResultEnvelope = typeof ThreadWorkQueryResultEnvelope.Type;

/** Requests terminal metadata for one thread workspace. */
export const TerminalListQueryEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("terminal.list.query"),
	payload: Schema.Struct({ thread_id: Identifier, workspace_id: Identifier }),
});

export type TerminalListQueryEnvelope = typeof TerminalListQueryEnvelope.Type;

/** Returns durable terminal metadata without replaying transient PTY output. */
export const TerminalListQueryResultEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("terminal.list.query.result"),
	payload: Schema.Struct({ terminals: Schema.Array(TerminalSession) }),
});

export type TerminalListQueryResultEnvelope = typeof TerminalListQueryResultEnvelope.Type;

/** Requests ordered updates for the thread-list projection. */
export const SubscribeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("subscribe"),
	payload: Schema.Struct({
		type: Schema.Literal("thread.list"),
	}),
	subscription_id: Identifier,
});

export type SubscribeEnvelope = typeof SubscribeEnvelope.Type;

/** Stops delivery for a previously established projection subscription. */
export const UnsubscribeEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("unsubscribe"),
	payload: Schema.Struct({}),
	subscription_id: Identifier,
});

export type UnsubscribeEnvelope = typeof UnsubscribeEnvelope.Type;

/** Confirms that the backend has established an ordered projection subscription. */
export const SubscriptionStartedEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("subscription.started"),
	payload: Schema.Struct({
		stream_id: Identifier,
	}),
	subscription_id: Identifier,
});

export type SubscriptionStartedEnvelope = typeof SubscriptionStartedEnvelope.Type;

/** Confirms that the backend has stopped an ordered projection subscription. */
export const SubscriptionStoppedEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("subscription.stopped"),
	payload: Schema.Struct({}),
	subscription_id: Identifier,
});

export type SubscriptionStoppedEnvelope = typeof SubscriptionStoppedEnvelope.Type;

/** Provides ephemeral thread-list state; reconnecting clients request a fresh snapshot. */
export const ThreadListSnapshotEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.list.snapshot"),
	payload: Schema.Struct({
		threads: Schema.Array(ThreadListItem),
	}),
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type ThreadListSnapshotEnvelope = typeof ThreadListSnapshotEnvelope.Type;

/** Delivers one connection-local thread-list upsert after a subscription snapshot. */
export const ThreadListUpsertEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	journal_sequence: JournalSequence,
	kind: Schema.Literal("thread.list.upsert"),
	payload: ThreadListItem,
	sequence: StreamSequence,
	stream_id: Identifier,
	subscription_id: Identifier,
});

export type ThreadListUpsertEnvelope = typeof ThreadListUpsertEnvelope.Type;

/** Acknowledges the highest contiguous journal position and durable event cursors. */
export const AckEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("ack"),
	payload: Schema.Struct({
		event_cursors: Schema.Array(StreamCursor),
		journal_sequence: JournalSequence,
	}),
});

export type AckEnvelope = typeof AckEnvelope.Type;

/** Requests durable replay after a global journal cursor with optional stream checks. */
export const ReplayEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	kind: Schema.Literal("replay"),
	payload: Schema.Struct({
		after_journal_sequence: JournalSequence,
		event_cursors: Schema.optional(Schema.Array(StreamCursor)),
	}),
});

export type ReplayEnvelope = typeof ReplayEnvelope.Type;

/** Marks a replay boundary and reports the current global and stream positions. */
export const ReplayCompleteEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("replay.complete"),
	payload: Schema.Struct({
		current_event_cursors: Schema.Array(StreamCursor),
		journal_sequence: JournalSequence,
	}),
});

export type ReplayCompleteEnvelope = typeof ReplayCompleteEnvelope.Type;

/** Checks client liveness from the backend side of an idle control connection. */
export const HeartbeatPingEnvelope = Schema.Struct({
	...NegotiatedBackendTraceMetadata,
	kind: Schema.Literal("heartbeat.ping"),
	payload: Schema.Struct({
		nonce: Identifier,
	}),
});

export type HeartbeatPingEnvelope = typeof HeartbeatPingEnvelope.Type;

/** Responds from the frontend using the backend ping identifier and nonce. */
export const HeartbeatPongEnvelope = Schema.Struct({
	...NegotiatedFrontendTraceMetadata,
	correlation_id: Identifier,
	kind: Schema.Literal("heartbeat.pong"),
	payload: Schema.Struct({
		nonce: Identifier,
	}),
});

export type HeartbeatPongEnvelope = typeof HeartbeatPongEnvelope.Type;

/** Decodes every client-to-backend frame accepted on the control channel. */
export const InboundControlEnvelope = Schema.Union([
	HelloEnvelope,
	CommandEnvelope,
	ThreadListQueryEnvelope,
	ThreadWorkQueryEnvelope,
	TerminalListQueryEnvelope,
	SubscribeEnvelope,
	UnsubscribeEnvelope,
	AckEnvelope,
	ReplayEnvelope,
	HeartbeatPongEnvelope,
]);

export type InboundControlEnvelope = typeof InboundControlEnvelope.Type;

/** Encodes every backend-to-client frame emitted on the control channel. */
export const OutboundControlEnvelope = Schema.Union([
	WelcomeEnvelope,
	PreNegotiationProtocolErrorEnvelope,
	CommandReceiptEnvelope,
	EventEnvelope,
	ProtocolErrorEnvelope,
	ThreadListQueryResultEnvelope,
	ThreadWorkQueryResultEnvelope,
	TerminalListQueryResultEnvelope,
	SubscriptionStartedEnvelope,
	SubscriptionStoppedEnvelope,
	ThreadListSnapshotEnvelope,
	ThreadListUpsertEnvelope,
	ReplayCompleteEnvelope,
	HeartbeatPingEnvelope,
]);

export type OutboundControlEnvelope = typeof OutboundControlEnvelope.Type;

/** Represents every validated V1 control-channel frame in either direction. */
export const ControlEnvelope = Schema.Union([InboundControlEnvelope, OutboundControlEnvelope]);

export type ControlEnvelope = typeof ControlEnvelope.Type;

/** Preserves the legacy name for backend-originated control frames. */
export const OutboundEnvelope = OutboundControlEnvelope;

export type OutboundEnvelope = typeof OutboundEnvelope.Type;

/** Preserves the legacy name for control frames in either direction. */
export const WireEnvelope = ControlEnvelope;

export type WireEnvelope = typeof WireEnvelope.Type;
