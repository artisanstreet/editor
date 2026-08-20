import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(path, "utf8");

describe("engine installation settings loading", () => {
	it("paints retained state before scoped installation and usage reads", () => {
		const source = Read("modules/frontend/src/routes/components/settings/engine.svelte");

		expect(source).toContain("yield* defaults_controller.Current");
		expect(source).not.toContain("defaults_controller.Refresh");
		expect(source).toContain("yield* installations_controller.Current");
		expect(source).toContain("LoadInstallations(engine_id).pipe(Effect.forkScoped)");
		expect(source).toContain(
			"LoadInitialUsage(engine_id, engine_enabled).pipe(Effect.forkScoped)",
		);
		expect(source).toContain("const AuthenticateInstallation = (current_engine_id: string) =>");
		expect(source).toContain("Effect.tap(() =>");
		expect(source).toContain("yield* RefreshUsage.pipe(Effect.forkScoped);");
		expect(source).not.toContain("Effect.tap(() => RefreshUsage.pipe(Effect.forkScoped))");
		expect(source).not.toContain("Effect.ensuring(RefreshUsage)");
		expect(source).toContain("if (engine_id !== page_engine_id) return;");
	});
});
