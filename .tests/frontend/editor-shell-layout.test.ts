import { existsSync, readdirSync, readFileSync } from "node:fs";
import { extname, join, resolve } from "node:path";

import { describe, expect, it } from "vitest";

const Read = (path: string) => readFileSync(resolve(path), "utf8");

describe("editor shell", () => {
	it("keeps the file tree out of the command menu and in the inspector column", () => {
		const menu = Read("modules/frontend/src/routes/components/command-menu.svelte");
		const panel = Read("modules/frontend/src/routes/components/sectioned-panel.svelte");
		const files = Read("modules/frontend/src/routes/components/editor-file-panel.svelte");

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
		expect(files).toContain("const discovered = yield* client\n\t\t\t\t.ListWorkspaceFiles({");
	});

	/**
	 * One inspector column serves whatever workspace is open: the file tree for
	 * the editor and the persistent environment inspector for a concrete thread.
	 */
	it("gives the inspector column to the editor's files and to thread routes", () => {
		const layout = Read("modules/frontend/src/routes/+layout.svelte");

		/**
		 * The editor's tree and the thread inspector both keep the column at every width.
		 */
		expect(layout).toContain('secondary={surface === "editor"');
		expect(layout).toContain("? editor_files");
		expect(layout).toContain('secondary={surface === "editor" ? editor_files : secondary}');
		expect(layout).toContain('const thread_inspector_open = $derived(surface === "threads")');
		expect(layout).toContain("{#snippet editor_files()}");
		expect(layout).toContain("<EditorFilePanel />");
		expect(layout).toContain("/^\\/t\\/[^/]+\\/[^/]+\\/?$/");
		expect(layout).toContain("/^\\/t\\/[^/]+\\/?$/");
	});

	it("drives the editor session from the URL so a deep link restores it", () => {
		const route = Read("modules/frontend/src/routes/e/[workspace]/[thread]/+page.svelte");
		const gate = Read(
			"modules/frontend/src/routes/e/[workspace]/[thread]/editor-route-gate.svelte",
		);

		expect(route).toContain("<EditorRouteGate {workspace_id} {thread_id} />");
		expect(route).toContain("page.params.thread");
		expect(gate).toContain('page.url.searchParams.get("file")');
		expect(gate).toContain("EditorRouteTargetForThread(");
		expect(gate).toContain("yield* WorkspaceCatalogController");
		expect(gate).toContain("workspace_catalog.Changes");
		expect(gate).not.toContain("client.ListThreads");
		expect(gate).not.toContain("client.SubscribeThreadList");
		expect(gate).toContain("ThreadRouteHasWorkspace(thread, workspace_id)");
		expect(gate).toContain("active_thread = undefined");
		expect(gate).toContain(
			"<EditorRoute\n\t\tthread_id={active_thread?.thread_id}\n\t\tworkspace_id={active_thread?.primary_project?.project_id}\n\t/>",
		);
		expect(gate).toContain('let route_id = $state.raw("");');
		expect(gate).toContain("route_id = untrack(() => route_thread_id);");
		expect(gate).not.toContain("const threads = $derived(");
		/** Saving and the strip that carried it are gone for now, not merely hidden. */
		expect(route).not.toContain("ReplaceWorkspaceFile({");
		expect(route).not.toContain("editor.Save");
		expect(route).not.toContain("Save file");
	});

	it("exposes no unscoped or thread-only compatibility routes", () => {
		const layout = Read("modules/frontend/src/routes/+layout.svelte");
		const identity = Read("modules/frontend/src/lib/editor/workspace-identity.ts");

		expect(existsSync(resolve("modules/frontend/src/routes/editor/+page.svelte"))).toBe(false);
		expect(existsSync(resolve("modules/frontend/src/routes/threads/[id]/+page.svelte"))).toBe(
			false,
		);
		expect(layout).not.toContain("page.params.id");
		expect(layout).not.toContain('startsWith("/editor")');
		expect(layout).not.toContain('searchParams.get("workspace")');
		expect(identity).not.toContain("LegacyEditorRoutePath");
		expect(identity).not.toContain('"/editor?workspace="');
	});

	/**
	 * A file that cannot be read must replace the surface, not sit behind a
	 * corner message while the previously opened document stays on screen.
	 */
	it("replaces the surface with a stated reason when a file cannot open", () => {
		const route = Read("modules/frontend/src/routes/components/editor-route.svelte");

		expect(route).toContain("open_failures");
		expect(route).toContain("This file can&rsquo;t be displayed");
		expect(route).toContain("{active_failure}");
		expect(route).toMatch(/\{:else if active_failure !== undefined\}[\s\S]*<EditorSurface/);
	});

	it("paints a file skeleton while a latest-only read runs outside route mounting", () => {
		const route = Read("modules/frontend/src/routes/components/editor-route.svelte");

		expect(route).toContain("yield* MakeLatestRequestGate");
		expect(route).toContain("OpenPath(path, target_workspace_id, generation)");
		expect(route).toContain("Effect.ensuring(");
		expect(route).toContain("duration: editor_open_deadline");
		expect(route).toContain("The editor did not finish opening this file before its deadline.");
		expect(route).toContain("Effect.forkScoped,");
		expect(route).toContain("{:else if opening_path === active_path}");
		expect(route).toContain("const retained = retained_files.get(path);");
		expect(route).not.toContain("const retained_file = $derived(");
		expect(route).not.toContain("yield* OpenPath(active_path)");
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
