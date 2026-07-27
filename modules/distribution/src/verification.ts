import { createHash, createPublicKey, verify } from "node:crypto";

import { Context, Data, Effect, Layer, Schema } from "effect";

import { type ReleaseArtifact, ReleaseManifest } from "./release-manifest.ts";

export const ReleaseManifestSignature = Schema.Struct({
	algorithm: Schema.Literal("ed25519"),
	key_id: Schema.NonEmptyString,
	signature: Schema.String.check(
		Schema.isPattern(/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/),
	),
});
export type ReleaseManifestSignature = typeof ReleaseManifestSignature.Type;

export class ReleaseVerificationError extends Data.TaggedError("ReleaseVerificationError")<{
	readonly cause?: unknown;
	readonly code:
		| "artifact_hash"
		| "artifact_size"
		| "invalid_manifest"
		| "invalid_signature"
		| "signing_identity_mismatch"
		| "untrusted_key";
}> {}

export class TrustedReleaseKeys extends Context.Service<
	TrustedReleaseKeys,
	ReadonlyMap<string, Uint8Array>
>()("Artisan/Distribution/TrustedReleaseKeys") {}

export class ReleaseCryptography extends Context.Service<
	ReleaseCryptography,
	{
		readonly Sha256: (bytes: Uint8Array) => Effect.Effect<string, ReleaseVerificationError>;
		readonly VerifyEd25519: (
			public_key: Uint8Array,
			message: Uint8Array,
			signature: Uint8Array,
		) => Effect.Effect<boolean, ReleaseVerificationError>;
	}
>()("Artisan/Distribution/ReleaseCryptography") {}

export class ReleaseVerification extends Context.Service<
	ReleaseVerification,
	{
		readonly VerifyArtifact: (
			artifact: ReleaseArtifact,
			bytes: Uint8Array,
		) => Effect.Effect<void, ReleaseVerificationError>;
		readonly VerifyManifest: (
			raw_manifest: Uint8Array,
			signature: unknown,
		) => Effect.Effect<ReleaseManifest, ReleaseVerificationError>;
	}
>()("Artisan/Distribution/ReleaseVerification") {}

export const make_trusted_release_keys_layer = (keys: Readonly<Record<string, Uint8Array>>) =>
	Layer.succeed(TrustedReleaseKeys, new Map(Object.entries(keys)));

/** Node cryptography is isolated behind this adapter; domain verification remains injectable. */
export const NodeReleaseCryptographyLive = Layer.succeed(
	ReleaseCryptography,
	ReleaseCryptography.of({
		Sha256: (bytes) =>
			Effect.try({
				try: () => createHash("sha256").update(bytes).digest("hex"),
				catch: (cause) => new ReleaseVerificationError({ cause, code: "artifact_hash" }),
			}),
		VerifyEd25519: (public_key, message, signature) =>
			Effect.try({
				try: () =>
					verify(
						null,
						message,
						createPublicKey({
							format: "der",
							key: public_key,
							type: "spki",
						}),
						signature,
					),
				catch: (cause) =>
					new ReleaseVerificationError({ cause, code: "invalid_signature" }),
			}),
	}),
);

const DecodeBase64 = (value: string) =>
	Effect.try({
		try: () => {
			const bytes = Buffer.from(value, "base64");
			if (bytes.length !== 64 || bytes.toString("base64") !== value)
				throw new Error("Expected a canonical 64-byte Ed25519 signature");
			return bytes;
		},
		catch: (cause) => new ReleaseVerificationError({ cause, code: "invalid_signature" }),
	});

export const ReleaseVerificationLive = Layer.effect(
	ReleaseVerification,
	Effect.gen(function* () {
		const trusted_keys = yield* TrustedReleaseKeys;
		const cryptography = yield* ReleaseCryptography;

		return ReleaseVerification.of({
			VerifyArtifact: (artifact, bytes) =>
				Effect.gen(function* () {
					if (bytes.byteLength !== artifact.byte_size)
						return yield* Effect.fail(
							new ReleaseVerificationError({ code: "artifact_size" }),
						);
					const digest = yield* cryptography.Sha256(bytes);
					if (digest !== artifact.sha256)
						return yield* Effect.fail(
							new ReleaseVerificationError({ code: "artifact_hash" }),
						);
				}),
			VerifyManifest: (raw_manifest, signature_input) =>
				Effect.gen(function* () {
					const signature = yield* Schema.decodeUnknownEffect(ReleaseManifestSignature)(
						signature_input,
					).pipe(
						Effect.mapError(
							(cause) =>
								new ReleaseVerificationError({
									cause,
									code: "invalid_signature",
								}),
						),
					);
					const trusted_key = trusted_keys.get(signature.key_id);
					if (trusted_key === undefined)
						return yield* Effect.fail(
							new ReleaseVerificationError({ code: "untrusted_key" }),
						);
					const signature_bytes = yield* DecodeBase64(signature.signature);
					const authentic = yield* cryptography.VerifyEd25519(
						trusted_key,
						raw_manifest,
						signature_bytes,
					);
					if (!authentic)
						return yield* Effect.fail(
							new ReleaseVerificationError({ code: "invalid_signature" }),
						);

					const manifest = yield* Effect.try({
						try: () => JSON.parse(new TextDecoder().decode(raw_manifest)) as unknown,
						catch: (cause) =>
							new ReleaseVerificationError({ cause, code: "invalid_manifest" }),
					}).pipe(
						Effect.flatMap(Schema.decodeUnknownEffect(ReleaseManifest)),
						Effect.mapError((cause) =>
							cause instanceof ReleaseVerificationError
								? cause
								: new ReleaseVerificationError({
										cause,
										code: "invalid_manifest",
									}),
						),
					);
					if (
						manifest.signing_identity.key_id !== signature.key_id ||
						manifest.signing_identity.algorithm !== signature.algorithm
					)
						return yield* Effect.fail(
							new ReleaseVerificationError({
								code: "signing_identity_mismatch",
							}),
						);
					return manifest;
				}),
		});
	}),
);
