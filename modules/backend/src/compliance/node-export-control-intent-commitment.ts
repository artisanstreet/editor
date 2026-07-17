import { createHmac } from "node:crypto";

import { Effect, Layer, Redacted } from "effect";

import {
	ExportControlIntentCommitment,
	ExportControlIntentCommitmentFailure,
} from "./export-control";

const text_encoder = new TextEncoder();

const AcquireKey = (secret: Redacted.Redacted<Uint8Array>) =>
	Effect.try({
		try: () => {
			const source = Redacted.value(secret);

			if (!(source instanceof Uint8Array) || source.byteLength !== 32) {
				throw new Error("Export-control commitment key must contain exactly 32 bytes");
			}

			return Uint8Array.from(source);
		},
		catch: (cause) => new ExportControlIntentCommitmentFailure({ cause }),
	});

/**
 * Builds scoped HMAC-SHA-256 commitments from an OS-protected key.
 *
 * Effect Crypto in the pinned Effect 4 beta exposes unkeyed digests but no HMAC primitive, so this
 * focused Node adapter owns the missing platform operation while the domain depends only on a Service.
 */
export function make_node_export_control_intent_commitment_layer(
	secret: Redacted.Redacted<Uint8Array>,
) {
	const Acquire = Effect.acquireRelease(AcquireKey(secret), (key) =>
		Effect.sync(() => key.fill(0)),
	).pipe(
		Effect.map((key) => ({
			Fingerprint: (canonical_intent: string) =>
				Effect.try({
					try: () =>
						createHmac("sha256", key)
							.update(text_encoder.encode(canonical_intent))
							.digest("hex"),
					catch: (cause) => new ExportControlIntentCommitmentFailure({ cause }),
				}),
		})),
	);

	return Layer.effect(ExportControlIntentCommitment, Acquire);
}
