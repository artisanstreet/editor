import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("notification loading", () => {
	it("constructs the app service from defaults and hydrates storage and permission in the background", () => {
		const service = Read("modules/frontend/src/lib/notifications/service.ts");
		const settings = Read(
			"modules/frontend/src/routes/components/settings/notifications.svelte",
		);

		expect(service).toContain("enabled: preferences.Default.enabled");
		expect(service).toContain('permission: "default"');
		expect(service).toContain("const EnsureHydrated = yield* Effect.cached(HydrateOnce)");
		expect(service).toContain("Effect.forkIn(EnsureHydrated, service_scope)");
		expect(service).not.toContain("const stored = yield* preferences.Load");
		expect(service).not.toContain("permission: yield* presenter.Permission");

		expect(settings).toContain("yield* notifications.Current");
		expect(settings).toContain("notifications.Refresh.pipe(Effect.forkScoped)");
		expect(settings).not.toContain("yield* notifications.Refresh);");
	});
});
