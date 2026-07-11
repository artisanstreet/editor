import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	type GlobalGuidanceQueryResultEnvelope,
} from "@artisan/protocol";

const hash_a = "a".repeat(64);
const timestamp = "2026-07-11T08:00:00.000Z";

function frontend_envelope(kind: string, payload: unknown) {
	return {
		kind,
		message_id: `message_${kind}`,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: timestamp,
	};
}

function query_result(
	overrides: Partial<GlobalGuidanceQueryResultEnvelope["payload"]> = {},
): GlobalGuidanceQueryResultEnvelope {
	return {
		correlation_id: "guidance_query",
		kind: "guidance.query.result",
		message_id: "guidance_result",
		origin: "backend",
		payload: {
			candidates: [],
			content: "Canonical guidance",
			metadata: {
				canonical: {
					byte_count: 18,
					content_hash: hash_a,
					status: "ready",
					updated_at: timestamp,
				},
				providers: [],
			},
			...overrides,
		},
		protocol_version: 1,
		schema_version: 1,
		sent_at: timestamp,
	};
}

describe("global guidance protocol codec", () => {
	it("decodes every V1 guidance request envelope", async () => {
		const envelopes = [
			frontend_envelope("guidance.query", {}),
			frontend_envelope("guidance.update", { content: "Use Effect services." }),
			frontend_envelope("guidance.selection", {
				content_hash: hash_a,
				provider: "codex",
			}),
			frontend_envelope("guidance.drift.resolve", {
				action: "overwrite",
				observed_hash: hash_a,
				provider: "claude",
			}),
			frontend_envelope("guidance.sync.retry", { provider: "codex" }),
		];

		const decoded = await Promise.all(
			envelopes.map((envelope) => Effect.runPromise(DecodeInboundControlEnvelope(envelope))),
		);

		expect(decoded).toEqual(envelopes);
	});

	it("decodes the content-bearing query result", async () => {
		const envelope = query_result();

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(envelope))).resolves.toEqual(
			envelope,
		);
	});

	it.each([
		["provider", frontend_envelope("guidance.sync.retry", { provider: "gemini" })],
		[
			"drift action",
			frontend_envelope("guidance.drift.resolve", {
				action: "merge",
				observed_hash: hash_a,
				provider: "codex",
			}),
		],
		[
			"hash casing",
			frontend_envelope("guidance.selection", {
				content_hash: "A".repeat(64),
				provider: "codex",
			}),
		],
		[
			"hash length",
			frontend_envelope("guidance.selection", {
				content_hash: "a".repeat(63),
				provider: "codex",
			}),
		],
	] as const)("rejects an invalid %s", async (_label, envelope) => {
		await expect(
			Effect.runPromise(DecodeInboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it.each([-1, 1_048_577, 1.5])("rejects invalid guidance byte count %s", async (byte_count) => {
		const envelope = query_result({
			metadata: {
				canonical: {
					byte_count,
					content_hash: hash_a,
					status: "ready",
					updated_at: timestamp,
				},
				providers: [],
			},
		});

		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(envelope)),
		).rejects.toBeDefined();
	});

	it("bounds content by encoded UTF-8 bytes rather than JavaScript length", async () => {
		const ascii_boundary = `${"a".repeat(1_048_575)}\n`;
		const multibyte_boundary = `${"\u{1F600}".repeat(262_143)}abc\n`;
		const accepted = [ascii_boundary, multibyte_boundary];
		const rejected = ["a".repeat(1_048_576), "\u{1F600}".repeat(262_144)];

		for (const content of accepted) {
			await expect(
				Effect.runPromise(
					DecodeInboundControlEnvelope(frontend_envelope("guidance.update", { content })),
				),
			).resolves.toBeDefined();
		}

		for (const content of rejected) {
			await expect(
				Effect.runPromise(
					DecodeInboundControlEnvelope(frontend_envelope("guidance.update", { content })),
				),
			).rejects.toBeDefined();
		}
	});

	it("applies the normalized byte ceiling to snapshot content and candidate previews", async () => {
		const normalization_overflow = "a".repeat(1_048_576);
		const snapshot = query_result({ content: normalization_overflow });
		const candidate = query_result({
			candidates: [
				{
					byte_count: 1_048_576,
					content_hash: hash_a,
					modified_at: timestamp,
					path: "C:/guidance/AGENTS.md",
					preview: normalization_overflow,
					provider: "codex",
				},
			],
		});

		for (const envelope of [snapshot, candidate]) {
			await expect(
				Effect.runPromise(DecodeOutboundControlEnvelope(envelope)),
			).rejects.toBeDefined();
		}
	});
});
