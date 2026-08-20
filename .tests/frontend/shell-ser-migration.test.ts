import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const root = resolve(import.meta.dirname, "../..");
const Read = (path: string) => readFileSync(resolve(root, path), "utf8");

const shell_sources = [
	"modules/frontend/src/lib/forge/discovery.ts",
	"modules/frontend/src/routes/components/forge-connection-overlay.svelte",
	"modules/frontend/src/routes/components/dev-instance-badge.svelte",
	"modules/frontend/src/routes/components/sidebar-identity.svelte",
	"modules/frontend/src/routes/components/command-menu.svelte",
].map(Read);

describe("shell SER migration", () => {
	it("keeps shell workflows generator-owned and removes bridge queues", () => {
		for (const source of shell_sources) {
			expect(source).not.toMatch(/Effect\.(flatMap|andThen|sync|succeed|suspend)/);
			expect(source).not.toMatch(/\$effect\([^]*Queue\./);
			expect(source).not.toContain(".then(");
			expect(source).not.toContain("new Promise");
		}
	});

	it("does not revive the retired banner bridge", () => {
		for (const source of shell_sources) {
			expect(source).not.toContain("banner/service");
			expect(source).not.toContain("sonner-presenter");
		}
	});

	it("uses native navigation and top-level SER discovery", () => {
		const overlay = Read(
			"modules/frontend/src/routes/components/forge-connection-overlay.svelte",
		);
		const command_menu = Read("modules/frontend/src/routes/components/command-menu.svelte");
		const sidebar = Read("modules/frontend/src/routes/components/sidebar-identity.svelte");
		expect(overlay).toContain('if (is_visible && presentation.tone === "error")');
		expect(command_menu).not.toContain("goto(");
		expect(sidebar).not.toContain("goto(");
	});
});
