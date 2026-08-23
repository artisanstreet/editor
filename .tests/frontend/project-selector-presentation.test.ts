import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

const selector = readFileSync(
	resolve(
		import.meta.dirname,
		"../../modules/frontend/src/routes/components/project-selector.svelte",
	),
	"utf8",
);
const mark = readFileSync(
	resolve(
		import.meta.dirname,
		"../../modules/frontend/src/routes/components/project-identity-mark.svelte",
	),
	"utf8",
);

describe("project selector presentation", () => {
	it("shows compact paths with the readable muted foreground token", () => {
		expect(selector).toContain("ShortProjectPath(");
		expect(selector).toContain("text-muted-foreground");
		expect(selector).toContain("title={FormatPathSeparators(");
		expect(selector).not.toContain("text-surface-600");
	});

	it("uses fetched identity images, provider marks, and a folder fallback", () => {
		expect(selector).toContain("<ProjectIdentityMark");
		expect(selector).not.toContain("GradientAvatarColorFor");
		expect(selector).not.toContain("ProjectMonogram");
		expect(mark).toContain("assets.Load(next.image)");
		expect(mark).toContain("RepositoryMarkFor(identity.host)");
		expect(mark).toContain('<Folder class="size-3.5" />');
	});

	it("uses restrained plastic chips for project identities only", () => {
		expect(mark).toContain(
			'class="card-plastic grid size-6 shrink-0 place-items-center overflow-hidden rounded-sm text-muted-foreground"',
		);
		expect(mark).toContain(
			"class:bg-card={Option.isNone(image_source) && repository_mark === undefined}",
		);
		expect(selector).toContain(
			'class="grid size-6 shrink-0 place-items-center rounded-sm text-muted-foreground"',
		);
	});

	it("uses the shared dropdown hover interaction for every action", () => {
		expect(selector).toContain("<DropdownMenuItem");
		expect(selector.match(/\{@attach FollowHighlight\(move_hover\)\}/gu)).toHaveLength(2);
		expect(selector).toContain("data-highlighted:bg-transparent!");
		expect(selector.match(/onpointerenter=\{move_hover\}/gu)).toHaveLength(2);
		expect(selector.match(/onpointermove=\{move_hover\}/gu)).toHaveLength(2);
		expect(selector.match(/onfocusin=\{move_hover\}/gu)).toHaveLength(2);
		expect(selector).toContain("onpointerenter={hover.move}");
		expect(selector).toContain("onpointermove={hover.move}");
		expect(selector).toContain("onfocusin={hover.move}");
		expect(selector).not.toContain("hover:bg-surface-875/60");
	});

	it("initializes Bits highlighting and the hover pill on the current project", () => {
		expect(selector).toContain("let selected_project_item: HTMLElement | undefined;");
		expect(selector).toContain("{@attach CaptureSelectedProject(chosen)}");
		expect(selector).toContain("onOpenAutoFocus={FocusSelectedProject}");
		expect(selector).toContain("event.preventDefault();");
		expect(selector).toContain("selected_project_item.focus({ preventScroll: true });");
	});

	it("uses Tabler selector chrome while retaining real repository brand marks", () => {
		expect(selector).toContain('import Selector from "@tabler/icons-svelte/icons/selector"');
		expect(selector).not.toContain("ChevronDown");
		expect(mark).toContain('import Folder from "@tabler/icons-svelte/icons/folder"');
		expect(mark).toContain('from "$lib/vcs/presentation"');
	});
});
