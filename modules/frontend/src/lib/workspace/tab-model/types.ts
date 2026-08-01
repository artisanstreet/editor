import { Option, Schema } from "effect";

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
	| { readonly _tag: "TabNotFound"; readonly state: WorkspaceState; readonly tab_id: string }
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
	| { readonly _tag: "TabNotFound"; readonly state: WorkspaceState; readonly tab_id: string };

export interface TabOverflow {
	readonly visible: ReadonlyArray<WorkspaceTab>;
	readonly overflow: ReadonlyArray<WorkspaceTab>;
}
