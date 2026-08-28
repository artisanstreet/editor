# Phase 1 feasibility fixture only.
#
# This schema exists to prove that Bazel owns Cap'n Proto code generation as
# an explicit hermetic action feeding Rust compilation inputs. It encodes no
# product API decisions; the Phase 2 protocol design replaces or removes it.
#
# Schema ID: placeholder chosen for this fixture; regenerate a random ID with
# `capnp id` before reusing this template elsewhere.

@0xa93d5ce74b1f8c2e;

struct Phase1ProofEnvelope {
  correlationId @0 :UInt64;
  payload @1 :Text;
  attemptNumbers @2 :List(UInt32);
}
