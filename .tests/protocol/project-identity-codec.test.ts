import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeInboundControlEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

const timestamp = "2026-08-20T08:00:00.000Z";

const frontend = (payload: unknown) => ({
	kind: "project.identity.query",
	message_id: "project_identity_query_1",
	origin: "frontend" as const,
	payload,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: timestamp,
});

const backend = (payload: unknown) => ({
	correlation_id: "project_identity_query_1",
	kind: "project.identity.query.result",
	message_id: "project_identity_result_1",
	origin: "backend" as const,
	payload,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: timestamp,
});

describe("project identity protocol", () => {
	it("keeps the requested project identities bounded and path-free", async () => {
		await expect(
			Effect.runPromise(
				DecodeInboundControlEnvelope(frontend({ project_ids: ["project_1"] })),
			),
		).resolves.toEqual(frontend({ project_ids: ["project_1"] }));

		const result = backend({
			identities: [
				{ kind: "folder", project_id: "project_local" },
				{
					host: "gitlab",
					image: {
						asset_id: "a".repeat(64),
						bytes: 12,
						content_type: "image/png",
						source: "project",
					},
					kind: "repository",
					project_id: "project_repository",
				},
				{ host: "github", kind: "repository", project_id: "project_owner_only" },
			],
		});

		const decoded = await Effect.runPromise(DecodeOutboundControlEnvelope(result));

		expect(decoded).toEqual(result);
		expect(JSON.stringify(decoded)).not.toContain("root_path");
	});

	it("rejects renderer-supplied identity images and oversized project batches", async () => {
		await expect(
			Effect.runPromise(
				DecodeInboundControlEnvelope(
					frontend({
						image: { asset_id: "a".repeat(64), bytes: 1, content_type: "image/png" },
						project_ids: ["project_1"],
					}),
				),
			),
		).rejects.toBeDefined();
		await expect(
			Effect.runPromise(
				DecodeInboundControlEnvelope(
					frontend({
						project_ids: Array.from({ length: 129 }, (_, index) => `project_${index}`),
					}),
				),
			),
		).rejects.toBeDefined();
	});
});
