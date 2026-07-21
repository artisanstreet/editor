import { readFileSync } from "node:fs";
import { basename, join } from "node:path";

import { describe, expect, layer } from "@effect/vitest";
import { Context, Effect, Layer } from "effect";

const component_root = join(process.cwd(), "modules/frontend/src/routes/components");
const source_paths = [
	join(component_root, "editor-shell.sv"),
	join(component_root, "left-pane.sv"),
	join(component_root, "main-pane.sv"),
	join(component_root, "right-pane.sv"),
	join(component_root, "mode-switcher.sv"),
	join(component_root, "quick-open.sv"),
	join(component_root, "file-tab-strip.sv"),
	join(component_root, "chat-transcript.sv"),
	join(component_root, "orchestrator-graph.sv"),
	join(component_root, "marketplace-dialog.sv"),
	join(component_root, "marketplace-list.sv"),
	join(component_root, "workspace-navigation.sv"),
	join(component_root, "activity-status.sv"),
] as const;

interface WorkspaceSource {
	readonly name: string;
	readonly source: string;
}

class WorkspaceSources extends Context.Service<
	WorkspaceSources,
	{ readonly aggregate: string; readonly sources: ReadonlyArray<WorkspaceSource> }
>()("Artisan/Test/WorkspaceSources") {}

const WorkspaceSourcesLive = Layer.effect(
	WorkspaceSources,
	Effect.gen(function* () {
		const sources = yield* Effect.forEach(source_paths, (path) =>
			Effect.try({
				try: () => ({ name: basename(path), source: readFileSync(path, "utf8") }),
				catch: (cause) => cause,
			}),
		);

		return WorkspaceSources.of({
			aggregate: sources.map((source) => source.source).join("\n"),
			sources,
		});
	}),
);

const SourceNamed = (sources: ReadonlyArray<WorkspaceSource>, name: string) =>
	Effect.sync(() => {
		const source = sources.find((candidate) => candidate.name === name);
		if (source === undefined) throw new Error(`Missing workspace source ${name}`);
		return source.source;
	});

describe("production workspace UI source", () => {
	layer(WorkspaceSourcesLive)((it) => {
		it.effect("retains the three-pane shell while composing live projections", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const shell = yield* SourceNamed(sources, "editor-shell.sv");

				expect(shell).toContain("LiveWorkspaceStore");
				expect(shell).toContain("<LeftPane");
				expect(shell).toContain("<MainPane");
				expect(shell).toContain("<RightPane");
				expect(shell).toContain("<Sheet");
			}),
		);

		it.effect("drives the native activity signal from canonical projections", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const shell = yield* SourceNamed(sources, "editor-shell.sv");

				expect(shell).toContain("HasActiveWorkspaceWork");
				expect(shell).toContain("desktop_bridge.setWorking(working)");
				expect(shell).toContain("Effect.addFinalizer(SetDesktopWorking(false))");
			}),
		);

		it.effect("uses registry primitives for interactive workspace controls", () =>
			Effect.gen(function* () {
				const { aggregate, sources } = yield* WorkspaceSources;
				const marketplace = yield* SourceNamed(sources, "marketplace-list.sv");
				const navigation = yield* SourceNamed(sources, "workspace-navigation.sv");

				expect(aggregate).not.toMatch(/<(?:button|input|textarea|select)\b/);
				expect(marketplace).toContain("$lib/components/ui/button");
				expect(navigation).toContain("$lib/components/ui/dropdown-menu");
			}),
		);

		it.effect("connects the editor to authoritative file reads, writes, and Monaco", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const main = yield* SourceNamed(sources, "main-pane.sv");

				expect(main).toContain("<QuickOpen");
				expect(main).toContain("<FileTabStrip");
				expect(main).toContain("<MonacoEditor");
				expect(main).toContain("on_read_workspace_file");
				expect(main).toContain("on_replace_workspace_file");
				expect(main).toContain("expected_before");
			}),
		);

		it.effect("renders every live product surface without fixture records", () =>
			Effect.gen(function* () {
				const { aggregate, sources } = yield* WorkspaceSources;
				const left = yield* SourceNamed(sources, "left-pane.sv");

				expect(left).toContain("No backend threads yet.");
				for (const projection of [
					"transcript",
					"orchestration_graph",
					"workspace_changes",
					"preview_targets",
					"tool_approvals",
					"routines",
					"capabilities",
				])
					expect(aggregate).toContain(projection);
				expect(aggregate).not.toMatch(/editor-fixtures|fixture/i);
			}),
		);

		it.effect("keeps keyboard sending Effect-owned and prevents the composing newline", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const chat = yield* SourceNamed(sources, "chat-transcript.sv");

				expect(chat).toContain("event.preventDefault()");
				expect(chat).toContain("event.metaKey || event.ctrlKey");
				expect(chat).toContain("yield* on_send(text)");
			}),
		);

		it.effect("allows a fresh project-backed chat and intake answer before a run exists", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const chat = yield* SourceNamed(sources, "chat-transcript.sv");

				expect(chat).toContain("selected_thread?.primary_project !== undefined");
				expect(chat).toContain('pending_question?.state === "pending"');
				expect(chat).toContain("disabled={draft.trim().length === 0 || !can_send}");
				expect(chat).not.toContain("Option.isNone(snapshot.thread_work)");
			}),
		);

		it.effect("keeps component behavior in SER without browser-side runners", () =>
			Effect.gen(function* () {
				const { aggregate } = yield* WorkspaceSources;

				expect(aggregate).not.toMatch(/Effect\.run\w*/);
				expect(aggregate).not.toContain("setTimeout(");
				expect(aggregate).not.toContain("new Promise");
				expect(aggregate).not.toContain("onMount(");
			}),
		);
	});
});
