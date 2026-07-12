export type EditorMode = "editor" | "chat" | "orchestrator";

export type FileFixture = {
	readonly id: string;
	readonly name: string;
	readonly language: string;
	readonly path: string;
	readonly lines: ReadonlyArray<{ readonly number: number; readonly code: string }>;
};

export const thread_fixtures = [
	{ id: "thread-editor", title: "Shape the editor shell", meta: "2 min ago", active: true },
	{ id: "thread-runtime", title: "Review runtime boundaries", meta: "34 min ago", active: false },
	{ id: "thread-branch", title: "Prepare branch summary", meta: "Yesterday", active: false },
	{ id: "thread-provider", title: "Provider capability matrix", meta: "Fri", active: false },
	{ id: "thread-permissions", title: "Permission policy pass", meta: "Thu", active: false },
] as const;

export const file_fixtures: ReadonlyArray<FileFixture> = [
	{
		id: "workspace-service",
		name: "workspace-service.ts",
		language: "TypeScript",
		path: "modules/core/src/workspace/workspace-service.ts",
		lines: [
			{ number: 1, code: 'import { Context, Effect, Layer } from "effect";' },
			{ number: 2, code: "" },
			{ number: 3, code: "export class WorkspaceService extends Context.Service(" },
			{ number: 4, code: '  "@artisan/WorkspaceService",' },
			{ number: 5, code: "  {" },
			{ number: 6, code: "    effect: Effect.gen(function* () {" },
			{ number: 7, code: "      const workspace = yield* WorkspaceRepository;" },
			{ number: 8, code: "" },
			{ number: 9, code: "      const inspect = Effect.gen(function* () {" },
			{ number: 10, code: "        return yield* workspace.current;" },
			{ number: 11, code: "      });" },
			{ number: 12, code: "" },
			{ number: 13, code: "      return { inspect } as const;" },
			{ number: 14, code: "    })," },
			{ number: 15, code: "  }," },
			{ number: 16, code: ") {}" },
		],
	},
	{
		id: "workspace-protocol",
		name: "workspace-protocol.ts",
		language: "TypeScript",
		path: "modules/protocol/src/workspace-protocol.ts",
		lines: [
			{ number: 1, code: 'import { Schema } from "effect";' },
			{ number: 2, code: "" },
			{ number: 3, code: "export const WorkspaceId = Schema.String.pipe(" },
			{ number: 4, code: '  Schema.brand("WorkspaceId"),' },
			{ number: 5, code: ");" },
			{ number: 6, code: "" },
			{ number: 7, code: "export const WorkspaceState = Schema.Struct({" },
			{ number: 8, code: "  id: WorkspaceId," },
			{ number: 9, code: "  branch: Schema.String," },
			{ number: 10, code: "  dirty: Schema.Boolean," },
			{ number: 11, code: "});" },
		],
	},
	{
		id: "workspace-test",
		name: "workspace-service.test.ts",
		language: "TypeScript",
		path: "modules/core/.tests/workspace-service.test.ts",
		lines: [
			{ number: 1, code: 'import { Effect, Layer } from "effect";' },
			{ number: 2, code: 'import { describe, expect, it } from "vitest";' },
			{ number: 3, code: "" },
			{ number: 4, code: 'describe("WorkspaceService", () => {' },
			{ number: 5, code: '  it.effect("reads the selected workspace", () =>' },
			{ number: 6, code: "    Effect.gen(function* () {" },
			{ number: 7, code: "      const service = yield* WorkspaceService;" },
			{ number: 8, code: "      expect(yield* service.inspect).toBeDefined();" },
			{ number: 9, code: "    })," },
			{ number: 10, code: "  );" },
			{ number: 11, code: "});" },
		],
	},
];

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
