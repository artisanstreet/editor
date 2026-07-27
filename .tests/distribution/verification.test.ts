import { createHash, generateKeyPairSync, sign } from "node:crypto";

import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import {
	make_trusted_release_keys_layer,
	NodeReleaseCryptographyLive,
	ReleaseVerification,
	ReleaseVerificationLive,
} from "@artisan/distribution";

const keys = generateKeyPairSync("ed25519");
const key_id = "release-2026";
const public_key = keys.publicKey.export({ format: "der", type: "spki" });
const artifact_bytes = new TextEncoder().encode("artifact");
const manifest = {
	artifacts: [
		{
			archive_entries: ["Artisan Editor.exe"],
			archive_format: "zip",
			architecture: "x64",
			artifact_id: "windows-x64",
			byte_size: artifact_bytes.byteLength,
			file_name: "artisan-windows-x64.zip",
			platform: "windows",
			sha256: createHash("sha256").update(artifact_bytes).digest("hex"),
		},
	],
	channel: "stable",
	editor_forge_compatibility_version: "1.0.0",
	format_version: 1,
	minimum_bootstrap_version: "1.0.0",
	minimum_cli_version: "1.0.0",
	product_version: "1.0.0",
	signing_identity: { algorithm: "ed25519", key_id },
};
const raw_manifest = new TextEncoder().encode(JSON.stringify(manifest));
const signature = sign(null, raw_manifest, keys.privateKey).toString("base64");

const VerificationLive = ReleaseVerificationLive.pipe(
	Layer.provide(NodeReleaseCryptographyLive),
	Layer.provide(make_trusted_release_keys_layer({ [key_id]: public_key })),
);

const Run = <A>(effect: Effect.Effect<A, unknown, ReleaseVerification>) =>
	Effect.runPromise(effect.pipe(Effect.provide(VerificationLive)));

describe("release verification", () => {
	it("verifies exact manifest bytes before decoding and verifies artifact integrity", async () => {
		const verified = await Run(
			Effect.gen(function* () {
				const verification = yield* ReleaseVerification;
				const release = yield* verification.VerifyManifest(raw_manifest, {
					algorithm: "ed25519",
					key_id,
					signature,
				});
				yield* verification.VerifyArtifact(release.artifacts[0], artifact_bytes);
				return release;
			}),
		);

		expect(verified.product_version).toBe("1.0.0");
	});

	it("rejects tampering, unknown keys, malformed signatures, and identity mismatch", async () => {
		const verification = await Effect.runPromise(
			ReleaseVerification.pipe(Effect.provide(VerificationLive)),
		);
		const Tampered = verification.VerifyManifest(
			new TextEncoder().encode(`${new TextDecoder().decode(raw_manifest)} `),
			{ algorithm: "ed25519", key_id, signature },
		);
		await expect(Run(Tampered)).rejects.toMatchObject({ code: "invalid_signature" });
		await expect(
			Run(
				verification.VerifyManifest(raw_manifest, {
					algorithm: "ed25519",
					key_id: "unknown",
					signature,
				}),
			),
		).rejects.toMatchObject({ code: "untrusted_key" });
		await expect(
			Run(
				verification.VerifyManifest(raw_manifest, {
					algorithm: "rsa",
					key_id,
					signature: "***",
				}),
			),
		).rejects.toMatchObject({ code: "invalid_signature" });

		const mismatched = {
			...manifest,
			signing_identity: { algorithm: "ed25519", key_id: "old" },
		};
		const mismatched_bytes = new TextEncoder().encode(JSON.stringify(mismatched));
		await expect(
			Run(
				verification.VerifyManifest(mismatched_bytes, {
					algorithm: "ed25519",
					key_id,
					signature: sign(null, mismatched_bytes, keys.privateKey).toString("base64"),
				}),
			),
		).rejects.toMatchObject({ code: "signing_identity_mismatch" });
	});

	it("rejects artifact size and digest mismatches", async () => {
		const verification = await Effect.runPromise(
			ReleaseVerification.pipe(Effect.provide(VerificationLive)),
		);
		const artifact = manifest.artifacts[0];

		await expect(
			Run(
				verification.VerifyArtifact({ ...artifact, byte_size: 1 } as never, artifact_bytes),
			),
		).rejects.toMatchObject({ code: "artifact_size" });
		await expect(
			Run(
				verification.VerifyArtifact(
					{ ...artifact, sha256: "0".repeat(64) } as never,
					artifact_bytes,
				),
			),
		).rejects.toMatchObject({ code: "artifact_hash" });
	});
});
