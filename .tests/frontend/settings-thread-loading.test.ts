import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

describe("thread settings loading", () => {
	it("paints a retention skeleton before starting the cold policy read", () => {
		const source = readFileSync(
			resolve("modules/frontend/src/routes/components/settings/threads.svelte"),
			"utf8",
		);

		expect(source).toContain("RetryPolicyLoad.pipe(Effect.forkScoped)");
		expect(source).not.toContain("yield* RetryPolicyLoad;\n</script>");
		expect(source).toContain('aria-label="Loading retention policy"');
		expect(source).toContain('policy_state._tag === "Loading"');
	});

	it("starts failed-save reconciliation in route scope instead of joining the foreground action", () => {
		const source = readFileSync(
			resolve("modules/frontend/src/routes/components/settings/threads.svelte"),
			"utf8",
		);
		const controller = readFileSync(
			resolve("modules/frontend/src/lib/settings/thread-retention-policy-controller.ts"),
			"utf8",
		);

		expect(source).toContain(
			"const retention_controller = yield* ThreadRetentionPolicyController",
		);
		expect(source).toContain("yield* retention_controller.Save(next)");
		const save_policy = source.slice(
			source.indexOf("const SavePolicy"),
			source.indexOf("const ToggleEnabled"),
		);
		expect(save_policy).toContain("Effect.ensuring(");
		expect(save_policy).toContain("saving = false;");
		expect(source).not.toContain("GetThreadRetentionPolicy");
		expect(controller).toContain("Effect.forkIn(\n\t\t\t\t\t\t\t\t\tRefresh.pipe");
		expect(controller).toContain("scope,");
	});

	it("uses the app-owned controller to coalesce reads and fence stale reconciliation answers", () => {
		const controller = readFileSync(
			resolve("modules/frontend/src/lib/settings/thread-retention-policy-controller.ts"),
			"utf8",
		);

		expect(controller).toContain(
			"const inflight = yield* Ref.make<Flight | undefined>(undefined)",
		);
		expect(controller).toContain(
			"current !== undefined && current.generation === request_generation",
		);
		expect(controller).toContain("restore(Deferred.await(claim.deferred))");
		expect(controller).toContain("Ref.updateAndGet(\n\t\t\t\t\t\tgeneration,");
		expect(controller).toContain("latest === write_generation");
	});
});
