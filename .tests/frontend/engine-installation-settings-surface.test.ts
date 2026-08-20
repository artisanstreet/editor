import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("engine installation settings surface", () => {
	it("uses the controller and direct SER actions for managed installation and sign-in", () => {
		const page = Read("modules/frontend/src/routes/components/settings/engine.svelte");

		expect(page).toContain('id="installation"');
		expect(page).toContain("yield* EngineInstallationsController");
		expect(page).toContain("installations_controller.Changes");
		expect(page).toContain("Stream.runForEach(ApplyInstallations)");
		expect(page).toContain("Effect.forkScoped");
		expect(page).toContain("onclick={yield* InstallationAction()}");
		expect(page).toContain("onclick={yield* RollbackInstallation}");
		expect(page).toContain("onclick={yield* AuthenticateInstallation(engine_id)}");
		expect(page).toContain("installations_state.load_error");
		expect(page).toContain("InstallationAction(current_installation.active_version)");
		expect(page).toContain('current_usage.authentication === "unauthenticated"');
		expect(page).toContain("current_installation?.managed === true");
		expect(page).toContain('role="status" aria-live="polite" aria-atomic="true"');
		expect(page).toContain('current_installation.activity === "failed"');
		expect(page).toContain('"The managed installation did not complete."');
		expect(page).toContain('"Managed installation ready."');
		expect(page).toContain(
			'current_installation.active_version === undefined ? "Retry install" : "Repair"',
		);
		expect(page).not.toContain('credentials_present ? "Signed in"');
	});

	it("registers the controller, fixture API, and navigation anchor", () => {
		const runtime = Read("modules/frontend/src/lib/runtime/browser-frontend-runtime.ts");
		const fixture = Read(
			"modules/frontend/src/lib/runtime/fixtures/project-identity-queries.ts",
		);
		const nav = Read("modules/frontend/src/routes/components/settings/nav.svelte");

		expect(runtime).toContain("EngineInstallationsControllerLive");
		expect(runtime).toContain("Layer.provide(FrontendRuntimeLive)");
		expect(fixture).toContain("GetEngineInstallations");
		expect(fixture).toContain("InstallEngine");
		expect(fixture).toContain("RollbackEngine");
		expect(fixture).toContain("AuthenticateEngine");
		expect(fixture).toContain("MakeFixtureProjectIdentityQueries");
		expect(nav).toContain('{ hash: "installation", label: "Installation" }');
		expect(nav).not.toContain('{ hash: "permissions", label: "Permissions" }');
	});
});
