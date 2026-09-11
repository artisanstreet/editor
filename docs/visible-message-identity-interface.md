# Visible message identity interface (FROZEN)

Native message echo correlation through the existing domain/protocol/DB
projection. Root finding: native `run_launch` mints a separately-minted
`item_id` alongside `source_message_id`, while `UserMessageItem` /
`MultimodalUserMessageItem` omit the source id — so the frontend cannot
truthfully match `receipt.message_id` to a user echo. Guessing via
body/timestamps is rejected. Existing item identities never change.

## 1. Domain (`modules/domain/src/conversation.rs`)

```rust
pub struct UserMessageItem {
    pub item_id: ItemId,
    /// Original queued message identity for truthful receipt echo
    /// correlation. `None` for rows projected before this field existed.
    pub source_message_id: Option<MessageId>,
    pub turn_id: TurnId,
    // ... rest unchanged, in existing order
}

pub struct MultimodalUserMessageItem {
    pub item_id: ItemId,
    /// Original queued message identity for truthful receipt echo
    /// correlation. `None` for rows projected before this field existed.
    pub source_message_id: Option<MessageId>,
    pub turn_id: TurnId,
    // ... rest unchanged, in existing order
}
```

`None` is the compatibility value for old wire payloads and legacy
fixtures. No constructor changes: both structs keep public fields and
plain struct literals; call sites name the new field explicitly.

## 2. Wire (`modules/protocol/schema/artisan.capnp`, codec)

Optional `Text` field appended to BOTH structs using the next UNUSED
ordinal; every existing tag/ordinal is frozen:

```capnp
struct UserMessageItem {
  // ... @0..@7 unchanged ...
  sourceMessageId @8 :Text;
}

struct MultimodalUserMessageItem {
  // ... @0..@9 unchanged ...
  sourceMessageId @10 :Text;
}
```

Decode rule: empty/absent field ⇒ `None` (legacy payloads, old
fixtures). Present non-empty field ⇒ `MessageId::parse`, typed
`ProtocolDecodeError` on invalid text — never a fabricated id, never a
silent drop. Encode: `Some(id)` writes the text; `None` leaves the field
unset (an explicitly empty string also decodes to `None`).

Regen note (root-owned): `modules/protocol/src/artisan_capnp.rs` is a
checked-in mirror of the pinned generator (`scripts/capnp_codegen`,
capnpc 0.27.0). The schema and codec changes in this packet require the
established follow-up `protocol: regenerate … bindings` commit before
the gate compiles; codec code references the deterministic accessor
names (`get/set/has_source_message_id`) the generator produces.

## 3. Database (no migration, no new table, no identity rewrite)

`conversation_items.source_message_id` is already queried in both
projection paths; this packet only projects it into the new domain
field, for text-only AND multimodal user items:

- `conversation_projection.rs` snapshot path: `Some(parsed)` (the
  existing fail-closed parse stays: corrupt source ids still reject
  the row).
- `conversation_patch_replay.rs` replay path: `Some(parsed)` when the
  patch carries a source id; the existing legacy-absent branch keeps
  building a text-only `UserMessage` with `source_message_id: None`.

## 4. Frontend correlation (other workers)

With this field, the frontend matches `receipt.message_id` against
`item.source_message_id` (`Some` equality) instead of body/timestamp
heuristics. Repeated identical bodies stay distinct because each queued
message mints its own `MessageId`, and `item_id != source_message_id`
by construction (separately minted). Frontend selection/rendering
workers own the match-site updates.

## 5. Explicit non-goals

- No change to existing item identities, ordinals, lifecycles, or bodies.
- No migration, no new table, no message-identity rewrite.
- No backend runtime changes (`run_launch` minting untouched).
- No guessing, no body/timestamp correlation anywhere.
- No `take`-style or consuming accessors; plain public optional field.
