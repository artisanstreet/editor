import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const root = resolve(import.meta.dirname, "../..");
const Read = (path: string) => readFileSync(resolve(root, path), "utf8");

const shell_sources = [
	"modules/frontend/src/lib/banner/service.ts",
	"modules/frontend/src/lib/banner/sonner-presenter.ts",
	"modules/frontend/src/lib/forge/discovery.ts",
	"modules/frontend/src/routes/components/forge-connection-overlay.sv",
	"modules/frontend/src/routes/components/dev-instance-badge.sv",
	"modules/frontend/src/routes/components/sidebar-identity.sv",
	"modules/frontend/src/routes/components/command-menu.sv",
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

	it("documents the sole foreign callback queue as a serialized presentation boundary", () => {
		const service = Read("modules/frontend/src/lib/banner/service.ts");
		expect(service).toContain("foreign UI ingress");
		expect(service).toContain("serializes only banner actions");
	});

	it("uses native navigation and top-level SER discovery", () => {
		const overlay = Read("modules/frontend/src/routes/components/forge-connection-overlay.sv");
		const command_menu = Read("modules/frontend/src/routes/components/command-menu.sv");
		const sidebar = Read("modules/frontend/src/routes/components/sidebar-identity.sv");
		expect(overlay).toContain('if (is_visible && presentation.tone === "error")');
		expect(command_menu).not.toContain("goto(");
		expect(sidebar).not.toContain("goto(");
	});
});
