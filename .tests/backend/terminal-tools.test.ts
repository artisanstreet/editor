import { mkdir, mkdtemp, realpath, rename, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { NodeFileSystem, NodePath } from "@effect/platform-node-shared";
import { Effect, Layer, Option, Schema } from "effect";
import { afterEach, describe, expect, it } from "vitest";

import {
	type TerminalSession,
	TerminalListToolResult,
	terminal_tool_list_maximum_items,
	terminal_tool_recent_output_maximum_bytes,
} from "@artisan/protocol";

import { ProjectRepository } from "../../modules/backend/src/projects/project-repository";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/runtime-metadata";
import { TerminalSessionService } from "../../modules/backend/src/terminal/terminal-sessions";
import { TerminalTools } from "../../modules/backend/src/tool-control/terminal-tools";
import {
	ToolRegistry,
	make_tool_registry_layer,
} from "../../modules/backend/src/tool-control/tool-registry";

const directories: Array<string> = [];
const context = {
	agent_id: "agent",
	run_id: "run",
	thread_id: "thread",
	workspace_id: "workspace_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
};

interface FakeTerminalCalls {
	readonly commands: Array<{ readonly command: unknown; readonly workspace_id: string }>;
	readonly lists: Array<{ readonly thread_id: string; readonly workspace_id: string }>;
	readonly recent: Array<{
		readonly max_bytes: number;
		readonly terminal_id: string;
		readonly thread_id: string;
		readonly workspace_id: string;
	}>;
}

interface RegistryOptions {
	readonly available?: boolean;
	readonly duplicate_invocations?: ReadonlySet<string>;
	readonly list_count?: number;
}

function terminal_session(
	terminal_id: string,
	generation = 1,
	state: TerminalSession["state"] = "active",
): TerminalSession {
	return {
		args: [],
		cols: 80,
		created_at: "2026-07-17T12:00:00.000Z",
		executable: "shell",
		generation,
		rows: 24,
		state,
		terminal_id,
		thread_id: context.thread_id,
		updated_at: "2026-07-17T12:00:00.000Z",
		workspace_id: context.workspace_id,
		working_directory: "root",
	};
}

function tool_invocation(
	tool_id: string,
	invocation_id: string,
	invocation_context: Omit<typeof context, "workspace_id"> & {
		readonly workspace_id?: string;
	} = context,
) {
	return {
		context: invocation_context,
		invocation_id,
		tool: { revision: 1, tool_id },
	};
}

afterEach(async () => {
	await Promise.all(
		directories.splice(0).map((directory) => rm(directory, { force: true, recursive: true })),
	);
});

function registry(root_path: string, calls: FakeTerminalCalls, options: RegistryOptions = {}) {
	const available = options.available ?? true;
	const projects = Layer.succeed(ProjectRepository, {
		FindByHostedIdentity: () => Effect.die("unused"),
		FindByProjectId: () => Effect.die("unused"),
		FindByRoot: () => Effect.die("unused"),
		FindByWorkspaceId: () =>
			Effect.succeed(
				available
					? Option.some({
							project: {
								display_name: "Project",
								project_id:
									"project_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
								root_path,
							},
							workspace_id: context.workspace_id,
							hosted_origin: {
								canonical_host: "github.com",
								clone_url: "https://github.com/a/b.git",
								fetch_url: "https://github.com/a/b.git",
								name: "b",
								native_id: "1",
								owner: "a",
								provider_id: "github",
								push_url: "https://github.com/a/b.git",
								remote_name: "origin",
								selected_account_login: "a",
								web_url: "https://github.com/a/b",
							},
							registered_at: "2026-07-17T12:00:00.000Z",
							updated_at: "2026-07-17T12:00:00.000Z",
						})
					: Option.none(),
			),
		List: Effect.die("unused"),
		RegisterHosted: () => Effect.die("unused"),
	} as typeof ProjectRepository.Service);
	const terminals = Layer.succeed(TerminalSessionService, {
		Handle: () => Effect.die("unused"),
		HandleCanonical: (command, workspace_id) =>
			Effect.sync(() => calls.commands.push({ command, workspace_id })).pipe(
				Effect.as({
					events: [],
					journal_sequence: 1,
					status: options.duplicate_invocations?.has(command.message_id)
						? ("duplicate" as const)
						: ("accepted" as const),
					terminal: terminal_session(
						command.payload.terminal_id,
						command.payload.type === "terminal.restart" ? 2 : 1,
					),
				}),
			),
		List: (thread_id, workspace_id) =>
			Effect.sync(() => calls.lists.push({ thread_id, workspace_id })).pipe(
				Effect.as(
					Array.from({ length: options.list_count ?? 1 }, (_, index) =>
						terminal_session(`terminal_listed_${index + 1}`),
					),
				),
			),
		Output: () => Effect.die("unused"),
		RecentOutput: (terminal_id, thread_id, workspace_id, max_bytes) =>
			Effect.sync(() =>
				calls.recent.push({ max_bytes, terminal_id, thread_id, workspace_id }),
			).pipe(
				Effect.as(
					terminal_id === "terminal_restarted"
						? {
								output: new Uint8Array(),
								state: "unavailable_after_restart" as const,
								terminal: terminal_session(terminal_id, 2),
								truncated: false,
							}
						: {
								output: Uint8Array.from([0, 255, 1]),
								state: "available" as const,
								terminal: terminal_session(terminal_id),
								truncated: true,
							},
				),
			),
		QuiesceThread: () => Effect.void,
	} satisfies typeof TerminalSessionService.Service);
	const metadata = Layer.succeed(RuntimeMetadata, {
		instance_id: "backend",
		MakeId: () => Effect.succeed("unused"),
		Now: Effect.succeed("2026-07-17T12:00:00.000Z"),
	});

	return TerminalTools.pipe(
		Effect.provide(
			Layer.mergeAll(projects, terminals, metadata, NodeFileSystem.layer, NodePath.layer),
		),
		Effect.map((tools) => make_tool_registry_layer(tools)),
		Effect.flatMap((layer) => Effect.service(ToolRegistry).pipe(Effect.provide(layer))),
	);
}

function make_calls(): FakeTerminalCalls {
	return { commands: [], lists: [], recent: [] };
}

describe("TerminalTools", () => {
	it("preserves workspace discovery reasons across all six registrations", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-terminal-tools-"));
		const calls = make_calls();

		directories.push(root);
		const service = await Effect.runPromise(registry(root, calls));
		const eligible = await Effect.runPromise(service.List(context));
		const required = await Effect.runPromise(
			service.List({ agent_id: "agent", run_id: "run", thread_id: "thread" }),
		);
		const unavailable_service = await Effect.runPromise(
			registry(root, calls, { available: false }),
		);
		const unavailable = await Effect.runPromise(unavailable_service.List(context));
		const terminal_tools = eligible.tools.filter(({ descriptor }) =>
			descriptor.tool_id.startsWith("terminal."),
		);

		expect(terminal_tools).toHaveLength(6);
		expect(terminal_tools).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					descriptor: expect.objectContaining({
						approval_policy: "automatic",
						effect: "read",
						tool_id: "terminal.list",
					}),
					state: "eligible",
				}),
				expect.objectContaining({
					descriptor: expect.objectContaining({
						approval_policy: "required",
						effect: "workspace_mutation",
						tool_id: "terminal.start",
					}),
					state: "eligible",
				}),
			]),
		);
		expect(required.tools).toHaveLength(6);
		expect(required.tools).toEqual(
			required.tools.map((tool) => ({
				...tool,
				reason_code: "workspace.required",
				state: "unavailable",
			})),
		);
		expect(unavailable.tools).toHaveLength(6);
		expect(unavailable.tools).toEqual(
			unavailable.tools.map((tool) => ({
				...tool,
				reason_code: "workspace.unavailable",
				state: "unavailable",
			})),
		);
	});

	it("executes all six tools with bounded JSON results and exact canonical attribution", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-terminal-tools-"));
		const canonical_root = await realpath(root);
		const calls = make_calls();
		const service = await Effect.runPromise(
			registry(root, calls, { duplicate_invocations: new Set(["write_duplicate"]) }),
		);

		directories.push(root);
		const start = await Effect.runPromise(
			service.Invoke(tool_invocation("terminal.start", "start_invocation"), {
				args: ["--interactive"],
				executable: "shell",
			}),
		);
		const listed = await Effect.runPromise(
			service.Invoke(tool_invocation("terminal.list", "list_invocation"), {}),
		);
		const recent = await Effect.runPromise(
			service.Invoke(tool_invocation("terminal.read_recent", "recent_invocation"), {
				max_bytes: 3,
				terminal_id: "terminal_owned",
			}),
		);
		const restarted_recent = await Effect.runPromise(
			service.Invoke(tool_invocation("terminal.read_recent", "restart_read"), {
				terminal_id: "terminal_restarted",
			}),
		);
		const write = await Effect.runPromise(
			service.Invoke(tool_invocation("terminal.write", "write_invocation"), {
				data: "hello\n",
				terminal_id: "terminal_owned",
			}),
		);
		const restart = await Effect.runPromise(
			service.Invoke(tool_invocation("terminal.restart", "restart_invocation"), {
				terminal_id: "terminal_owned",
			}),
		);
		const stop = await Effect.runPromise(
			service.Invoke(tool_invocation("terminal.stop", "stop_invocation"), {
				terminal_id: "terminal_owned",
			}),
		);
		const duplicate = await Effect.runPromise(
			service.Invoke(tool_invocation("terminal.write", "write_duplicate"), {
				data: "same",
				terminal_id: "terminal_owned",
			}),
		);
		const excessive_recent = await Effect.runPromise(
			service
				.Invoke(tool_invocation("terminal.read_recent", "recent_excess"), {
					max_bytes: terminal_tool_recent_output_maximum_bytes + 1,
					terminal_id: "terminal_owned",
				})
				.pipe(Effect.flip),
		);

		expect(start).toEqual({
			status: "accepted",
			terminal: { generation: 1, state: "active", terminal_id: "terminal_start_invocation" },
		});
		expect(listed).toEqual({
			terminals: [{ generation: 1, state: "active", terminal_id: "terminal_listed_1" }],
			truncated: false,
		});
		expect(recent).toEqual({
			data: "AP8B",
			encoding: "base64",
			state: "available",
			terminal: { generation: 1, state: "active", terminal_id: "terminal_owned" },
			truncated: true,
		});
		expect(restarted_recent).toEqual({
			data: "",
			encoding: "base64",
			state: "unavailable_after_restart",
			terminal: { generation: 2, state: "active", terminal_id: "terminal_restarted" },
			truncated: false,
		});
		expect(write).toMatchObject({
			status: "accepted",
			terminal: { terminal_id: "terminal_owned" },
		});
		expect(restart).toEqual({
			status: "accepted",
			terminal: { generation: 2, state: "active", terminal_id: "terminal_owned" },
		});
		expect(stop).toMatchObject({
			status: "accepted",
			terminal: { terminal_id: "terminal_owned" },
		});
		expect(duplicate).toEqual({
			status: "duplicate",
			terminal: { generation: 1, state: "active", terminal_id: "terminal_owned" },
		});
		expect(excessive_recent.reason_code).toBe("invalid_arguments");
		expect(calls.lists).toEqual([
			{ thread_id: context.thread_id, workspace_id: context.workspace_id },
		]);
		expect(calls.recent).toEqual([
			{
				max_bytes: 3,
				terminal_id: "terminal_owned",
				thread_id: context.thread_id,
				workspace_id: context.workspace_id,
			},
			{
				max_bytes: terminal_tool_recent_output_maximum_bytes,
				terminal_id: "terminal_restarted",
				thread_id: context.thread_id,
				workspace_id: context.workspace_id,
			},
		]);
		expect(calls.commands).toEqual([
			{
				command: {
					agent_id: context.agent_id,
					kind: "command",
					message_id: "start_invocation",
					origin: "frontend",
					payload: {
						args: ["--interactive"],
						cols: 80,
						executable: "shell",
						rows: 24,
						terminal_id: "terminal_start_invocation",
						type: "terminal.open",
						working_directory: canonical_root,
						workspace_id: context.workspace_id,
					},
					protocol_version: 1,
					run_id: context.run_id,
					schema_version: 1,
					sent_at: "2026-07-17T12:00:00.000Z",
					thread_id: context.thread_id,
				},
				workspace_id: context.workspace_id,
			},
			...[
				["write_invocation", "terminal.write", { data: "hello\n" }],
				["restart_invocation", "terminal.restart", {}],
				["stop_invocation", "terminal.close", {}],
				["write_duplicate", "terminal.write", { data: "same" }],
			].map(([message_id, type, extra]) => ({
				command: {
					agent_id: context.agent_id,
					kind: "command",
					message_id,
					origin: "frontend",
					payload: {
						...(extra as object),
						terminal_id: "terminal_owned",
						type,
						workspace_id: context.workspace_id,
					},
					protocol_version: 1,
					run_id: context.run_id,
					schema_version: 1,
					sent_at: "2026-07-17T12:00:00.000Z",
					thread_id: context.thread_id,
				},
				workspace_id: context.workspace_id,
			})),
		]);
	});

	it("returns a truthful bounded terminal list instead of failing result validation", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-terminal-tools-"));
		const calls = make_calls();
		const service = await Effect.runPromise(
			registry(root, calls, { list_count: terminal_tool_list_maximum_items + 1 }),
		);

		directories.push(root);
		const listed = Schema.decodeUnknownSync(TerminalListToolResult)(
			await Effect.runPromise(
				service.Invoke(tool_invocation("terminal.list", "bounded_list"), {}),
			),
		);

		expect(listed.terminals).toHaveLength(terminal_tool_list_maximum_items);
		expect(listed.terminals.at(-1)?.terminal_id).toBe(
			`terminal_listed_${terminal_tool_list_maximum_items}`,
		);
		expect(listed.truncated).toBe(true);
	});

	it("rejects cwd symlink escapes and registered-root junction substitution", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-terminal-tools-"));
		const outside = await mkdtemp(join(tmpdir(), "artisan-terminal-outside-"));
		const parent = await mkdtemp(join(tmpdir(), "artisan-terminal-substitution-"));
		const registered_root = join(parent, "registered");
		const displaced_root = join(parent, "displaced");
		const calls = make_calls();

		directories.push(root, outside, parent);
		await mkdir(join(root, "inside"));
		await symlink(outside, join(root, "escape"), "junction");
		await mkdir(registered_root);
		await rename(registered_root, displaced_root);
		await symlink(outside, registered_root, "junction");
		const service = await Effect.runPromise(registry(root, calls));
		const escaped = await Effect.runPromise(
			service
				.Invoke(tool_invocation("terminal.start", "escape"), {
					args: [],
					cwd: "escape",
					executable: "shell",
				})
				.pipe(Effect.flip),
		);
		const substituted = await Effect.runPromise(registry(registered_root, calls));
		const replaced_root = await Effect.runPromise(
			substituted
				.Invoke(tool_invocation("terminal.start", "substituted"), {
					args: [],
					executable: "shell",
				})
				.pipe(Effect.flip),
		);

		expect(escaped.reason_code).toBe("execution_failed");
		expect(replaced_root.reason_code).toBe("execution_failed");
		expect(calls.commands).toEqual([]);
	});

	it("rejects strict start schemas before terminal delegation", async () => {
		const root = await mkdtemp(join(tmpdir(), "artisan-terminal-tools-"));
		const calls = make_calls();
		const service = await Effect.runPromise(registry(root, calls));

		directories.push(root);
		const failures = await Effect.runPromise(
			Effect.forEach(
				[
					{ args: ["\u0000"], executable: "shell" },
					{ args: [], executable: "shell", env: {} },
					{ args: [], cwd: "../escape", executable: "shell" },
				],
				(arguments_) =>
					service
						.Invoke(tool_invocation("terminal.start", "invalid"), arguments_)
						.pipe(Effect.flip),
			),
		);

		expect(failures.map(({ reason_code }) => reason_code)).toEqual([
			"invalid_arguments",
			"invalid_arguments",
			"invalid_arguments",
		]);
		expect(calls.commands).toEqual([]);
	});
});
