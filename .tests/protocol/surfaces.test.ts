import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import { SurfaceItem } from "../../modules/protocol/src/surfaces";

const occurred_at = "2026-07-18T10:00:00.000Z";

const surface_item = {
	attribution: { agent_id: "agent_1", run_id: "run_1", thread_id: "thread_1" },
	category: "work",
	correlation_id: "command_1",
	kind: "run",
	occurred_at,
	raw_observation: { engine_id: "codex", observation_id: "observation_1" },
	raw_origin: { provider: "codex", reference: "native_1" },
	summary: { label: "Agent run", status: "running" },
	surface_id: "surface_1",
};

describe("surface taxonomy protocol", () => {
	it("decodes each canonical category with bounded public attribution and provenance", async () => {
		const categories = [
			["work", "run"],
			["time", "timer"],
			["guidance", "global_guidance"],
			["routine", "skill"],
			["capability", "mcp_server"],
			["process", "terminal"],
			["change", "workspace_conflict"],
			["permission", "approval"],
			["native_action", "opaque_engine_work"],
		] as const;

		for (const [category, kind] of categories) {
			await expect(
				Effect.runPromise(
					Schema.decodeUnknownEffect(SurfaceItem)({
						...surface_item,
						category,
						kind,
					}),
				),
			).resolves.toMatchObject({ category, kind });
		}
	});

	it("rejects provider vocabulary, mismatched category kinds, raw frames, and unsafe summaries", async () => {
		for (const invalid of [
			{ ...surface_item, category: "engine", kind: "codex_timer" },
			{ ...surface_item, category: "work", kind: "timer" },
			{ ...surface_item, raw_frame: { secret: "never public" } },
			{ ...surface_item, summary: { label: "Contains\u0000control" } },
		]) {
			await expect(
				Effect.runPromise(
					Schema.decodeUnknownEffect(SurfaceItem, { onExcessProperty: "error" })(invalid),
				),
			).rejects.toBeDefined();
		}
	});
});
