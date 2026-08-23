import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

const source = readFileSync(new URL("../../.scripts/build/runner.ts", import.meta.url), "utf8");
const retirement_source = readFileSync(
	new URL("../../modules/installer/rust/processes.rs", import.meta.url),
	"utf8",
);

describe("release build update preflight", () => {
	it("makes close Artisan the first phase before release work begins", () => {
		const close_artisan = source.indexOf('name: "close Artisan"');
		const lifecycle_binaries = source.indexOf('name: "lifecycle binaries"');
		const retire_instances = source.indexOf('name: "retire instances"');
		const prepare_release = source.indexOf('name: "prepare"');
		const module_build = source.indexOf('name: "modules"');
		const native_build = source.indexOf('name: "native build"');

		expect(close_artisan).toBeGreaterThan(-1);
		expect(lifecycle_binaries).toBeGreaterThan(close_artisan);
		expect(retire_instances).toBeGreaterThan(lifecycle_binaries);
		expect(prepare_release).toBeGreaterThan(close_artisan);
		expect(module_build).toBeGreaterThan(close_artisan);
		expect(native_build).toBeGreaterThan(module_build);
		expect(source).toContain('"prepare-update"');
	});

	it("passes an explicit force override to both retirement phases", () => {
		expect(source).toContain('process.argv.slice(2).includes("--force")');
		expect(source).toContain('(["--yes", "--force"] as const)');
		expect(source).toContain('...(force_retirement ? ["--force"] : [])');
	});

	it("leaves the editor open when Forge refuses the idle-only shutdown", () => {
		const forge_gate = retirement_source.indexOf("if !forges.is_empty()");
		const close_editors = retirement_source.indexOf("for process in &others");

		expect(forge_gate).toBeGreaterThan(-1);
		expect(close_editors).toBeGreaterThan(forge_gate);
	});
});
