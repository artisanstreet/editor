import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import { ReadStylesheets } from "./stylesheet-source";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("scrollbar visibility policy", () => {
	it("hides native and component scrollbars without disabling scrolling", () => {
		const styles = ReadStylesheets();

		expect(styles).toMatch(/\*\s*\{[^}]*scrollbar-width:\s*none;[^}]*\}/u);
		expect(styles).toMatch(/\*\s*\{[^}]*-ms-overflow-style:\s*none;[^}]*\}/u);
		expect(styles).toMatch(/::-webkit-scrollbar\s*\{[^}]*display:\s*none;/u);
		expect(styles).toMatch(
			/\[data-slot="scroll-area-scrollbar"\]\s*\{[^}]*display:\s*none;[^}]*\}/u,
		);
		expect(styles).not.toContain("::-webkit-scrollbar-thumb");
		expect(styles).not.toContain("scrollbar-gutter");
	});

	it("does not reserve fake gutters beside hidden scrollbars", () => {
		const editor_route = Read("modules/frontend/src/routes/components/editor-route.svelte");
		const model_list = Read(
			"modules/frontend/src/routes/components/model-selector/model-list.svelte",
		);
		const compaction_model = Read(
			"modules/frontend/src/routes/components/settings/compaction-model.svelte",
		);

		expect(editor_route).not.toContain('<div class="mr-3">');
		expect(model_list).not.toContain(
			'class="pr-2 [--docs-sidebar-hover-radius:calc(var(--radius-3xl)-0.5rem)]"',
		);
		expect(model_list).not.toContain(
			'class="mr-2 flex min-w-0 items-center gap-1 transition-transform',
		);
		expect(model_list).not.toContain('class="group/star mr-1 grid');
		expect(compaction_model).not.toContain('class="mr-2 flex min-w-0 items-center gap-1"');
	});
});
