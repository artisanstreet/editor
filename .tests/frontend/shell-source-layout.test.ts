import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("Barekey docs shell reset", () => {
	it("lets the layout compose page surfaces through snippets", () => {
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.sv");

		expect(layout).toContain("<SectionedPanel {sidebar} {primary}");
		expect(layout).toContain("{#snippet sidebar()}");
		expect(layout).toContain("{#snippet primary()}");
		expect(layout).toContain("{@render children()}");
		expect(panel).toContain("primary: Snippet");
		expect(panel).toContain("secondary?: Snippet");
		expect(panel).toContain("sidebar: Snippet");
	});

	it("mounts the third panel only for concrete thread routes", () => {
		const layout = Read("modules/frontend/src/routes/+layout.sv");
		const thread = Read("modules/frontend/src/routes/thread/[id]/+page.sv");
		const thread_panel = Read("modules/frontend/src/routes/components/thread-panel.sv");
		const model_selector = Read("modules/frontend/src/routes/components/model-selector.sv");

		expect(layout).toContain("/^\\/thread\\/[^/]+\\/?$/");
		expect(layout).toContain("<ThreadPanel />");
		expect(layout).toContain("secondary={is_thread ? secondary : undefined}");
		expect(thread).toContain("Thread · Artisan Editor");
		expect(thread_panel).toContain("<ModelSelector />");
		expect(model_selector).toContain('aria-label="Select model"');
		expect(model_selector).toContain('aria-label="Coding engines"');
		expect(model_selector).toContain("SvglCodexLogo");
		expect(model_selector).toContain("SvglClaudeAILogo");
		expect(model_selector).toContain("SvglGrokLogo");
		expect(model_selector).toContain("SvglOpenCodeLogo");
		expect(model_selector).toContain("SvglGoogleAntigravityLogo");
		expect(model_selector).toContain('{ id: "cline", name: "Cline", icon: Terminal2');
		expect(model_selector).toContain('<ScrollArea class="h-64 rounded-xl">');
		expect(model_selector).toContain('aria-label="Available models"');
		expect(model_selector).toContain('name: "GPT 5.6 Sol", lab: "Codex"');
		expect(model_selector).not.toContain('lab: "OpenAI"');
	});

	it("matches the Barekey docs inset sidebar and circular toggle", () => {
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.sv");

		expect(panel).toContain('style="--sidebar-width: 16rem; --sidebar-width-icon: 2.5rem;"');
		expect(panel).toContain('<Sidebar.Root variant="inset" collapsible="icon">');
		expect(panel).toContain("absolute right-0 top-2 hidden size-10");
		expect(panel).toContain("rounded-full bg-foreground/5 card");
		expect(panel).toContain("<LayoutSidebar");
	});

	it("uses the Barekey docs gradient card surface for page content", () => {
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.sv");

		expect(panel).toContain(
			"rounded-3xl bg-linear-to-b from-foreground/5 to-foreground/2.5 p-1 card",
		);
		expect(panel).not.toContain("bg-background");
	});

	it("shows only the copied docs header identity inside the sidebar", () => {
		const sidebar = Read("modules/frontend/src/routes/components/artisan-sidebar.sv");
		const home = Read("modules/frontend/src/routes/+page.sv");

		expect(sidebar).toContain("$lib/assets/barekey/logo-40.png");
		expect(sidebar).toContain('class="size-5 shrink-0 invert dark:invert-0"');
		expect(sidebar).toContain('<span class="font-logo">Artisan Editor</span>');
		expect(sidebar).toContain('<Sidebar.Header class="pl-6 pr-14 lg:pl-2">');
		expect(home).not.toMatch(/WelcomePage|ThreadWorkspace|SettingsPage|LiveWorkspaceStore/);
	});

	it("retains the complete Barekey style foundation", () => {
		const global = Read("modules/frontend/src/lib/styles/global.css");

		for (const stylesheet of ["sidebar.css", "prose.css", "markdown.css"])
			expect(global).toContain(`@import "./${stylesheet}"`);
		for (const utility of ["inset-shadow", "card", "card-color", "card-lg", "card-diff"])
			expect(global).toContain(`@utility ${utility}`);
	});
});
