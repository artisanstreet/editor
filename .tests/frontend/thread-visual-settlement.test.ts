import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import {
	ConversationVisualSettlementDeadlineMillis,
	ConversationVisualSettlementDecision,
	ConversationVisualSettlementQuietMillis,
} from "../../modules/frontend/src/lib/conversation/visual-settlement";

const repository_root = resolve(import.meta.dirname, "../..");
const Read = (path: string) => readFileSync(resolve(repository_root, path), "utf8");

describe("thread visual settlement", () => {
	it("requires quiet geometry, loaded fonts, and completed content transforms", () => {
		const stable = {
			elapsed_ms: 500,
			fonts_loaded: true,
			maximum_wait_ms: ConversationVisualSettlementDeadlineMillis(false),
			pending_transforms: false,
			quiet_ms: ConversationVisualSettlementQuietMillis,
			stable_sample_count: 3,
		};

		expect(ConversationVisualSettlementDecision(stable)).toBe("stable");
		expect(
			ConversationVisualSettlementDecision({ ...stable, pending_transforms: true }),
		).toBeUndefined();
		expect(
			ConversationVisualSettlementDecision({ ...stable, fonts_loaded: false }),
		).toBeUndefined();
		expect(
			ConversationVisualSettlementDecision({
				...stable,
				quiet_ms: ConversationVisualSettlementQuietMillis - 1,
			}),
		).toBeUndefined();
	});

	it("uses a shorter live-thread deadline but still waits for two matching layouts", () => {
		const active_deadline = ConversationVisualSettlementDeadlineMillis(true);
		const blocked = {
			elapsed_ms: active_deadline,
			fonts_loaded: false,
			maximum_wait_ms: active_deadline,
			pending_transforms: true,
			quiet_ms: 0,
			stable_sample_count: 1,
		};

		expect(active_deadline).toBeLessThan(ConversationVisualSettlementDeadlineMillis(false));
		expect(ConversationVisualSettlementDecision(blocked)).toBeUndefined();
		expect(ConversationVisualSettlementDecision({ ...blocked, stable_sample_count: 2 })).toBe(
			"deadline",
		);
	});

	it("mounts the thread behind an inert cover and exposes measured diagnostics", () => {
		const gate = Read("modules/frontend/src/routes/components/thread-route-gate.svelte");
		const workspace = Read("modules/frontend/src/routes/components/thread-workspace.svelte");
		const markdown = Read("modules/frontend/src/lib/components/markdown/content.svelte");

		expect(gate).toContain("draft_thread.PendingSubmission(route_id)");
		expect(gate).toContain("let visually_settled = $state(draft_handoff)");
		expect(gate).toContain(
			"onvisualsettled={draft_handoff ? undefined : RevealThread}",
		);
		expect(gate).toContain("class:opacity-0={!visually_settled}");
		expect(gate).toContain("inert={!visually_settled}");
		expect(gate).toContain("absolute inset-0 z-20");
		expect(gate).not.toContain(
			'class="absolute inset-0 z-20 flex items-center justify-center bg-background"',
		);
		expect(gate).toContain("data-visual-settlement-duration-ms");
		expect(workspace).toContain("ConversationVisualSettlementDecision({");
		expect(workspace).toContain('performance.measure("artisan.thread.visual-settlement"');
		expect(workspace).toContain(
			'[data-thread-content-transforming="true"], [aria-busy="true"]',
		);
		expect(markdown).toContain("data-thread-content-transforming");
	});
});
