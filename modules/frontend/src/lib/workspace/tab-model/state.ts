import { Effect, Option } from "effect";

import { MakeFileTab } from "./internal";
import type {
	ChatViewState,
	EditorViewState,
	OrchestratorViewState,
	WorkspaceFileReference,
	WorkspaceMode,
	WorkspaceState,
	WorkspaceTab,
} from "./types";

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
		return { ...state, mode };
	});

export const UpdateEditorView = (state: WorkspaceState, editor: EditorViewState) =>
	Effect.gen(function* () {
		return { ...state, editor };
	});

export const UpdateChatView = (state: WorkspaceState, chat: ChatViewState) =>
	Effect.gen(function* () {
		return { ...state, chat };
	});

export const UpdateOrchestratorView = (
	state: WorkspaceState,
	orchestrator: OrchestratorViewState,
) =>
	Effect.gen(function* () {
		return { ...state, orchestrator };
	});
