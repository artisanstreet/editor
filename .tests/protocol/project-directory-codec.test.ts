import { Effect } from "effect";
import { describe, expect, it } from "vitest";

import { DecodeInboundControlEnvelope, DecodeOutboundControlEnvelope } from "@artisan/protocol";

const timestamp = "2026-08-13T08:00:00.000Z";

const frontend = {
	kind: "project.directory.pick",
	message_id: "pick_project_directory",
	origin: "frontend" as const,
	payload: {},
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: timestamp,
};

const result = (payload: unknown) => ({
	correlation_id: frontend.message_id,
	kind: "project.directory.pick.result",
	message_id: "pick_project_directory_result",
	origin: "backend" as const,
	payload,
	protocol_version: 1 as const,
	schema_version: 1 as const,
	sent_at: timestamp,
});

describe("native project directory picker protocol", () => {
	it("accepts a path-free request and an explicit cancellation", async () => {
		await expect(Effect.runPromise(DecodeInboundControlEnvelope(frontend))).resolves.toEqual(
			frontend,
		);
		await expect(
			Effect.runPromise(DecodeOutboundControlEnvelope(result({ status: "cancelled" }))),
		).resolves.toEqual(result({ status: "cancelled" }));
	});

	it("returns only an opaque Forge directory identity when selected", async () => {
		const selected = result({
			directory: {
				directory_id: "directory_opaque",
				display_name: "artisan-editor",
				has_children: true,
				kind: "root",
			},
			status: "selected",
		});
		const decoded = await Effect.runPromise(DecodeOutboundControlEnvelope(selected));

		expect(decoded).toEqual(selected);
		expect(JSON.stringify(decoded)).not.toMatch(/[A-Z]:[\\/]|root_path/u);
	});
});
