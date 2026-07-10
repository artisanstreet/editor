/** Re-exports shared primitives for existing direct messages-module consumers. */
export {
	Identifier,
	IsoDateTime,
	NegotiatedProtocolVersion,
	ProtocolVersion,
	RawOrigin,
	SchemaVersion,
} from "./common";

/** Re-exports legacy command, receipt, event, and error schemas. */
export {
	AcceptedCommandReceiptPayload,
	CommandEnvelope,
	CommandPayload,
	CommandReceiptEnvelope,
	CommandReceiptPayload,
	ControlEnvelope,
	EventEnvelope,
	EventPayload,
	OutboundEnvelope,
	ProtocolErrorDetail,
	ProtocolErrorEnvelope,
	RejectedCommandReceiptPayload,
	ThreadCreateCommand,
	ThreadCreatedEvent,
	WireEnvelope,
} from "./control";
