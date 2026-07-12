import { Effect, Option } from "effect";

import {
	CreateWorkspaceState,
	EditTab,
	OpenDiffPreview,
	OpenPreview,
	PinTab,
	RecordAgentChange,
	UpdateChatView,
	UpdateEditorView,
	UpdateOrchestratorView,
	type TabMutationOutcome,
	type WorkspaceFileReference,
	type WorkspaceState,
} from "$lib/workspace/workspace-tab-model";

export type EditorMode = "editor" | "chat" | "orchestrator";

export type FileFixture = WorkspaceFileReference & {
	readonly lines: ReadonlyArray<{ readonly number: number; readonly code: string }>;
	readonly before_lines?: ReadonlyArray<{ readonly number: number; readonly code: string }>;
};

const typescript_fixture_lines: FileFixture["lines"] = [
	{ number: 1, code: 'import { Effect } from "effect";' },
	{ number: 2, code: "" },
	{ number: 3, code: "export const InspectWorkspace = Effect.gen(function* () {" },
	{ number: 4, code: "  const workspace = yield* WorkspaceService;" },
	{ number: 5, code: "" },
	{ number: 6, code: "  return yield* workspace.inspect;" },
	{ number: 7, code: "});" },
];

export const file_fixtures: ReadonlyArray<FileFixture> = [
	{
		id: "workspace-service",
		name: "workspace-service.ts",
		language: "TypeScript",
		path: "modules/core/src/workspace/workspace-service.ts",
		before_lines: [
			{ number: 1, code: 'import { Effect } from "effect";' },
			{ number: 2, code: "" },
			{ number: 3, code: "export const InspectWorkspace = Effect.void;" },
			{ number: 4, code: "export const ReplaceWorkspace = Effect.void;" },
		],
		lines: typescript_fixture_lines,
	},
	{
		id: "workspace-protocol",
		name: "workspace-protocol.ts",
		language: "TypeScript",
		path: "modules/protocol/src/workspace-protocol.ts",
		lines: typescript_fixture_lines,
	},
	{
		id: "workspace-test",
		name: "workspace-service.test.ts",
		language: "TypeScript",
		path: ".tests/backend/workspace-service.test.ts",
		lines: typescript_fixture_lines,
	},
	{
		id: "workspace-style",
		name: "global.css",
		language: "CSS",
		path: "modules/frontend/src/lib/styles/global.css",
		before_lines: [
			{ number: 1, code: ":root {" },
			{ number: 2, code: "  --canvas: oklch(0.18 0.01 264);" },
			{ number: 3, code: "  --pane: oklch(0.2 0.01 264);" },
			{ number: 4, code: "  --focus: oklch(0.68 0.11 242);" },
			{ number: 5, code: "}" },
		],
		lines: [
			{ number: 1, code: ":root {" },
			{ number: 2, code: "  --canvas: oklch(0.145 0.007 264);" },
			{ number: 3, code: "  --pane: oklch(0.175 0.008 264);" },
			{ number: 4, code: "  --focus: oklch(0.72 0.13 242);" },
			{ number: 5, code: "}" },
		],
	},
	{
		id: "editor-shell",
		name: "editor-shell.sv",
		language: "Svelte",
		path: "modules/frontend/src/routes/components/editor-shell.sv",
		lines: [
			{ number: 1, code: '<script lang="ts" effect>' },
			{ number: 2, code: "  const preferences = yield* ShellPresentationPreferences;" },
			{ number: 3, code: "  const state = yield* preferences.Load;" },
			{ number: 4, code: "</script>" },
		],
	},
	{
		id: "runtime-client",
		name: "artisan-client.ts",
		language: "TypeScript",
		path: "modules/transport/src/artisan-client.ts",
		lines: typescript_fixture_lines,
	},
	{
		id: "changed-only",
		name: "workspace-recovery.ts",
		language: "TypeScript",
		path: "modules/backend/src/workspace/workspace-recovery.ts",
		before_lines: [
			{ number: 1, code: 'import { Effect } from "effect";' },
			{ number: 2, code: "" },
			{ number: 3, code: "export const RecoverWorkspace = Effect.void;" },
		],
		lines: [
			{ number: 1, code: 'import { Effect } from "effect";' },
			{ number: 2, code: "" },
			{ number: 3, code: "export const RecoverWorkspace = Effect.gen(function* () {" },
			{ number: 4, code: "  yield* WorkspaceRecovery.Reconcile;" },
			{ number: 5, code: "});" },
		],
	},
];

export const FileFixtureById = (file_id: string) =>
	Effect.gen(function* () {
		yield* Effect.void;

		for (const file of file_fixtures) {
			if (file.id === file_id) {
				return file;
			}
		}

		return yield* Effect.die(`Unknown workspace fixture file: ${file_id}`);
	});

const UpdatedState = (outcome: TabMutationOutcome) =>
	Effect.gen(function* () {
		if (outcome._tag === "Updated") {
			return outcome.state;
		}

		return yield* Effect.die(`Fixture tab mutation failed: ${outcome._tag}`);
	});

export const CreateWorkspaceFixtureState = (): Effect.Effect<WorkspaceState> =>
	Effect.gen(function* () {
		const [service, protocol, test, style, shell, , changed_only] = file_fixtures;
		let state = yield* CreateWorkspaceState([service!, protocol!, test!]);

		state = yield* UpdatedState(yield* PinTab(state, "file:workspace-protocol"));
		state = yield* UpdatedState(yield* EditTab(state, "file:workspace-test"));
		state = yield* OpenDiffPreview(state, style!, "fixture-change-style-17");
		state = yield* UpdatedState(yield* PinTab(state, state.tabs.at(-1)!.id));
		state = yield* OpenPreview(state, shell!);
		state = yield* RecordAgentChange(state, service!, {
			agent_name: "Terra",
			added: 7,
			removed: 4,
		});
		state = yield* RecordAgentChange(state, changed_only!, {
			agent_name: "Luna",
			added: 5,
			removed: 3,
		});
		state = yield* UpdateEditorView(state, {
			scroll_top: 84,
			cursor_line: 7,
			cursor_column: 31,
		});
		state = yield* UpdateChatView(state, {
			draft: "Can you explain the workspace service boundary?",
			transcript_scroll_top: 52,
		});
		state = yield* UpdateOrchestratorView(state, {
			selected_node_id: Option.some("node-terra"),
			graph_scroll_top: 36,
		});

		return state;
	});

export const thread_fixtures = [
	{ id: "thread-editor", title: "Shape the editor shell", meta: "2 min ago", active: true },
	{ id: "thread-runtime", title: "Review runtime boundaries", meta: "34 min ago", active: false },
	{ id: "thread-branch", title: "Prepare branch summary", meta: "Yesterday", active: false },
	{ id: "thread-provider", title: "Provider capability matrix", meta: "Fri", active: false },
	{ id: "thread-permissions", title: "Permission policy pass", meta: "Thu", active: false },
] as const;

export const session_fixture = {
	id: "ses_01JARTISAN",
	engine: "Codex CLI",
	model: "GPT-5.6",
	context: "41% used",
	status: "Waiting",
} as const;

export const permission_fixtures = [
	{ label: "Workspace files", value: "Read + proposed writes", tone: "permission" },
	{ label: "Network", value: "Ask before access", tone: "muted" },
] as const;

export const change_fixtures = [
	{ path: "modules/frontend/src/routes/+page.svelte", added: 12, removed: 2 },
	{ path: "modules/frontend/src/lib/styles/global.css", added: 28, removed: 0 },
] as const;

export const agent_fixtures = [
	{ name: "Sol", role: "Coordinator", state: "Waiting" },
	{ name: "Terra", role: "Frontend implementation", state: "Working" },
] as const;
