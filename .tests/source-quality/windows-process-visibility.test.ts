import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const workspace = resolve(import.meta.dirname, "../..");
const require = createRequire(import.meta.url);

const Read = (path: string) => readFileSync(resolve(workspace, path), "utf8");

describe("Windows process visibility", () => {
	it("hides every Effect-owned Node child process at the platform adapter", () => {
		const workspace_configuration = Read("pnpm-workspace.yaml");
		const effect_patch = Read("patches/@effect__platform-node-shared@4.0.0-beta.97.patch");
		const installed_adapter = readFileSync(
			require.resolve("@effect/platform-node-shared/NodeChildProcessSpawner"),
			"utf8",
		);

		expect(workspace_configuration).toContain(
			'"@effect/platform-node-shared@4.0.0-beta.97": patches/@effect__platform-node-shared@4.0.0-beta.97.patch',
		);
		expect(effect_patch.match(/windowsHide: true/gu)).toHaveLength(6);
		expect(installed_adapter).toContain("windowsHide: true");
		expect(installed_adapter.match(/taskkill \/pid/gu)).toHaveLength(2);
		expect(
			installed_adapter.match(/taskkill \/pid[\s\S]{0,150}windowsHide: true/gu),
		).toHaveLength(2);
	});

	it("keeps direct Node background launchers hidden and the foreground CLI explicit", () => {
		for (const path of [
			"modules/backend/src/git/node-process-runner.ts",
			"modules/backend/src/runtime/host-identity.ts",
			"modules/cli/src/node-distribution-runtime.ts",
			"modules/desktop/src/forge-handoff.ts",
			"modules/distribution/src/node-release-adapters.ts",
			"modules/distribution/src/windows-native-adapter.ts",
			"modules/engines/src/process/process.ts",
			"modules/engines/src/process/windows-process-host.ts",
			"modules/installer/src/node-runtime.ts",
		]) {
			expect(Read(path), path).toContain("windowsHide: true");
		}

		const launcher = Read("modules/cli/src/node-launcher.ts");
		expect(launcher).toContain("windowsHide: true");
		expect(launcher).toMatch(/stdio: "inherit",\s+windowsHide: false/u);
	});

	it("keeps runner and checklist helpers inside the existing terminal surface", () => {
		const runner = Read(".scripts/dev/runner.ts");
		const checklist = Read("modules/checklist/src/tui-bridge.ts");

		expect(runner).not.toContain("windowsHide: false");
		expect(checklist).not.toContain("windowsHide: false");
		expect(runner.match(/windowsHide: true/gu)?.length).toBeGreaterThanOrEqual(7);
		expect(checklist).toContain("windowsHide: true");
	});

	it("routes native installer helpers through the no-console command boundary", () => {
		const boundary = Read("modules/installer/rust/background_process.rs");

		expect(boundary).toContain("const CREATE_NO_WINDOW: u32 = 0x0800_0000;");
		expect(boundary).toContain("command.creation_flags(CREATE_NO_WINDOW);");
		expect(boundary).toContain("command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);");

		for (const path of [
			"modules/installer/rust/install.rs",
			"modules/installer/rust/main.rs",
			"modules/installer/rust/processes.rs",
			"modules/installer/rust/shortcuts.rs",
		]) {
			expect(Read(path), path).not.toMatch(
				/(?:std::process::)?Command::new\("(?:cmd\.exe|powershell\.exe|taskkill)"\)/u,
			);
		}
	});
});
