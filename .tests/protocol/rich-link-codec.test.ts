import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeInboundControlEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

const timestamp = "2026-07-15T12:00:00.000Z";
const asset_id = "a".repeat(64);

const query = {
	kind: "rich-link.metadata.query",
	message_id: "query_1",
	origin: "frontend",
	payload: { url: "https://example.com/articles/one" },
	protocol_version: 1,
	schema_version: 1,
	sent_at: timestamp,
} as const;

const result = {
	correlation_id: "query_1",
	kind: "rich-link.metadata.query.result",
	message_id: "result_1",
	origin: "backend",
	payload: {
		cache: { expires_at_ms: 2_000, status: "miss" },
		favicon: {
			asset_id,
			bytes: 42,
			content_type: "image/png",
			source: "document_icon",
			source_url: "https://example.com/favicon.png",
		},
		fetched_at_ms: 1_000,
		final_url: "https://www.example.com/articles/one",
		page_name: "An example article",
		requested_url: "https://example.com/articles/one",
		site_name: "Example",
		title: "Example article title",
	},
	protocol_version: 1,
	schema_version: 1,
	sent_at: timestamp,
} as const;

describe("rich-link protocol codec", () => {
	it("roundtrips valid metadata queries and complete results", async () => {
		await expect(Effect.runPromise(DecodeInboundControlEnvelope(query))).resolves.toEqual(
			query,
		);
		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(result))).resolves.toEqual(
			result,
		);
	});

	it("roundtrips each optional title and favicon form", async () => {
		for (const payload of [
			{ ...result.payload, favicon: undefined },
			{ ...result.payload, title: undefined },
			{ ...result.payload, page_name: "\u{1f600}".repeat(512) },
			{
				cache: { expires_at_ms: 2_000, status: "hit" },
				fetched_at_ms: 1_000,
				final_url: "https://example.com/articles/one",
				page_name: "Example",
				requested_url: "https://example.com/articles/one",
				site_name: "Example",
			},
		]) {
			const source_safe_result = { ...result, payload };

			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope(source_safe_result)),
			).resolves.toEqual(source_safe_result);
		}
	});

	it("rejects excess fields and unsafe rich-link URLs", async () => {
		for (const payload of [
			{ url: "ftp://example.com/" },
			{ url: "https://user:secret@example.com/" },
			{ url: "http://localhost/" },
			{ url: "http://0.0.0.0/" },
			{ url: "http://127.0.0.1/" },
			{ url: "http://127.1/" },
			{ url: "http://2130706433/" },
			{ url: "http://0x7f000001/" },
			{ url: "http://[::]/" },
			{ url: "http://[::1]/" },
			{ url: "http://10.0.0.1/" },
			{ url: "http://169.254.169.254/latest/meta-data/" },
			{ url: "http://[::ffff:127.0.0.1]/" },
			{ url: "http://[::ffff:7f00:1]/" },
			{ url: "http://[fc00::1]/" },
			{ url: "http://[fe80::1]/" },
			{ url: "https://example.com/\nprivate" },
			{ url: `https://example.com/${"a".repeat(2049)}` },
			{ url: "https://example.com/", extra: true },
		]) {
			await expect(
				Effect.runPromise(DecodeInboundControlEnvelope({ ...query, payload })),
			).rejects.toBeDefined();
		}

		await expect(
			Effect.runPromise(
				DecodeOutboundControlEnvelope({
					...result,
					payload: { ...result.payload, response_body: "private page content" },
				}),
			),
		).rejects.toBeDefined();

		for (const payload of [
			{ ...result.payload, final_url: "https://user:secret@example.com/" },
			{ ...result.payload, requested_url: "https://example.com/\nprivate" },
			{
				...result.payload,
				favicon: { ...result.payload.favicon, source_url: "ftp://example.com/favicon.ico" },
			},
		]) {
			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope({ ...result, payload })),
			).rejects.toBeDefined();
		}
	});

	it("rejects malformed asset metadata, timestamps, and user-facing names", async () => {
		for (const payload of [
			{
				...result.payload,
				favicon: { ...result.payload.favicon, asset_id: "A".repeat(64) },
			},
			{
				...result.payload,
				favicon: { ...result.payload.favicon, bytes: 128 * 1024 + 1 },
			},
			{
				...result.payload,
				favicon: { ...result.payload.favicon, bytes: 0 },
			},
			{
				...result.payload,
				favicon: { ...result.payload.favicon, content_type: "image/png\nprivate" },
			},
			{
				...result.payload,
				favicon: { ...result.payload.favicon, content_type: "a".repeat(257) },
			},
			{
				...result.payload,
				favicon: { ...result.payload.favicon, content_type: "text/html" },
			},
			{ ...result.payload, fetched_at_ms: 4_102_444_800_001 },
			{
				...result.payload,
				cache: { ...result.payload.cache, expires_at_ms: 4_102_444_800_001 },
			},
			{ ...result.payload, cache: { ...result.payload.cache, expires_at_ms: 999 } },
			{ ...result.payload, page_name: "unsafe\u0000name" },
			{ ...result.payload, site_name: "a".repeat(513) },
			{ ...result.payload, title: "a".repeat(513) },
			{ ...result.payload, page_name: "\u{1f600}".repeat(513) },
		]) {
			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope({ ...result, payload })),
			).rejects.toBeDefined();
		}
	});
});
