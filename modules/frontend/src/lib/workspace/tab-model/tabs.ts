import { Effect, Option } from "effect";

import {
	DiffTabId,
	FileTabId,
	FindTabIndex,
	MakeFileTab,
	RemoveTabAt,
	ReplaceTab,
	WithRecentFile,
} from "./internal";
import type {
	AgentChangeBadge,
	CloseTabOutcome,
	DirtyCloseConfirmation,
	TabMutationOutcome,
	TabOwnership,
	WorkspaceFileReference,
	WorkspaceState,
} from "./types";

export const ActivateTab = (state: WorkspaceState, tab_id: string) =>
	Effect.gen(function* () {
		const tab_index = yield* FindTabIndex(state.tabs, tab_id);
		if (Option.isNone(tab_index))
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;

		const tab = state.tabs.at(tab_index.value);
		if (tab === undefined)
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;

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
			return (yield* ActivateTab(promoted, tab_id)).state;
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

const OpenPreviewTab = (
	state: WorkspaceState,
	file: WorkspaceFileReference,
	change_id: Option.Option<string>,
) =>
	Effect.gen(function* () {
		const tab_id = Option.match(change_id, {
			onNone: () => FileTabId(file.id),
			onSome: (value) => DiffTabId(file.id, value),
		});
		const resolved_tab_id = yield* tab_id;
		const existing = yield* FindTabIndex(state.tabs, resolved_tab_id);
		if (Option.isSome(existing)) return (yield* ActivateTab(state, resolved_tab_id)).state;

		const content = Option.match(change_id, {
			onNone: () => ({ _tag: "File" }) as const,
			onSome: (value) => ({ _tag: "DiffPreview", change_id: value }) as const,
		});
		const preview = yield* MakeFileTab(
			file,
			{ _tag: "Preview" },
			content,
			state.next_tab_generation,
		);
		return yield* WithRecentFile(
			{
				...state,
				tabs: [...state.tabs.filter((tab) => tab.ownership._tag !== "Preview"), preview],
				active_tab_id: Option.some(preview.id),
				next_tab_generation: state.next_tab_generation + 1,
			},
			file,
		);
	});

export const OpenPreview = (state: WorkspaceState, file: WorkspaceFileReference) =>
	Effect.gen(function* () {
		return yield* OpenPreviewTab(state, file, Option.none());
	});

export const OpenDiffPreview = (
	state: WorkspaceState,
	file: WorkspaceFileReference,
	change_id: string,
) =>
	Effect.gen(function* () {
		return yield* OpenPreviewTab(state, file, Option.some(change_id));
	});

export const PromoteTab = (
	state: WorkspaceState,
	tab_id: string,
	ownership: Exclude<TabOwnership, { readonly _tag: "Preview" }> = { _tag: "Open" },
) =>
	Effect.gen(function* () {
		const tab_index = yield* FindTabIndex(state.tabs, tab_id);
		if (Option.isNone(tab_index))
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;

		const tab = state.tabs.at(tab_index.value);
		if (tab === undefined)
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;

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
		if (Option.isNone(tab_index))
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;

		const tab = state.tabs.at(tab_index.value);
		if (tab === undefined)
			return { _tag: "TabNotFound", state, tab_id } satisfies TabMutationOutcome;
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
		if (Option.isNone(tab_index))
			return { _tag: "TabNotFound", state, tab_id } satisfies CloseTabOutcome;

		const tab = state.tabs.at(tab_index.value);
		if (tab === undefined)
			return { _tag: "TabNotFound", state, tab_id } satisfies CloseTabOutcome;
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
		if (Option.isNone(tab_index))
			return {
				_tag: "TabNotFound",
				state,
				tab_id: confirmation.tab_id,
			} satisfies CloseTabOutcome;

		const tab = state.tabs.at(tab_index.value);
		if (tab === undefined)
			return {
				_tag: "TabNotFound",
				state,
				tab_id: confirmation.tab_id,
			} satisfies CloseTabOutcome;
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
		const badge = { _tag: "AgentChange", ...change } satisfies AgentChangeBadge;
		const tabs = state.tabs.map((tab) =>
			tab.file.id === file.id ? { ...tab, agent_change: Option.some(badge) } : tab,
		);
		const changed_files = [
			{ file, change: badge },
			...state.changed_files.filter((changed_file) => changed_file.file.id !== file.id),
		];
		return { ...state, tabs, changed_files };
	});
