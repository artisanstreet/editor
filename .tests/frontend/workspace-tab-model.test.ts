import { describe, expect, it } from "@effect/vitest";
import { Effect, Option } from "effect";

import {
	ActivateTab,
	CloseTab,
	ConfirmCloseTab,
	CreateWorkspaceState,
	DeriveTabOverflow,
	DoubleClickTab,
	EditTab,
	OpenDiffPreview,
	OpenFile,
	OpenPreview,
	PinTab,
	RecordAgentChange,
	SwitchMode,
	UpdateChatView,
	UpdateEditorView,
	UpdateOrchestratorView,
	type TabMutationOutcome,
	type WorkspaceFileReference,
	type WorkspaceState,
	type WorkspaceTab,
} from "../../modules/frontend/src/lib/workspace/tab-model";

const FileA: WorkspaceFileReference = {
	id: "a",
	name: "alpha.ts",
	language: "TypeScript",
	path: "modules/alpha.ts",
};

const FileB: WorkspaceFileReference = {
	id: "b",
	name: "beta.ts",
	language: "TypeScript",
	path: "modules/beta.ts",
};

const FileC: WorkspaceFileReference = {
	id: "c",
	name: "gamma.sv",
	language: "Svelte",
	path: "modules/gamma.sv",
};

const FileD: WorkspaceFileReference = {
	id: "d",
	name: "delta.css",
	language: "CSS",
	path: "modules/delta.css",
};

const TabByFile = (state: WorkspaceState, file_id: string) =>
	Effect.gen(function* () {
		yield* Effect.void;

		for (const tab of state.tabs) {
			if (tab.file.id === file_id) {
				return Option.some(tab);
			}
		}

		return Option.none<WorkspaceTab>();
	});

const UpdatedState = (outcome: TabMutationOutcome) =>
	Effect.gen(function* () {
		if (outcome._tag === "Updated") {
			return outcome.state;
		}

		return yield* Effect.die(`Expected Updated, received ${outcome._tag}`);
	});

describe("workspace tab model", () => {
	it.effect("replaces the one unpromoted preview tab", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA]);
			state = yield* OpenPreview(state, FileB);
			state = yield* OpenPreview(state, FileC);

			expect(state.tabs).toHaveLength(2);
			expect(Option.isNone(yield* TabByFile(state, FileB.id))).toBe(true);
			const preview = yield* TabByFile(state, FileC.id);
			expect(Option.isSome(preview) ? preview.value.ownership._tag : "missing").toBe(
				"Preview",
			);
		}),
	);

	it.effect("pins a preview so later temporary navigation cannot replace it", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA]);
			state = yield* OpenPreview(state, FileB);
			state = yield* UpdatedState(yield* PinTab(state, "file:b"));
			state = yield* OpenPreview(state, FileC);

			expect(state.tabs).toHaveLength(3);
			const pinned = yield* TabByFile(state, FileB.id);
			expect(Option.isSome(pinned) ? pinned.value.ownership._tag : "missing").toBe("Pinned");
		}),
	);

	it.effect("claims an existing preview when the user explicitly opens it", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA]);
			state = yield* OpenPreview(state, FileB);
			state = yield* OpenFile(state, FileB);
			state = yield* OpenPreview(state, FileC);

			expect(state.tabs).toHaveLength(3);
			const claimed = yield* TabByFile(state, FileB.id);
			expect(Option.isSome(claimed) ? claimed.value.ownership._tag : "missing").toBe("Open");
		}),
	);

	it.effect("promotes previews through double-click and edit", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState();
			state = yield* OpenPreview(state, FileA);
			state = yield* UpdatedState(yield* DoubleClickTab(state, "file:a"));

			let promoted = yield* TabByFile(state, FileA.id);
			expect(Option.isSome(promoted) ? promoted.value.ownership._tag : "missing").toBe(
				"Open",
			);

			state = yield* OpenPreview(state, FileB);
			state = yield* UpdatedState(yield* EditTab(state, "file:b"));
			promoted = yield* TabByFile(state, FileB.id);
			expect(Option.isSome(promoted) ? promoted.value.ownership._tag : "missing").toBe(
				"Open",
			);
			expect(Option.isSome(promoted) ? promoted.value.edit_state._tag : "missing").toBe(
				"Dirty",
			);
		}),
	);

	it.effect("keeps pinned ownership monotonic and rejects editing a diff preview", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState();
			state = yield* OpenPreview(state, FileA);
			state = yield* UpdatedState(yield* PinTab(state, "file:a"));
			state = yield* UpdatedState(yield* DoubleClickTab(state, "file:a"));

			const pinned = yield* TabByFile(state, FileA.id);
			expect(Option.isSome(pinned) ? pinned.value.ownership._tag : "missing").toBe("Pinned");

			state = yield* OpenDiffPreview(state, FileB, "change-17");
			const diff_id = Option.getOrUndefined(state.active_tab_id);
			if (diff_id === undefined) {
				return yield* Effect.die("Expected an active diff preview");
			}
			const edit = yield* EditTab(state, diff_id);
			expect(edit._tag).toBe("UnsupportedTabOperation");
			const diff = state.tabs[state.tabs.length - 1];
			expect(diff?.edit_state._tag).toBe("Clean");
		}),
	);

	it.effect("does not apply dirty consent to a newly reopened tab incarnation", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA]);
			state = yield* UpdatedState(yield* EditTab(state, "file:a"));
			const old_guard = yield* CloseTab(state, "file:a");

			if (old_guard._tag !== "ConfirmationRequired") {
				return yield* Effect.die("Expected original dirty-close confirmation");
			}

			const current_guard = yield* CloseTab(state, "file:a");
			if (current_guard._tag !== "ConfirmationRequired") {
				return yield* Effect.die("Expected current dirty-close confirmation");
			}

			const closed = yield* ConfirmCloseTab(state, current_guard.confirmation);
			state = closed.state;
			state = yield* OpenFile(state, FileA);
			state = yield* UpdatedState(yield* EditTab(state, "file:a"));

			const stale = yield* ConfirmCloseTab(state, old_guard.confirmation);
			expect(stale._tag).toBe("ConfirmationStale");
			expect(stale.state.tabs).toHaveLength(1);
		}),
	);

	it.effect("requires confirmation before closing dirty work and then chooses a successor", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA, FileB]);
			state = yield* UpdatedState(yield* ActivateTab(state, "file:b"));
			state = yield* UpdatedState(yield* EditTab(state, "file:b"));

			const guarded = yield* CloseTab(state, "file:b");
			expect(guarded._tag).toBe("ConfirmationRequired");
			expect(guarded.state.tabs).toHaveLength(2);

			if (guarded._tag !== "ConfirmationRequired") {
				return yield* Effect.die("Expected dirty-close confirmation");
			}

			const closed = yield* ConfirmCloseTab(state, guarded.confirmation);
			expect(closed._tag).toBe("Closed");
			expect(closed.state.tabs).toHaveLength(1);
			expect(Option.getOrUndefined(closed.state.active_tab_id)).toBe("file:a");
		}),
	);

	it.effect("rejects stale dirty-close consent after another edit", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA]);
			state = yield* UpdatedState(yield* EditTab(state, "file:a"));
			const guarded = yield* CloseTab(state, "file:a");

			if (guarded._tag !== "ConfirmationRequired") {
				return yield* Effect.die("Expected dirty-close confirmation");
			}

			state = yield* UpdatedState(yield* EditTab(state, "file:a"));
			const stale = yield* ConfirmCloseTab(state, guarded.confirmation);

			expect(stale._tag).toBe("ConfirmationStale");
			expect(stale.state.tabs).toHaveLength(1);
			const tab = yield* TabByFile(stale.state, FileA.id);
			expect(
				Option.isSome(tab) && tab.value.edit_state._tag === "Dirty"
					? tab.value.edit_state.revision
					: 0,
			).toBe(2);
		}),
	);

	it.effect("closes a clean active tab and selects its nearest neighbor", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA, FileB, FileC]);
			state = yield* UpdatedState(yield* ActivateTab(state, "file:b"));
			const closed = yield* CloseTab(state, "file:b");

			expect(closed._tag).toBe("Closed");
			expect(Option.getOrUndefined(closed.state.active_tab_id)).toBe("file:c");
		}),
	);

	it.effect("opens a distinct diff preview", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA]);
			state = yield* OpenDiffPreview(state, FileB, "change-17");
			const diff = state.tabs[1];

			expect(diff?.id).toContain('"change-17"');
			expect(diff?.content).toEqual({ _tag: "DiffPreview", change_id: "change-17" });
			expect(diff?.ownership._tag).toBe("Preview");
		}),
	);

	it.effect("keeps successive same-file diff reviews distinct", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA]);
			state = yield* OpenDiffPreview(state, FileB, "change-17");
			const first_diff_id = Option.getOrUndefined(state.active_tab_id);
			if (first_diff_id === undefined) {
				return yield* Effect.die("Expected first diff preview");
			}
			state = yield* UpdatedState(yield* PinTab(state, first_diff_id));
			state = yield* OpenDiffPreview(state, FileB, "change-18");

			expect(state.tabs).toHaveLength(3);
			expect(state.tabs[1]?.id).toBe(first_diff_id);
			expect(state.tabs[2]?.id).not.toBe(first_diff_id);
			expect(state.tabs[2]?.content).toEqual({
				_tag: "DiffPreview",
				change_id: "change-18",
			});
		}),
	);

	it.effect("uses injective identities for diff file and change tuples", () =>
		Effect.gen(function* () {
			const FileWithColon = { ...FileA, id: "a:b" };
			let state = yield* CreateWorkspaceState();
			state = yield* OpenDiffPreview(state, FileWithColon, "c");
			const first_id = Option.getOrUndefined(state.active_tab_id);
			if (first_id === undefined) {
				return yield* Effect.die("Expected first composite diff id");
			}
			state = yield* UpdatedState(yield* PinTab(state, first_id));
			state = yield* OpenDiffPreview(state, FileA, "b:c");
			const second_id = Option.getOrUndefined(state.active_tab_id);

			expect(second_id).toBeDefined();
			expect(second_id).not.toBe(first_id);
			expect(state.tabs).toHaveLength(2);
		}),
	);

	it.effect("records changed files without opening agent-owned tabs", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA]);
			state = yield* RecordAgentChange(state, FileB, {
				agent_name: "Terra",
				added: 12,
				removed: 2,
			});

			expect(state.tabs).toHaveLength(1);
			expect(state.changed_files).toHaveLength(1);
			expect(state.changed_files[0]?.file.id).toBe(FileB.id);
		}),
	);

	it.effect("adds an agent-change badge when the user already owns the tab", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA]);
			state = yield* RecordAgentChange(state, FileA, {
				agent_name: "Terra",
				added: 4,
				removed: 1,
			});
			const tab = yield* TabByFile(state, FileA.id);

			expect(Option.isSome(tab)).toBe(true);
			expect(Option.isSome(tab) && Option.isSome(tab.value.agent_change)).toBe(true);
		}),
	);

	it.effect("replaces an existing agent badge with the latest change summary", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA]);
			state = yield* RecordAgentChange(state, FileA, {
				agent_name: "Terra",
				added: 4,
				removed: 1,
			});
			state = yield* RecordAgentChange(state, FileA, {
				agent_name: "Luna",
				added: 8,
				removed: 3,
			});
			const tab = yield* TabByFile(state, FileA.id);

			expect(
				Option.isSome(tab) ? Option.getOrUndefined(tab.value.agent_change) : undefined,
			).toEqual({
				_tag: "AgentChange",
				agent_name: "Luna",
				added: 8,
				removed: 3,
			});
			expect(state.changed_files).toHaveLength(1);
		}),
	);

	it.effect("preserves mode-local state across mode switches", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA]);
			state = yield* UpdateEditorView(state, {
				scroll_top: 480,
				cursor_line: 22,
				cursor_column: 9,
			});
			state = yield* UpdateChatView(state, {
				draft: "Keep this draft",
				transcript_scroll_top: 920,
			});
			state = yield* UpdateOrchestratorView(state, {
				selected_node_id: Option.some("node-terra"),
				graph_scroll_top: 310,
			});
			state = yield* SwitchMode(state, "chat");
			state = yield* SwitchMode(state, "orchestrator");
			state = yield* SwitchMode(state, "editor");

			expect(state.editor).toEqual({ scroll_top: 480, cursor_line: 22, cursor_column: 9 });
			expect(state.chat).toEqual({ draft: "Keep this draft", transcript_scroll_top: 920 });
			expect(Option.getOrUndefined(state.orchestrator.selected_node_id)).toBe("node-terra");
			expect(state.orchestrator.graph_scroll_top).toBe(310);
			expect(Option.getOrUndefined(state.active_tab_id)).toBe("file:a");
		}),
	);

	it.effect("keeps recent files deduplicated in most-recent-first order", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState();
			state = yield* OpenFile(state, FileA);
			state = yield* OpenFile(state, FileB);
			state = yield* OpenFile(state, FileA);

			expect(state.recent_files).toEqual([FileA, FileB]);
		}),
	);

	it.effect("keeps the active tab visible when tabs overflow", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA, FileB, FileC, FileD]);
			state = yield* UpdatedState(yield* ActivateTab(state, "file:d"));
			const overflow = yield* DeriveTabOverflow(state, 2);

			expect(overflow.visible).toHaveLength(2);
			expect(overflow.overflow).toHaveLength(2);
			expect(overflow.visible[0]?.id).toBe("file:d");
			let active_is_visible = false;
			for (const tab of overflow.visible) {
				active_is_visible ||= tab.id === "file:d";
			}
			expect(active_is_visible).toBe(true);
		}),
	);

	it.effect("activates an existing file instead of duplicating its tab", () =>
		Effect.gen(function* () {
			let state = yield* CreateWorkspaceState([FileA, FileB]);
			state = yield* OpenFile(state, FileA);

			expect(state.tabs).toHaveLength(2);
			expect(Option.getOrUndefined(state.active_tab_id)).toBe("file:a");
		}),
	);
});
