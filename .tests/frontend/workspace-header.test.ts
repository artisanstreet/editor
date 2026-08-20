import { readFileSync } from "node:fs";
import { resolve } from "node:path";

import { describe, expect, it } from "vitest";

import {
	RepositoryDestinationLabel,
	RepositoryQualifiedLabel,
} from "../../modules/frontend/src/lib/vcs/labels";

const ReadSource = (path: string) =>
	readFileSync(resolve(import.meta.dirname, "../..", path), "utf8");

describe("workspace header", () => {
	it("names a repository as owner/repo, keeping only the innermost group", () => {
		expect(RepositoryQualifiedLabel("https://github.com/sandersonstabo/artisan-editor")).toBe(
			"sandersonstabo/artisan-editor",
		);
		expect(RepositoryQualifiedLabel("https://gitlab.com/group/subgroup/project")).toBe(
			"subgroup/project",
		);
	});

	it("degrades to the bare name, host, or raw value when the URL carries less", () => {
		expect(RepositoryQualifiedLabel("https://example.com/only-name")).toBe("only-name");
		expect(RepositoryQualifiedLabel("https://example.com/")).toBe("example.com");
		expect(RepositoryQualifiedLabel("not a url")).toBe("not a url");
	});

	it("names an external repository destination with its host", () => {
		expect(RepositoryDestinationLabel("https://github.com/artisanstreet/editor")).toBe(
			"github.com/artisanstreet/editor",
		);
		expect(RepositoryDestinationLabel("not a url")).toBe("not a url");
	});

	/**
	 * The identity renders exactly once per surface: inside the window frame on
	 * the bundled desktop shell, and as the primary card's top band on the web.
	 */
	it("mounts in the desktop window frame and in the web content card, never both", () => {
		const layout = ReadSource("modules/frontend/src/routes/+layout.svelte");
		const panel = ReadSource("modules/frontend/src/routes/components/sectioned-panel.svelte");
		const header = ReadSource("modules/frontend/src/routes/components/workspace-header.svelte");

		expect(layout).toContain("-webkit-app-region: drag;");
		expect(layout).toContain("{@render workspace_header()}");
		expect(layout).toContain("header={desktop_runtime || header_project === undefined");
		/**
		 * Both thread surfaces title themselves with the workspace they are inside;
		 * the project picker has no workspace yet and so renders no identity.
		 */
		expect(layout).toContain("const header_project = $derived(active_project);");
		expect(panel).toContain("{@render header()}");
		/** A repository names its remote, branch, and checkout; a plain folder keeps the folder mark. */
		expect(header).toContain("RepositoryQualifiedLabel(remote.web_url)");
		/** Header identity paints from Current; Git inspection belongs to the component scope. */
		expect(header).toContain(
			"repository_controller.Refresh(project?.project_id).pipe(Effect.forkScoped)",
		);
		expect(header).not.toContain("yield* repository_controller.Refresh(project?.project_id);");
		expect(header).not.toMatch(/GetProjectRepositories\(\s*\)/u);
		expect(header).toContain('repository.branch.type === "detached"');
		expect(header).toContain("{project.display_name}");
		expect(header).toContain('<span class="shrink-0">in</span>');
		/** The open thread closes the line after a slash, as the one foreground segment. */
		expect(header).toContain('<span class="shrink-0">/</span>');
		expect(header).toContain(
			'<span class="min-w-0 flex-1 truncate text-foreground">{thread_title}</span>',
		);
		/** Workspace context never competes with the thread title for truncation. */
		expect(header).not.toContain('class="truncate text-(--banner-info)');
		expect(header).not.toContain('class="truncate">{project.display_name}</span>');
		expect(layout).toContain("thread_display_title(active_thread, $thread_title_mode)");
		/** The remote link must escape the titlebar drag region to stay clickable. */
		expect(header).toContain("[-webkit-app-region:no-drag]");
	});
});
