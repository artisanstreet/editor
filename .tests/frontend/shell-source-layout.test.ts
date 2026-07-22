import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "@effect/vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("routed application shell source layout", () => {
	it("keeps one persistent sidebar around every routed page", () => {
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const shell = Read("modules/frontend/src/routes/components/app-shell.sv");

		expect(layout).toContain("<AppShell");
		expect(shell).toContain("<AppSidebar");
		expect(shell).toContain("100dvh");
		expect(shell).toContain('class="app-content"');
	});

	it("uses client-side SPA routing for dynamic Electron protocol routes", () => {
		const layout = Read("modules/frontend/src/routes/+layout.ts");

		expect(layout).toContain("prerender = false");
		expect(layout).toContain("ssr = false");
	});

	it("provides welcome, thread, and settings route boundaries", () => {
		const home = Read("modules/frontend/src/routes/+page.sv");
		const thread = Read("modules/frontend/src/routes/thread/[id]/+page.sv");
		const settings = Read("modules/frontend/src/routes/settings/+page.sv");

		expect(home).toContain("<WelcomePage");
		expect(thread).toContain("<ThreadWorkspace");
		expect(settings).toContain("<SettingsPage");
	});

	it("switches the sidebar to anchored settings navigation", () => {
		const sidebar = Read("modules/frontend/src/routes/components/app-sidebar.sv");

		expect(sidebar).toContain('pathname === "/settings"');
		for (const section of [
			"general",
			"codex",
			"guidance",
			"model-behaviour",
			"retention",
			"appearance",
		]) {
			expect(sidebar).toContain(`["${section}",`);
		}
		expect(sidebar).toContain("/settings#${section_id}");
		expect(sidebar).toContain('href="/settings"');
	});

	it("renders the authorized Barekey logo with Artisan identity", () => {
		const sidebar = Read("modules/frontend/src/routes/components/app-sidebar.sv");

		expect(sidebar).toContain("/barekey-logo.png");
		expect(sidebar).toContain("Artisan Editor");
	});

	it("loads the complete Barekey style and font foundation", () => {
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const global = Read("modules/frontend/src/lib/styles/global.css");
		const fonts = Read("modules/frontend/src/lib/styles/fonts.css");

		for (const stylesheet of ["sidebar.css", "prose.css", "markdown.css"])
			expect(global).toContain(`@import "./${stylesheet}"`);
		for (const utility of ["inset-shadow", "card", "card-color", "card-lg", "card-diff"])
			expect(global).toContain(`@utility ${utility}`);
		expect(global).toContain('@plugin "@tailwindcss/typography"');
		expect(fonts).toContain('font-family: "PP Neue Montreal"');
		expect(fonts).toContain('font-family: "Artisan Neo"');
		expect(layout).toContain("$lib/styles/fonts.css");
		expect(layout).toContain("$lib/styles/artisan-compatibility.css");
	});

	it("keeps previews external-only and supplies reduced-motion behavior", () => {
		const source = Read("modules/frontend/src/lib/styles/artisan-compatibility.css");
		const right_pane = Read("modules/frontend/src/routes/components/right-pane.sv");

		expect(right_pane).toContain("LaunchPreviewInExternalBrowser");
		expect(right_pane).not.toMatch(/<iframe|<webview/i);
		expect(source).toMatch(/prefers-reduced-motion:\s*reduce/);
	});

	it("keeps component behavior in SER without browser-side runners", () => {
		const sources = [
			"app-shell.sv",
			"app-sidebar.sv",
			"welcome-page.sv",
			"settings-page.sv",
		].map((name) => Read(`modules/frontend/src/routes/components/${name}`));

		for (const source of sources) expect(source).not.toMatch(/Effect\.run\w*/);
	});
});
