# Artisan protocol: shared scalar and identifier shapes.
#
# Canonical wire definition (Cap'n Proto framing, transport version 2).
# The TypeScript Effect schemas in modules/protocol/src/common.ts remain the
# validation layer; this file is the structure they validate against.
#
# Naming rule: Cap'n Proto identifiers cannot carry underscores, so fields use
# camelCase here. Every field whose legacy MessagePack name differed carries a
# `# json:` comment naming the exact serialized name it replaces. Ordinals are
# part of the compatibility contract: never renumber, only append.

@0x9e4d2c7a5b3f4816;

# Validates a non-empty identifier that is safe to carry across transports.
# Application-side check: non-empty, no whitespace (legacy pattern ^\S+$).
using Identifier = Text;

# Validates a positive integer protocol version offered during negotiation.
# Application-side check: > 0.
using ProtocolVersion = UInt16;

# Validates a positive integer used for bounded timing and capacity values.
# Application-side check: > 0.
using PositiveInt = UInt32;

# Validates the version of the schema carried by every frame.
# Application-side check: == 1.
using SchemaVersion = UInt16;

# Validates a canonical UTC timestamp used by trace metadata.
# Application-side checks: pattern ^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{3})?Z$
# and real-calendar-date verification.
using IsoDateTime = Text;

enum Origin {
	frontend @0;
	backend @1;
}

struct RawOrigin {
	provider @0 :Identifier;   # json: provider
	reference @1 :Identifier;  # json: reference
}

struct StreamCursor {
	sequence @0 :UInt64;       # json: sequence
	streamId @1 :Identifier;   # json: stream_id
}
