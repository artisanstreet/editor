import { NodeCrypto } from "@effect/platform-node-shared";
import { Crypto, Effect } from "effect";
import { describe, expect, it } from "vitest";

import { workspace_text_maximum_bytes } from "@artisan/protocol";

import {
	ComputeContentIdentity,
	DecodeWorkspaceText,
	EncodeWorkspaceText,
	WorkspaceFileContentError,
} from "../../modules/backend/src/workspace/files/content";

const utf8 = new TextEncoder();
const node_crypto = NodeCrypto.layer;

function run<A, E>(effect: Effect.Effect<A, E, never>) {
	return Effect.runPromise(effect);
}

function run_with_crypto<A, E>(effect: Effect.Effect<A, E, Crypto.Crypto>) {
	return Effect.runPromise(Effect.provide(effect, node_crypto));
}

describe("workspace file content", () => {
	it("round-trips ASCII and multibyte text", async () => {
		const text = "artisan: caf\u00e9 \u65e5\u672c\u8a9e";

		const encoded = await run(EncodeWorkspaceText(text));
		const decoded = await run(DecodeWorkspaceText(encoded));

		expect(decoded).toBe(text);
	});

	it("accepts the exact maximum byte bound", async () => {
		const text = "a".repeat(workspace_text_maximum_bytes);

		await expect(run(EncodeWorkspaceText(text))).resolves.toHaveLength(
			workspace_text_maximum_bytes,
		);
		await expect(run(DecodeWorkspaceText(utf8.encode(text)))).resolves.toBe(text);
		await expect(
			run_with_crypto(ComputeContentIdentity(utf8.encode(text))),
		).resolves.toMatchObject({ byte_count: workspace_text_maximum_bytes });
	});

	it("rejects content over the byte bound", async () => {
		const bytes = new Uint8Array(workspace_text_maximum_bytes + 1);
		const text = "a".repeat(workspace_text_maximum_bytes + 1);

		await expect(run(EncodeWorkspaceText(text))).rejects.toBeInstanceOf(
			WorkspaceFileContentError,
		);
		await expect(run(DecodeWorkspaceText(bytes))).rejects.toBeInstanceOf(
			WorkspaceFileContentError,
		);
		await expect(run_with_crypto(ComputeContentIdentity(bytes))).rejects.toBeInstanceOf(
			WorkspaceFileContentError,
		);
	});

	it("rejects malformed UTF-8 without leaking bytes", async () => {
		const bytes = new Uint8Array([0xc3, 0x28]);

		await expect(run(DecodeWorkspaceText(bytes))).rejects.toMatchObject({
			_tag: "WorkspaceFileContentError",
			operation: "decode",
		});
		await expect(run(DecodeWorkspaceText(bytes))).rejects.not.toHaveProperty("cause");
		await expect(run(DecodeWorkspaceText(bytes))).rejects.not.toHaveProperty("bytes");
	});

	it("computes the deterministic SHA-256 identity", async () => {
		const bytes = utf8.encode("hello");

		await expect(run_with_crypto(ComputeContentIdentity(bytes))).resolves.toEqual({
			algorithm: "sha256",
			byte_count: 5,
			content_hash: "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
		});
	});

	it("returns and hashes caller-independent byte copies", async () => {
		const source = utf8.encode("copy-safe");
		const encoded = await run(EncodeWorkspaceText("copy-safe"));
		const second_encoded = await run(EncodeWorkspaceText("copy-safe"));
		const identity = await run_with_crypto(ComputeContentIdentity(source));

		encoded[0] = 0;
		source[0] = 0;

		expect(second_encoded[0]).toBe(99);
		expect(await run(DecodeWorkspaceText(utf8.encode("copy-safe")))).toBe("copy-safe");
		expect(identity.content_hash).toBe(
			"daca4d4edbd2715aa32226a8a6775ab5ddba4f1bba62572d3ab384d46f4dcba7",
		);
	});
});
