# Artisan protocol: control-lane handshake and error frames.
#
# Replaces the lifecycle envelopes in
# modules/protocol/src/control-contract/lifecycle.ts on transport version 2,
# scoped to the negotiation family: hello, welcome, protocol errors. The
# command, receipt, and event families land as later schema packets; their
# union payloads are too large to convert responsibly in one slice.
#
# The `kind` literal each legacy envelope carried becomes the ControlFrame
# union discriminant; it is no longer a serialized field.

@0xc2d85e6f1a94b703;

using Common = import "common.capnp";

struct ProtocolErrorDetail {
	code @0 :Common.Identifier;  # json: code
	message @1 :Text;            # json: message; non-empty, application-checked
	retryable @2 :Bool;          # json: retryable
}

# kind: "hello" — pre-negotiation client hello without a selected version.
struct HelloEnvelope {
	messageId @0 :Common.Identifier;   # json: message_id
	origin @1 :Common.Origin;          # json: origin; always "frontend", app-checked
	schemaVersion @2 :Common.SchemaVersion;  # json: schema_version
	sentAt @3 :Common.IsoDateTime;     # json: sent_at

	payload :group {
		eventCursors @4 :List(Common.StreamCursor);        # json: payload.event_cursors
		lastJournalSequence @5 :UInt64;                    # json: payload.last_journal_sequence
		resumeMode @6 :Text;                               # json: payload.resume_mode;
		                                                   # null = absent, else "fresh" | "resume"
		supportedProtocolVersions @7 :List(Common.ProtocolVersion);  # json: payload.supported_protocol_versions;
		                                                   # non-empty, application-checked
	}
}

# kind: "welcome" — confirms the negotiated version and replay cursors.
struct WelcomeEnvelope {
	messageId @0 :Common.Identifier;         # json: message_id
	origin @1 :Common.Origin;                # json: origin; always "backend", app-checked
	protocolVersion @2 :Common.ProtocolVersion;  # json: protocol_version
	schemaVersion @3 :Common.SchemaVersion;      # json: schema_version
	sentAt @4 :Common.IsoDateTime;           # json: sent_at

	correlationId @5 :Common.Identifier;     # json: correlation_id

	payload :group {
		connectionId @6 :Common.Identifier;              # json: payload.connection_id
		currentEventCursors @7 :List(Common.StreamCursor);  # json: payload.current_event_cursors
		heartbeatIntervalMs @8 :Common.PositiveInt;      # json: payload.heartbeat_interval_ms
		heartbeatTimeoutMs @9 :Common.PositiveInt;       # json: payload.heartbeat_timeout_ms
		journalSequence @10 :UInt64;                     # json: payload.journal_sequence
		streamTicket @11 :Common.Identifier;             # json: payload.stream_ticket
	}
}

# kind: "protocol.error" — typed error after a version was negotiated.
struct ProtocolErrorEnvelope {
	messageId @0 :Common.Identifier;             # json: message_id
	origin @1 :Common.Origin;                    # json: origin; always "backend"
	protocolVersion @2 :Common.ProtocolVersion;  # json: protocol_version
	schemaVersion @3 :Common.SchemaVersion;      # json: schema_version
	sentAt @4 :Common.IsoDateTime;               # json: sent_at

	causationId @5 :Common.Identifier;    # json: causation_id; null = absent
	correlationId @6 :Common.Identifier;  # json: correlation_id; null = absent
	threadId @7 :Common.Identifier;       # json: thread_id; null = absent

	payload @8 :ProtocolErrorDetail;      # json: payload
}

# kind: "protocol.error" before a version was selected — carries no
# protocol_version field at all.
struct PreNegotiationProtocolErrorEnvelope {
	messageId @0 :Common.Identifier;         # json: message_id
	origin @1 :Common.Origin;                # json: origin; always "backend"
	schemaVersion @2 :Common.SchemaVersion;  # json: schema_version
	sentAt @3 :Common.IsoDateTime;           # json: sent_at

	causationId @4 :Common.Identifier;    # json: causation_id; null = absent
	correlationId @5 :Common.Identifier;  # json: correlation_id; null = absent

	payload @6 :ProtocolErrorDetail;      # json: payload
}

# Discriminates control frames during negotiation. Later schema packets append
# command, receipt, and event variants; only ever add to the union.
struct ControlFrame {
	union {
		hello @0 :HelloEnvelope;
		welcome @1 :WelcomeEnvelope;
		protocolError @2 :ProtocolErrorEnvelope;
		preNegotiationProtocolError @3 :PreNegotiationProtocolErrorEnvelope;
	}
}
