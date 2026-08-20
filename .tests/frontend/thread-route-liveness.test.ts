import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const route = readFileSync(
	resolve("modules/frontend/src/routes/components/thread-route.svelte"),
	"utf8",
);

describe("thread route conversation liveness", () => {
	it("probes a silent conversation stream while durable work reports an active run", () => {
		const watchdog = route.slice(
			route.indexOf("const WatchConversationLiveness"),
			route.indexOf("const RespondQuestion"),
		);

		expect(route).toContain("const conversation_liveness_interval_ms = 30_000;");
		expect(watchdog).toContain("if (work === undefined)");
		expect(watchdog).toContain("if (silent_ms < conversation_liveness_interval_ms) continue;");
		expect(watchdog).toContain(
			"yield* ReconcileConversationAndInteraction.pipe(Effect.ignore);",
		);
		expect(watchdog).toContain(
			"yield* Effect.forkIn(WatchConversationLiveness, thread_scope);",
		);
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
