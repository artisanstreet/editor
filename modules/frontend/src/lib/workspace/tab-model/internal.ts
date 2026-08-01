import { Effect, Option } from "effect";

import type {
	AgentChangeBadge,
	TabContent,
	TabOwnership,
	WorkspaceFileReference,
	WorkspaceState,
	WorkspaceTab,
} from "./types";

export const FileTabId = (file_id: string) =>
	Effect.gen(function* () {
		return `file:${file_id}`;
	});

export const DiffTabId = (file_id: string, change_id: string) =>
	Effect.gen(function* () {
		return JSON.stringify(["diff", file_id, change_id]);
	});

export const FindTabIndex = (tabs: ReadonlyArray<WorkspaceTab>, tab_id: string) =>
	Effect.gen(function* () {
		for (let index = 0; index < tabs.length; index += 1) {
			if (tabs[index]?.id === tab_id) return Option.some(index);
		}

		return Option.none<number>();
	});

export const WithRecentFile = (state: WorkspaceState, file: WorkspaceFileReference) =>
	Effect.gen(function* () {
		const recent_files = [
			file,
			...state.recent_files.filter((recent_file) => recent_file.id !== file.id),
		];

		return { ...state, recent_files };
	});

export const MakeFileTab = (
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

export const ReplaceTab = (state: WorkspaceState, tab_index: number, replacement: WorkspaceTab) =>
	Effect.gen(function* () {
		const tabs = [...state.tabs];
		tabs[tab_index] = replacement;
		return { ...state, tabs };
	});

export const RemoveTabAt = (state: WorkspaceState, tab_index: number) =>
	Effect.gen(function* () {
		const tabs = [...state.tabs];
		tabs.splice(tab_index, 1);
		const successor = tabs[Math.min(tab_index, tabs.length - 1)];
		const active_tab_id =
			Option.isSome(state.active_tab_id) &&
			state.active_tab_id.value === state.tabs[tab_index]?.id
				? successor === undefined
					? Option.none()
					: Option.some(successor.id)
				: state.active_tab_id;

		return { ...state, tabs, active_tab_id };
	});
