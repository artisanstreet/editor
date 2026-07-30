import { Effect, Option, Schema } from "effect";

export const WorkspaceMode = Schema.Literals(["editor", "chat", "orchestrator"]);
export type WorkspaceMode = typeof WorkspaceMode.Type;

export const WorkspaceFileReference = Schema.Struct({
	id: Schema.String,
	name: Schema.String,
	language: Schema.String,
	path: Schema.String,
});
export type WorkspaceFileReference = typeof WorkspaceFileReference.Type;

export type TabOwnership =
	| { readonly _tag: "Preview" }
	| { readonly _tag: "Open" }
	| { readonly _tag: "Pinned" };

export type TabContent =
	| { readonly _tag: "File" }
	| { readonly _tag: "DiffPreview"; readonly change_id: string };

export type TabEditState =
	| { readonly _tag: "Clean" }
	| { readonly _tag: "Dirty"; readonly revision: number };

export interface AgentChangeBadge {
	readonly _tag: "AgentChange";
	readonly agent_name: string;
	readonly added: number;
	readonly removed: number;
}

export interface WorkspaceTab {
	readonly id: string;
	readonly generation: number;
	readonly file: WorkspaceFileReference;
	readonly ownership: TabOwnership;
	readonly content: TabContent;
	readonly edit_state: TabEditState;
	readonly agent_change: Option.Option<AgentChangeBadge>;
}

export interface EditorViewState {
	readonly scroll_top: number;
	readonly cursor_line: number;
	readonly cursor_column: number;
}

export interface ChatViewState {
	readonly draft: string;
	readonly transcript_scroll_top: number;
}

export interface OrchestratorViewState {
	readonly selected_node_id: Option.Option<string>;
	readonly graph_scroll_top: number;
}

export interface ChangedFile {
	readonly file: WorkspaceFileReference;
	readonly change: AgentChangeBadge;
}

export interface WorkspaceState {
	readonly mode: WorkspaceMode;
	readonly tabs: ReadonlyArray<WorkspaceTab>;
	readonly active_tab_id: Option.Option<string>;
	readonly recent_files: ReadonlyArray<WorkspaceFileReference>;
	readonly changed_files: ReadonlyArray<ChangedFile>;
	readonly editor: EditorViewState;
	readonly chat: ChatViewState;
	readonly orchestrator: OrchestratorViewState;
	readonly next_tab_generation: number;
}

export type TabMutationOutcome =
	| { readonly _tag: "Updated"; readonly state: WorkspaceState }
	| {
			readonly _tag: "TabNotFound";
			readonly state: WorkspaceState;
			readonly tab_id: string;
	  }
	| {
			readonly _tag: "UnsupportedTabOperation";
			readonly state: WorkspaceState;
			readonly tab_id: string;
			readonly operation: "edit_diff_preview";
	  };

export interface DirtyCloseConfirmation {
	readonly tab_id: string;
	readonly tab_generation: number;
	readonly dirty_revision: number;
}

export type CloseTabOutcome =
	| { readonly _tag: "Closed"; readonly state: WorkspaceState }
	| {
			readonly _tag: "ConfirmationRequired";
			readonly state: WorkspaceState;
			readonly tab: WorkspaceTab;
			readonly confirmation: DirtyCloseConfirmation;
	  }
	| {
			readonly _tag: "ConfirmationStale";
			readonly state: WorkspaceState;
			readonly tab: WorkspaceTab;
			readonly confirmation: DirtyCloseConfirmation;
	  }
	| {
			readonly _tag: "TabNotFound";
			readonly state: WorkspaceState;
			readonly tab_id: string;
	  };

export interface TabOverflow {
	readonly visible: ReadonlyArray<WorkspaceTab>;
	readonly overflow: ReadonlyArray<WorkspaceTab>;
}

const FileTabId = (file_id: string) =>
	Effect.gen(function* () {
		yield* Effect.void;

		return `file:${file_id}`;
	});

const DiffTabId = (file_id: string, change_id: string) =>
	Effect.gen(function* () {
		yield* Effect.void;

		return JSON.stringify(["diff", file_id, change_id]);
	});

const FindTabIndex = (tabs: ReadonlyArray<WorkspaceTab>, tab_id: string) =>
	Effect.gen(function* () {
		yield* Effect.void;

		for (let index = 0; index < tabs.length; index += 1) {
			if (tabs[index]?.id === tab_id) {
				return Option.some(index);
			}
		}

		return Option.none<number>();
	});

const WithRecentFile = (state: WorkspaceState, file: WorkspaceFileReference) =>
	Effect.gen(function* () {
		yield* Effect.void;

		const recent_files: Array<WorkspaceFileReference> = [file];

		for (const recent_file of state.recent_files) {
			if (recent_file.id !== file.id) {
				recent_files.push(recent_file);
			}
		}

		return { ...state, recent_files };
	});

const MakeFileTab = (
	file: WorkspaceFileReference,
	ownership: TabOwnership,
	content: TabContent,
	generation: number,
) =>
	Effect.gen(function* () {
		const id =
			content._tag === "DiffPreview"
				? yield* DiffTabId(file.id, content.change_id)
				: yield* FileTabId(file.id);

		return {
			id,
			generation,
			file,
			ownership,
			content,
			edit_state: { _tag: "Clean" },
			agent_change: Option.none<AgentChangeBadge>(),
		} satisfies WorkspaceTab;
	});

const ReplaceTab = (state: WorkspaceState, tab_index: number, replacement: WorkspaceTab) =>
	Effect.gen(function* () {
		yield* Effect.void;

		const tabs = [...state.tabs];
		tabs[tab_index] = replacement;

		return { ...state, tabs };
	});

const RemoveTabAt = (state: WorkspaceState, tab_index: number) =>
	Effect.gen(function* () {
		yield* Effect.void;

		const tabs = [...state.tabs];
		tabs.splice(tab_index, 1);

		let active_tab_id = state.active_tab_id;
		if (Option.isSome(active_tab_id) && active_tab_id.value === state.tabs[tab_index]?.id) {
			const successor = tabs[Math.min(tab_index, tabs.length - 1)];
			active_tab_id = successor === undefined ? Option.none() : Option.some(successor.id);
		}

		return { ...state, tabs, active_tab_id };
	});

export const CreateWorkspaceState = (
	initial_files: ReadonlyArray<WorkspaceFileReference> = [],
): Effect.Effect<WorkspaceState> =>
	Effect.gen(function* () {
		const tabs: Array<WorkspaceTab> = [];

		for (const file of initial_files) {
			tabs.push(yield* MakeFileTab(file, { _tag: "Open" }, { _tag: "File" }, tabs.length));
		}

		const active_tab = tabs[0];

		return {
			mode: "editor",
			tabs,
			active_tab_id: active_tab === undefined ? Option.none() : Option.some(active_tab.id),
			recent_files: [...initial_files].reverse(),
			changed_files: [],
			editor: { scroll_top: 0, cursor_line: 1, cursor_column: 1 },
			chat: { draft: "", transcript_scroll_top: 0 },
			orchestrator: { selected_node_id: Option.none(), graph_scroll_top: 0 },
			next_tab_generation: tabs.length,
		} satisfies WorkspaceState;
	});

export const SwitchMode = (state: WorkspaceState, mode: WorkspaceMode) =>
	Effect.gen(function* () {
		yield* Effect.void;

		return { ...state, mode };
	});

export const ActivateTab = (state: WorkspaceState, tab_id: string) =>
	Effect.gen(function* () {
		const tab_index = yield* FindTabIndex(state.tabs, tab_id);

		if (Option.isNone(tab_index)) {
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;
		}

		const tab = state.tabs.at(tab_index.value);
		if (tab === undefined) {
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;
		}
		const activated = yield* WithRecentFile(
			{ ...state, active_tab_id: Option.some(tab.id) },
			tab.file,
		);

		return { _tag: "Updated", state: activated } satisfies TabMutationOutcome;
	});

export const OpenFile = (state: WorkspaceState, file: WorkspaceFileReference) =>
	Effect.gen(function* () {
		const tab_id = yield* FileTabId(file.id);
		const existing = yield* FindTabIndex(state.tabs, tab_id);

		if (Option.isSome(existing)) {
			const existing_tab = state.tabs.at(existing.value);
			if (existing_tab === undefined) return state;
			const ownership =
				existing_tab.ownership._tag === "Preview"
					? ({ _tag: "Open" } as const)
					: existing_tab.ownership;
			const promoted = yield* ReplaceTab(state, existing.value, {
				...existing_tab,
				ownership,
			});
			const activated = yield* ActivateTab(promoted, tab_id);

			return activated.state;
		}

		const tab = yield* MakeFileTab(
			file,
			{ _tag: "Open" },
			{ _tag: "File" },
			state.next_tab_generation,
		);
		return yield* WithRecentFile(
			{
				...state,
				tabs: [...state.tabs, tab],
				active_tab_id: Option.some(tab.id),
				next_tab_generation: state.next_tab_generation + 1,
			},
			file,
		);
	});

export const OpenPreview = (state: WorkspaceState, file: WorkspaceFileReference) =>
	Effect.gen(function* () {
		const tab_id = yield* FileTabId(file.id);
		const existing = yield* FindTabIndex(state.tabs, tab_id);

		if (Option.isSome(existing)) {
			const activated = yield* ActivateTab(state, tab_id);

			return activated.state;
		}

		const retained_tabs: Array<WorkspaceTab> = [];
		for (const tab of state.tabs) {
			if (tab.ownership._tag !== "Preview") {
				retained_tabs.push(tab);
			}
		}

		const preview = yield* MakeFileTab(
			file,
			{ _tag: "Preview" },
			{ _tag: "File" },
			state.next_tab_generation,
		);
		return yield* WithRecentFile(
			{
				...state,
				tabs: [...retained_tabs, preview],
				active_tab_id: Option.some(preview.id),
				next_tab_generation: state.next_tab_generation + 1,
			},
			file,
		);
	});

export const OpenDiffPreview = (
	state: WorkspaceState,
	file: WorkspaceFileReference,
	change_id: string,
) =>
	Effect.gen(function* () {
		const tab_id = yield* DiffTabId(file.id, change_id);
		const existing = yield* FindTabIndex(state.tabs, tab_id);

		if (Option.isSome(existing)) {
			const activated = yield* ActivateTab(state, tab_id);

			return activated.state;
		}

		const retained_tabs: Array<WorkspaceTab> = [];
		for (const tab of state.tabs) {
			if (tab.ownership._tag !== "Preview") {
				retained_tabs.push(tab);
			}
		}

		const preview = yield* MakeFileTab(
			file,
			{ _tag: "Preview" },
			{ _tag: "DiffPreview", change_id },
			state.next_tab_generation,
		);
		return yield* WithRecentFile(
			{
				...state,
				tabs: [...retained_tabs, preview],
				active_tab_id: Option.some(preview.id),
				next_tab_generation: state.next_tab_generation + 1,
			},
			file,
		);
	});

export const PromoteTab = (
	state: WorkspaceState,
	tab_id: string,
	ownership: Exclude<TabOwnership, { readonly _tag: "Preview" }> = { _tag: "Open" },
) =>
	Effect.gen(function* () {
		const tab_index = yield* FindTabIndex(state.tabs, tab_id);

		if (Option.isNone(tab_index)) {
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;
		}

		const tab = state.tabs.at(tab_index.value);
		if (tab === undefined) {
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;
		}
		const next_ownership =
			tab.ownership._tag === "Pinned" && ownership._tag === "Open"
				? tab.ownership
				: ownership;
		const updated = yield* ReplaceTab(state, tab_index.value, {
			...tab,
			ownership: next_ownership,
		});

		return { _tag: "Updated", state: updated } satisfies TabMutationOutcome;
	});

export const DoubleClickTab = (state: WorkspaceState, tab_id: string) =>
	Effect.gen(function* () {
		return yield* PromoteTab(state, tab_id, { _tag: "Open" });
	});

export const PinTab = (state: WorkspaceState, tab_id: string) =>
	Effect.gen(function* () {
		return yield* PromoteTab(state, tab_id, { _tag: "Pinned" });
	});

export const EditTab = (state: WorkspaceState, tab_id: string) =>
	Effect.gen(function* () {
		const tab_index = yield* FindTabIndex(state.tabs, tab_id);

		if (Option.isNone(tab_index)) {
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;
		}

		const tab = state.tabs.at(tab_index.value);
		if (tab === undefined) {
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;
		}
		if (tab.content._tag === "DiffPreview") {
			return {
				_tag: "UnsupportedTabOperation",
				state,
				tab_id,
				operation: "edit_diff_preview",
			} satisfies TabMutationOutcome;
		}

		const revision = tab.edit_state._tag === "Dirty" ? tab.edit_state.revision + 1 : 1;
		const ownership =
			tab.ownership._tag === "Preview" ? ({ _tag: "Open" } as const) : tab.ownership;
		const updated = yield* ReplaceTab(state, tab_index.value, {
			...tab,
			ownership,
			edit_state: { _tag: "Dirty", revision },
		});

		return { _tag: "Updated", state: updated } satisfies TabMutationOutcome;
	});

export const CloseTab = (state: WorkspaceState, tab_id: string) =>
	Effect.gen(function* () {
		const tab_index = yield* FindTabIndex(state.tabs, tab_id);

		if (Option.isNone(tab_index)) {
			return { _tag: "TabNotFound", state, tab_id } satisfies CloseTabOutcome;
		}

		const tab = state.tabs.at(tab_index.value);
		if (tab === undefined) {
			return { _tag: "TabNotFound", state, tab_id } satisfies CloseTabOutcome;
		}
		if (tab.edit_state._tag === "Dirty") {
			return {
				_tag: "ConfirmationRequired",
				state,
				tab,
				confirmation: {
					tab_id: tab.id,
					tab_generation: tab.generation,
					dirty_revision: tab.edit_state.revision,
				},
			} satisfies CloseTabOutcome;
		}

		return {
			_tag: "Closed",
			state: yield* RemoveTabAt(state, tab_index.value),
		} satisfies CloseTabOutcome;
	});

export const ConfirmCloseTab = (state: WorkspaceState, confirmation: DirtyCloseConfirmation) =>
	Effect.gen(function* () {
		const tab_index = yield* FindTabIndex(state.tabs, confirmation.tab_id);

		if (Option.isNone(tab_index)) {
			return {
				_tag: "TabNotFound",
				state,
				tab_id: confirmation.tab_id,
			} satisfies CloseTabOutcome;
		}

		const tab = state.tabs.at(tab_index.value);
		if (tab === undefined) {
			return {
				_tag: "TabNotFound",
				state,
				tab_id: confirmation.tab_id,
			} satisfies CloseTabOutcome;
		}
		if (
			tab.generation !== confirmation.tab_generation ||
			tab.edit_state._tag !== "Dirty" ||
			tab.edit_state.revision !== confirmation.dirty_revision
		) {
			return {
				_tag: "ConfirmationStale",
				state,
				tab,
				confirmation,
			} satisfies CloseTabOutcome;
		}

		return {
			_tag: "Closed",
			state: yield* RemoveTabAt(state, tab_index.value),
		} satisfies CloseTabOutcome;
	});

export const RecordAgentChange = (
	state: WorkspaceState,
	file: WorkspaceFileReference,
	change: Omit<AgentChangeBadge, "_tag">,
) =>
	Effect.gen(function* () {
		yield* Effect.void;

		const badge = { _tag: "AgentChange", ...change } satisfies AgentChangeBadge;
		const tabs: Array<WorkspaceTab> = [];

		for (const tab of state.tabs) {
			tabs.push(tab.file.id === file.id ? { ...tab, agent_change: Option.some(badge) } : tab);
		}

		const changed_files: Array<ChangedFile> = [{ file, change: badge }];
		for (const changed_file of state.changed_files) {
			if (changed_file.file.id !== file.id) {
				changed_files.push(changed_file);
			}
		}

		return { ...state, tabs, changed_files };
	});

export const UpdateEditorView = (state: WorkspaceState, editor: EditorViewState) =>
	Effect.gen(function* () {
		yield* Effect.void;

		return { ...state, editor };
	});

export const UpdateChatView = (state: WorkspaceState, chat: ChatViewState) =>
	Effect.gen(function* () {
		yield* Effect.void;

		return { ...state, chat };
	});

export const UpdateOrchestratorView = (
	state: WorkspaceState,
	orchestrator: OrchestratorViewState,
) =>
	Effect.gen(function* () {
		yield* Effect.void;

		return { ...state, orchestrator };
	});

export const DeriveTabOverflow = (state: WorkspaceState, max_visible: number) =>
	Effect.gen(function* () {
		yield* Effect.void;

		const limit = Math.max(1, Math.trunc(max_visible));
		if (state.tabs.length <= limit) {
			return { visible: state.tabs, overflow: [] } satisfies TabOverflow;
		}

		const visible = state.tabs.slice(0, limit);
		if (Option.isSome(state.active_tab_id)) {
			let active_is_visible = false;
			for (const tab of visible) {
				active_is_visible ||= tab.id === state.active_tab_id.value;
			}

			if (!active_is_visible) {
				const active_index = yield* FindTabIndex(state.tabs, state.active_tab_id.value);
				if (Option.isSome(active_index)) {
					visible.pop();
					const active_tab = state.tabs.at(active_index.value);
					if (active_tab !== undefined) visible.unshift(active_tab);
				}
			}
		}

		const overflow: Array<WorkspaceTab> = [];
		for (const tab of state.tabs) {
			let is_visible = false;
			for (const visible_tab of visible) {
				is_visible ||= visible_tab.id === tab.id;
			}

			if (!is_visible) {
				overflow.push(tab);
			}
		}

		return { visible, overflow } satisfies TabOverflow;
	});

export const DeriveBreadcrumbs = (file: WorkspaceFileReference) =>
	Effect.gen(function* () {
		yield* Effect.void;

		const breadcrumbs: Array<string> = [];
		for (const segment of file.path.split("/")) {
			if (segment.length > 0) {
				breadcrumbs.push(segment);
			}
		}

		return breadcrumbs;
	});
