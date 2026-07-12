import { readFileSync } from "node:fs";
import { basename, join } from "node:path";

import { describe, expect, layer } from "@effect/vitest";
import { Context, Effect, Layer } from "effect";

const component_root = join(process.cwd(), "modules/frontend/src/routes/components");
const source_paths = [
	join(component_root, "editor-fixtures.ts"),
	join(component_root, "main-pane.sv"),
	join(component_root, "file-tab-strip.sv"),
	join(component_root, "workspace-navigation.sv"),
	join(component_root, "quick-open.sv"),
	join(process.cwd(), "modules/frontend/src/lib/styles/global.css"),
] as const;

interface WorkspaceSource {
	readonly name: string;
	readonly source: string;
}

class WorkspaceSources extends Context.Service<
	WorkspaceSources,
	{
		readonly aggregate: string;
		readonly sources: ReadonlyArray<WorkspaceSource>;
	}
>()("Artisan/Test/WorkspaceSources") {}

const WorkspaceSourcesLive = Layer.effect(
	WorkspaceSources,
	Effect.gen(function* () {
		const sources: Array<WorkspaceSource> = [];
		for (const path of source_paths) {
			const source = yield* Effect.try({
				try: () => readFileSync(path, "utf8"),
				catch: (cause) => cause,
			});
			sources.push({ name: basename(path), source });
		}
		const aggregate: Array<string> = [];
		for (const source of sources) {
			aggregate.push(source.source);
		}

		return WorkspaceSources.of({
			aggregate: aggregate.join("\n"),
			sources,
		});
	}),
);

const SourceNamed = (sources: ReadonlyArray<WorkspaceSource>, name: string) =>
	Effect.gen(function* () {
		yield* Effect.void;

		for (const source of sources) {
			if (source.name === name) {
				return source.source;
			}
		}

		return yield* Effect.die(`Missing workspace source ${name}`);
	});

describe("fixture-first workspace UI source", () => {
	layer(WorkspaceSourcesLive)((it) => {
		it.effect("integrates the immutable workspace model as the only tab authority", () =>
			Effect.gen(function* () {
				const { aggregate, sources } = yield* WorkspaceSources;
				const main_pane = yield* SourceNamed(sources, "main-pane.sv");

				for (const model_operation of [
					"CreateWorkspaceFixtureState",
					"DeriveTabOverflow",
					"ActivateTab",
					"PinTab",
					"DoubleClickTab",
					"CloseTab",
					"ConfirmCloseTab",
					"OpenPreview",
					"OpenDiffPreview",
					"SwitchMode",
				]) {
					expect(aggregate).toContain(model_operation);
				}
				expect(main_pane).toContain("let workspace = $state.raw(initial_view.workspace)");
				expect(main_pane).toContain("DeriveTabOverflow(workspace, 3)");
				expect(main_pane).toContain("Before and after fixture diff");
				expect(main_pane).toContain('class="diff-side removed"');
				expect(main_pane).toContain('class="diff-side added"');
				expect(main_pane).not.toContain("active_file_index");
			}),
		);

		it.effect("seeds every tab state while leaving an agent-only change unopened", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const fixtures = yield* SourceNamed(sources, "editor-fixtures.ts");

				expect(fixtures).toContain("CreateWorkspaceState([service!, protocol!, test!])");
				expect(fixtures).toContain('PinTab(state, "file:workspace-protocol")');
				expect(fixtures).toContain('EditTab(state, "file:workspace-test")');
				expect(fixtures).toContain("OpenDiffPreview(state, style!");
				expect(fixtures).toContain("PinTab(state, state.tabs.at(-1)!.id)");
				expect(fixtures).toContain("OpenPreview(state, shell!)");
				expect(fixtures).toContain("RecordAgentChange(state, service!");
				expect(fixtures).toContain("RecordAgentChange(state, changed_only!");
				expect(fixtures).not.toMatch(
					/Open(?:File|Preview|DiffPreview)\(state, changed_only/,
				);
			}),
		);

		it.effect("renders explicit non-color-only tab states and separate actions", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const tabs = yield* SourceNamed(sources, "file-tab-strip.sv");

				for (const state_label of [
					"Preview",
					"Pinned",
					"Dirty",
					"Diff preview",
					"Agent change",
				]) {
					expect(tabs).toContain(state_label);
				}
				expect(tabs).toContain("ondblclick={yield* on_promote(tab.id)}");
				expect(tabs).toContain('aria-label={tab.ownership._tag === "Pinned"');
				expect(tabs).toContain("aria-label={`Close ${tab.file.name}`}");
				expect(tabs).toContain('role="group" aria-label="Editor file tabs"');
				expect(tabs).not.toContain('role="tablist"');
				expect(tabs).toContain("overflow-x: auto");
			}),
		);

		it.effect("passes the exact dirty token and safely refreshes stale consent", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const tabs = yield* SourceNamed(sources, "file-tab-strip.sv");
				const main_pane = yield* SourceNamed(sources, "main-pane.sv");

				expect(tabs).toContain("const exact_confirmation = pending_confirmation");
				expect(tabs).toContain("on_confirm_close(exact_confirmation)");
				expect(tabs).toMatch(
					/ConfirmationStale[\s\S]*on_close\(exact_confirmation\.tab_id\)/,
				);
				expect(main_pane).toContain("ConfirmCloseTab(workspace, confirmation)");
			}),
		);

		it.effect("keeps mode controls separate and restores real mode viewports", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const main_pane = yield* SourceNamed(sources, "main-pane.sv");

				expect(main_pane.indexOf("<ModeSwitcher")).toBeLessThan(
					main_pane.indexOf("<FileTabStrip"),
				);
				expect(main_pane).toContain("bind:this={editor_viewport}");
				expect(main_pane).toContain("bind:this={chat_viewport}");
				expect(main_pane).toContain("bind:this={orchestrator_viewport}");
				expect(main_pane).toContain("editor_viewport.scrollTop = editor_scroll_top");
				expect(main_pane).toContain("chat_viewport.scrollTop = chat_scroll_top");
				expect(main_pane).toContain(
					"orchestrator_viewport.scrollTop = orchestrator_scroll_top",
				);
				expect(main_pane).toContain("UpdateChatDraft");
				expect(main_pane).toContain("SelectNode");
			}),
		);

		it.effect("provides intentional recent, changed, and overflow navigation", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const navigation = yield* SourceNamed(sources, "workspace-navigation.sv");
				const main_pane = yield* SourceNamed(sources, "main-pane.sv");

				for (const label of ["Recent files", "Changed files", "Overflow tabs"]) {
					expect(navigation).toContain(`aria-label="${label}"`);
				}
				expect(main_pane).toContain("OpenPreview(workspace, file)");
				expect(main_pane).toContain(
					"OpenDiffPreview(workspace, changed_file.file, change_id)",
				);
				expect(navigation).toContain("bind:value={recent_selection}");
				expect(navigation).toContain("bind:value={changed_selection}");
				expect(navigation).toContain("bind:value={overflow_selection}");
				expect(navigation).toContain("TabOptionLabel");
			}),
		);

		it.effect("implements keyboard quick open as an accessible animated modal", () =>
			Effect.gen(function* () {
				const { sources } = yield* WorkspaceSources;
				const quick_open = yield* SourceNamed(sources, "quick-open.sv");
				const global_css = yield* SourceNamed(sources, "global.css");

				expect(quick_open).toContain("keyboard_event.ctrlKey || keyboard_event.metaKey");
				for (const key of ["ArrowDown", "ArrowUp", "Enter", "Escape"]) {
					expect(quick_open).toContain(`keyboard_event.key === "${key}"`);
				}
				expect(quick_open).toContain('role="dialog"');
				expect(quick_open).toContain('aria-modal="true"');
				expect(quick_open).toContain("Fixture files");
				expect(quick_open).toContain("trigger?.focus()");
				expect(quick_open).toContain("TrapDialogFocus");
				expect(quick_open).toContain('keyboard_event.key === "Tab"');
				expect(quick_open).toContain("transition_generation !== close_generation");
				expect(quick_open).toContain("return_focus?.isConnected");
				expect(quick_open).toContain("quick-open-result-${selected_index}");
				expect(quick_open).toContain("yield* RevealSelectedResult");
				expect(quick_open).toContain("!dialog.contains(active_element)");
				expect(quick_open).toContain('class="quick-open-dialog t-modal"');
				expect(quick_open).toContain("will-change: transform, opacity");
				expect(quick_open).toContain("getComputedStyle(document.documentElement)");
				expect(quick_open).toContain("Effect.sleep(Duration.millis(close_duration))");
				expect(quick_open).toContain("@media (prefers-reduced-motion: reduce)");
				expect(global_css).toContain("--modal-close-dur: 150ms");
			}),
		);

		it.effect("keeps all owned behavior in SER and contains no copied names or assets", () =>
			Effect.gen(function* () {
				const { aggregate, sources } = yield* WorkspaceSources;

				expect(aggregate).not.toMatch(/Effect\.run\w*/);
				expect(aggregate).not.toContain("setTimeout(");
				expect(aggregate).not.toContain("new Promise");
				expect(aggregate).not.toContain("$effect(");
				expect(aggregate).not.toContain("onMount(");
				expect(aggregate).not.toMatch(
					/(?:on(?:click|input|change|keydown|scroll|dblclick))=\{\s*\(?\s*(?:event|\w+)\s*=>/,
				);

				for (const source of sources) {
					expect(`${source.name}\n${source.source}`).not.toMatch(/barekey|usebarekey/i);
					expect(source.source).not.toMatch(/(?:logo|asset)[-_]barekey/i);
				}
			}),
		);
	});
});
