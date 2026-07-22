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
		const popover_content = Read("modules/frontend/src/lib/components/ui/popover/popover-content.sv");

		expect(layout).toContain("/^\\/thread\\/[^/]+\\/?$/");
		expect(layout).toContain("<ThreadPanel />");
		expect(layout).toContain("secondary={is_thread ? secondary : undefined}");
		expect(thread).toContain("Thread · Artisan Editor");
		expect(thread_panel).toContain("<ModelSelector />");
		expect(model_selector).toContain('aria-label="Select model"');
		expect(model_selector).toContain('rounded-2xl bg-linear-to-b from-foreground/7.5 to-foreground/2.5 p-2');
		expect(model_selector).toContain("transition-colors card-lg");
		expect(model_selector).toContain('? "size-6 shrink-0 dark:invert"');
		expect(model_selector).toContain('truncate text-base font-semibold text-foreground">{selected_model.name}');
		expect(model_selector).toContain('truncate text-xs text-muted-foreground">{selected_model.lab}');
		expect(model_selector).toContain('<Selector class="pointer-events-none size-4 shrink-0 text-muted-foreground" />');
		expect(model_selector).toContain('aria-label="Coding engines"');
		expect(model_selector).toContain("SvglOpenAILogo");
		expect(model_selector).toContain("SvglClaudeAILogo");
		expect(model_selector).toContain("SvglGrokLogo");
		expect(model_selector).toContain('icon: OpenCodeIcon');
		expect(model_selector).toContain("SvglGoogleAntigravityLogo");
		expect(model_selector).not.toMatch(/Cline|Terminal2/);
		expect(popover_content).toContain('"card bg-popover text-popover-foreground');
		expect(popover_content).not.toMatch(/ring-foreground|shadow-2xl|ring-1/);
		expect(model_selector).not.toContain("card!");
		expect(model_selector).toContain('rounded-lg! bg-linear-to-b from-foreground/10 to-foreground/5 p-1');
		expect(model_selector).toContain('after:hidden hover:text-foreground data-active:border-transparent data-active:bg-transparent');
		expect(model_selector).toContain('<ScrollArea class="h-64 rounded-xl">');
		expect(model_selector).toContain('truncate text-base font-semibold text-foreground');
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
