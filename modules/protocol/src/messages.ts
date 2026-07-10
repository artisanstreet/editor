import { Schema } from "effect";

export const ProtocolVersion = Schema.Literal(1);

export const SchemaVersion = Schema.Literal(1);

export const Identifier = Schema.String.check(
	Schema.isPattern(/^\S+$/, {
		message: "Expected a non-empty identifier without whitespace",
	}),
);

export const IsoDateTime = Schema.String.check(
	Schema.isPattern(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/, {
		message: "Expected an ISO 8601 UTC timestamp",
	}),
	Schema.makeFilter<string>((input) => {
		const date = new Date(input);
		const normalized = input.includes(".") ? input : input.replace("Z", ".000Z");

		return Number.isNaN(date.getTime()) || date.toISOString() !== normalized
			? "Expected a real ISO 8601 UTC timestamp"
			: undefined;
	}),
);

export const RawOrigin = Schema.Struct({
	provider: Identifier,
	reference: Identifier,
});

export type RawOrigin = typeof RawOrigin.Type;

export const ThreadCreateCommand = Schema.Struct({
	type: Schema.Literal("thread.create"),
	title: Schema.NonEmptyString,
});

export type ThreadCreateCommand = typeof ThreadCreateCommand.Type;

export const CommandPayload = Schema.Union([ThreadCreateCommand]);

export type CommandPayload = typeof CommandPayload.Type;

export const CommandEnvelope = Schema.Struct({
	protocol_version: ProtocolVersion,
	schema_version: SchemaVersion,
	kind: Schema.Literal("command"),
	message_id: Identifier,
	thread_id: Identifier,
	run_id: Schema.optional(Identifier),
	agent_id: Schema.optional(Identifier),
	causation_id: Schema.optional(Identifier),
	origin: Schema.Literal("frontend"),
	raw_origin: Schema.optional(RawOrigin),
	sent_at: IsoDateTime,
	payload: CommandPayload,
});

export type CommandEnvelope = typeof CommandEnvelope.Type;

export const ProtocolErrorDetail = Schema.Struct({
	code: Identifier,
	message: Schema.NonEmptyString,
	retryable: Schema.Boolean,
});

export type ProtocolErrorDetail = typeof ProtocolErrorDetail.Type;

export const AcceptedCommandReceiptPayload = Schema.Struct({
	status: Schema.Literals(["accepted", "duplicate"]),
	journal_sequence: Schema.Int,
});

export const RejectedCommandReceiptPayload = Schema.Struct({
	status: Schema.Literal("rejected"),
	error: ProtocolErrorDetail,
});

export const CommandReceiptPayload = Schema.Union([
	AcceptedCommandReceiptPayload,
	RejectedCommandReceiptPayload,
]);

export type CommandReceiptPayload = typeof CommandReceiptPayload.Type;

export const CommandReceiptEnvelope = Schema.Struct({
	protocol_version: ProtocolVersion,
	schema_version: SchemaVersion,
	kind: Schema.Literal("command.receipt"),
	message_id: Identifier,
	correlation_id: Identifier,
	thread_id: Identifier,
	run_id: Schema.optional(Identifier),
	agent_id: Schema.optional(Identifier),
	causation_id: Identifier,
	origin: Schema.Literal("backend"),
	sent_at: IsoDateTime,
	payload: CommandReceiptPayload,
});

export type CommandReceiptEnvelope = typeof CommandReceiptEnvelope.Type;

export const ThreadCreatedEvent = Schema.Struct({
	type: Schema.Literal("thread.created"),
	title: Schema.NonEmptyString,
});

export type ThreadCreatedEvent = typeof ThreadCreatedEvent.Type;

export const EventPayload = Schema.Union([ThreadCreatedEvent]);

export type EventPayload = typeof EventPayload.Type;

export const EventEnvelope = Schema.Struct({
	protocol_version: ProtocolVersion,
	schema_version: SchemaVersion,
	kind: Schema.Literal("event"),
	message_id: Identifier,
	correlation_id: Identifier,
	causation_id: Identifier,
	stream_id: Identifier,
	sequence: Schema.Int,
	journal_sequence: Schema.Int,
	thread_id: Identifier,
	run_id: Schema.optional(Identifier),
	agent_id: Schema.optional(Identifier),
	origin: Schema.Literal("backend"),
	raw_origin: Schema.optional(RawOrigin),
	sent_at: IsoDateTime,
	payload: EventPayload,
});

export type EventEnvelope = typeof EventEnvelope.Type;

export const ProtocolErrorEnvelope = Schema.Struct({
	protocol_version: ProtocolVersion,
	schema_version: SchemaVersion,
	kind: Schema.Literal("protocol.error"),
	message_id: Identifier,
	correlation_id: Schema.optional(Identifier),
	thread_id: Schema.optional(Identifier),
	causation_id: Schema.optional(Identifier),
	origin: Schema.Literal("backend"),
	sent_at: IsoDateTime,
	payload: ProtocolErrorDetail,
});

export type ProtocolErrorEnvelope = typeof ProtocolErrorEnvelope.Type;

export const OutboundEnvelope = Schema.Union([
	CommandReceiptEnvelope,
	EventEnvelope,
	ProtocolErrorEnvelope,
]);

export type OutboundEnvelope = typeof OutboundEnvelope.Type;

export const WireEnvelope = Schema.Union([
	CommandEnvelope,
	CommandReceiptEnvelope,
	EventEnvelope,
	ProtocolErrorEnvelope,
]);

export type WireEnvelope = typeof WireEnvelope.Type;
