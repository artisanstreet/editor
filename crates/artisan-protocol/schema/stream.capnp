# Artisan protocol: the ephemeral stream lane.
#
# Replaces modules/protocol/src/stream.ts on transport version 2. One
# StreamFrame models every reserved stream-channel frame; the unnamed union is
# the kind discriminator that `kind: "stream.bind"` et al. carried under
# MessagePack.
#
# Behavioral notes preserved from the legacy definition:
# - `stream.bind` and `stream.ready` payloads repeat the outer stream id; both
#   copies are kept so read models stay byte-for-byte comparable during
#   cross-runtime fixture checks.
# - Legacy base64 chunk encoding becomes native bytes. The TS adapter decodes
#   base64 before building the frame; the Rust side receives raw bytes with no
#   decode step.

@0xb7f1a93d2e8c4057;

using Common = import "common.capnp";

struct StreamFrame {
	messageId @0 :Common.Identifier;        # json: message_id
	origin @1 :Common.Origin;               # json: origin ("frontend" | "backend")
	protocolVersion @2 :Common.ProtocolVersion;  # json: protocol_version
	schemaVersion @3 :Common.SchemaVersion;      # json: schema_version
	sentAt @4 :Common.IsoDateTime;          # json: sent_at

	channelId @5 :Common.Identifier;             # json: channel_id
	channelSequence @6 :UInt64;                  # json: channel_sequence
	streamId @7 :Common.Identifier;              # json: stream_id

	union {
		bind :group {                              # kind: "stream.bind"
			payloadStreamId @8 :Common.Identifier;  # json: payload.stream_id
		}
		ready :group {                             # kind: "stream.ready"
			payloadStreamId @9 :Common.Identifier;   # json: payload.stream_id
		}
		chunk :group {                             # kind: "stream.chunk"
			union {
				text @10 :Text;    # legacy payload { data, encoding: "utf8" }
				bytes @11 :Data;   # legacy payload { data, encoding: "base64" }, decoded
			}
		}
		end :group {                               # kind: "stream.end"
			reason @12 :Common.Identifier;  # json: payload.reason; null = absent
		}
	}
}
