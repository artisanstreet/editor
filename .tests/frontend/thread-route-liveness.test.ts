import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const route = readFileSync(
	resolve("modules/frontend/src/routes/components/thread-route.svelte"),
	"utf8",
);

describe("thread route conversation liveness", () => {
	it("probes a silent conversation stream only while durable work is genuinely active", () => {
		const watchdog = route.slice(
			route.indexOf("const WatchConversationLiveness"),
			route.indexOf("const RespondQuestion"),
		);

		expect(route).toContain("const conversation_liveness_interval_ms = 30_000;");
		/**
		 * The gate must be run activity, not work-item existence: `GetThreadWork`
		 * returns the pointer run in whatever state it settled, so gating on
		 * existence reconciled every open settled thread forever — polling
		 * wearing a watchdog's clothes.
		 */
		expect(watchdog).toContain("if (!WorkIsActive())");
		expect(watchdog).not.toContain("if (work === undefined)");
		expect(watchdog).toContain("if (silent_ms < conversation_liveness_interval_ms) continue;");
		expect(watchdog).toContain(
			"yield* ReconcileConversationAndInteraction.pipe(Effect.ignore);",
		);
		expect(watchdog).toContain(
			"yield* Effect.forkIn(WatchConversationLiveness, thread_scope);",
		);
	});

	/**
	 * The deterministic half of self-healing: the run-lifecycle push that raises
	 * the finish toast also proves the transcript stale when its session never
	 * settled, and one resync converges. No interval is involved.
	 */
	it("resyncs when a settled run's tail provably never reached the transcript", () => {
		expect(route).toContain("const TranscriptReflectsSettledWork = ");
		expect(route).toContain("const EnsureTranscriptSettled = Effect.gen(function* () {");
		expect(route).toContain("if (TranscriptReflectsSettledWork()) return;");
		expect(route).toContain('yield* Effect.sleep("2 seconds");');
		expect(route).toContain("yield* Effect.forkIn(EnsureTranscriptSettled, thread_scope);");
	});

	it("counts every delivered conversation envelope as liveness, snapshots included", () => {
		const apply_update = route.slice(
			route.indexOf("const ApplyUpdate"),
			route.indexOf("yield* Effect.forkIn(\n\t\tRunConversationSubscription("),
		);

		expect(apply_update).toContain(
			"last_conversation_delivery_ms = yield* Clock.currentTimeMillis;",
		);
	});
});
