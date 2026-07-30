import { existsSync, readdirSync, readFileSync } from "node:fs";
import { extname, join, resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("editor shell", () => {
	it("keeps the file tree out of the command menu and in the inspector column", () => {
		const menu = Read("modules/frontend/src/routes/components/command-menu.sv");
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.sv");
		const files = Read("modules/frontend/src/routes/components/editor-file-panel.sv");

		/**
		 * The surface follows the route rather than a tab strip, and the tree is
		 * part of the workspace rather than of navigation: the command menu keeps
		 * neither. It carries threads only.
		 */
		expect(menu).not.toContain("TabsTrigger");
		expect(menu).not.toContain("WorkspaceFileTree");
		expect(menu).not.toContain("client.ListWorkspaceFiles");
		expect(panel).toContain('id: "threads"');
		expect(panel).toContain('id: "editor"');
		expect(files).toContain("<WorkspaceFileTree");
		expect(files).toContain("client.ListWorkspaceFiles");
	});

	/**
	 * One inspector column serves whatever workspace is open: the file tree for
	 * the editor, the thread inspector for a concrete thread, and nothing for the
	 * routes that own neither.
	 */
	it("gives the inspector column to the editor's files and to thread routes", () => {
		const layout = Read("modules/frontend/src/routes/+layout.sv");

		expect(layout).toContain(
			'secondary={surface === "editor" ? editor_files : is_thread ? secondary : undefined}',
		);
		expect(layout).toContain("{#snippet editor_files()}");
		expect(layout).toContain("<EditorFilePanel />");
		expect(layout).toContain("/^\\/threads(?:\\/[^/]+)?\\/?$/");
	});

	it("drives the editor session from the URL so a deep link restores it", () => {
		const route = Read("modules/frontend/src/routes/editor/+page.sv");

		expect(route).toContain('page.url.searchParams.get("file")');
		expect(route).toContain("client.ReadWorkspaceFile");
		expect(route).toContain("editor.Activate");
		expect(route).toContain("<EditorSurface");
		/** Saving and the strip that carried it are gone for now, not merely hidden. */
		expect(route).not.toContain("ReplaceWorkspaceFile({");
		expect(route).not.toContain("editor.Save");
		expect(route).not.toContain("Save file");
	});

	/**
	 * A file that cannot be read must replace the surface, not sit behind a
	 * corner message while the previously opened document stays on screen.
	 */
	it("replaces the surface with a stated reason when a file cannot open", () => {
		const route = Read("modules/frontend/src/routes/editor/+page.sv");

		expect(route).toContain("open_failures");
		expect(route).toContain("This file can&rsquo;t be displayed");
		expect(route).toContain("{active_failure}");
		expect(route).toMatch(/\{:else if active_failure !== undefined\}[\s\S]*<EditorSurface/);
	});

	it("keeps every editor implementation behind the adapter seam", () => {
		const service = Read("modules/frontend/src/lib/editor/service.ts");
		const adapter = Read("modules/frontend/src/lib/editor/adapter.ts");

		/** The service must never import an editor library directly. */
		expect(service).not.toMatch(/@codemirror|monaco/);
		expect(adapter).not.toMatch(/@codemirror|monaco/);
		expect(service).toContain("MakeEditorLayer");
		expect(service).toContain("EditorAdapter");
	});

	/** Grammars are the reason Lezer is the default; they must load on demand. */
	it("loads every Lezer grammar lazily", () => {
		const language = Read("modules/frontend/src/lib/editor/language.ts");

		expect(language).toContain('import("@codemirror/lang-javascript")');
		expect(language).toContain('import("@codemirror/lang-rust")');
		expect(language).not.toMatch(/^import \{[^}]*\} from "@codemirror\/lang-/m);
	});

	/**
	 * The load-time property the editor exists to protect: opening a thread must
	 * not download the editor. If the layout node ever references the CodeMirror
	 * chunk, the route split has regressed.
	 */
	it.runIf(existsSync(resolve(".dist/frontend/_app/immutable")))(
		"keeps CodeMirror out of the shared layout chunk",
		() => {
			const immutable = resolve(".dist/frontend/_app/immutable");
			const javascript = (directory: string) =>
				existsSync(join(immutable, directory))
					? readdirSync(join(immutable, directory))
							.filter((name) => extname(name) === ".js")
							.map((name) => ({
								name,
								text: readFileSync(join(immutable, directory, name), "utf8"),
							}))
					: [];

			const editor_chunks = javascript("chunks").filter((chunk) =>
				chunk.text.includes("cm-content"),
			);
			expect(editor_chunks.length).toBeGreaterThan(0);

			const layout = javascript("nodes").find((node) => node.name.startsWith("0."));
			expect(layout).toBeDefined();
			for (const chunk of editor_chunks)
				expect(layout?.text.includes(chunk.name.replace(".js", ""))).toBe(false);
		},
	);
});
