import { Crypto, Data, Effect, Encoding } from "effect";

import { workspace_text_maximum_bytes, type ContentIdentity } from "@artisan/protocol";

const text_encoder = new TextEncoder();
const text_decoder = new TextDecoder("utf-8", { fatal: true });

/** Reports invalid, oversized, or malformed workspace file content without retaining its bytes. */
export class WorkspaceFileContentError extends Data.TaggedError("WorkspaceFileContentError")<{
	readonly operation: "decode" | "encode" | "identity";
}> {}

function validate_byte_count(
	byte_count: number,
	operation: WorkspaceFileContentError["operation"],
) {
	if (byte_count > workspace_text_maximum_bytes) {
		return Effect.fail(new WorkspaceFileContentError({ operation }));
	}

	return Effect.void;
}

/** Encodes bounded workspace text as fresh UTF-8 bytes. */
export const EncodeWorkspaceText = (
	text: string,
): Effect.Effect<Uint8Array, WorkspaceFileContentError> =>
	Effect.gen(function* () {
		const bytes = text_encoder.encode(text);

		yield* validate_byte_count(bytes.byteLength, "encode");

		return new Uint8Array(bytes);
	});

/** Strictly decodes bounded UTF-8 bytes as workspace text. */
export const DecodeWorkspaceText = (
	bytes: Uint8Array,
): Effect.Effect<string, WorkspaceFileContentError> =>
	Effect.gen(function* () {
		yield* validate_byte_count(bytes.byteLength, "decode");

		return yield* Effect.try({
			catch: () => new WorkspaceFileContentError({ operation: "decode" }),
			try: () => text_decoder.decode(new Uint8Array(bytes)),
		});
	});

/** Computes the canonical SHA-256 identity for bounded workspace content. */
export const ComputeContentIdentity = (
	bytes: Uint8Array,
): Effect.Effect<ContentIdentity, WorkspaceFileContentError, Crypto.Crypto> =>
	Effect.gen(function* () {
		yield* validate_byte_count(bytes.byteLength, "identity");

		const crypto = yield* Crypto.Crypto;
		const digest = yield* crypto
			.digest("SHA-256", new Uint8Array(bytes))
			.pipe(Effect.mapError(() => new WorkspaceFileContentError({ operation: "identity" })));

		return {
			algorithm: "sha256",
			byte_count: bytes.byteLength,
			content_hash: Encoding.encodeHex(digest),
		};
	});
