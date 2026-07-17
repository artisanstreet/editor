import { Effect, Encoding, FileSystem, Option, Path, Schema } from "effect";
import { Tool } from "effect/unstable/ai";

import {
	TerminalCommandToolResult,
	TerminalIdentifierArguments,
	TerminalListToolResult,
	TerminalReadRecentToolArguments,
	TerminalReadRecentToolResult,
	TerminalStartToolArguments,
	terminal_tool_list_maximum_items,
	terminal_tool_recent_output_maximum_bytes,
	TerminalWriteToolArguments,
	ToolInputSchema,
} from "@artisan/protocol";

import { ProjectRepository } from "../projects/project-repository";
import { RuntimeMetadata } from "../runtime/runtime-metadata";
import { TerminalSessionService } from "../terminal/terminal-sessions";
import { make_effect_tool_adapter } from "./internal/effect-tool-adapter";
import { ToolIneligible, type ToolRegistration } from "./tool-registry";

const default_terminal_cols = 80;
const default_terminal_rows = 24;

function terminal_metadata(terminal: {
	readonly generation: number;
	readonly state: "opening" | "active" | "closed" | "failed";
	readonly terminal_id: string;
}) {
	return {
		generation: terminal.generation,
		state: terminal.state,
		terminal_id: terminal.terminal_id,
	};
}

function path_is_within(path_service: Path.Path, root: string, candidate: string) {
	const relative = path_service.relative(root, candidate);

	return relative === "" || (!relative.startsWith("..") && !path_service.isAbsolute(relative));
}

function canonical_path_key(path_service: Path.Path, value: string) {
	const normalized = path_service.normalize(value);

	return path_service.sep === "\\" ? normalized.toLowerCase() : normalized;
}

function same_canonical_path(path_service: Path.Path, left: string, right: string) {
	return canonical_path_key(path_service, left) === canonical_path_key(path_service, right);
}

function eligibility_error(error: unknown) {
	return error instanceof ToolIneligible
		? error
		: new ToolIneligible({ reason_code: "workspace.unavailable" });
}

/** Registers the canonical terminal control tools through the durable tool plane. */
export const TerminalTools = Effect.gen(function* () {
	const file_system = yield* FileSystem.FileSystem;
	const metadata = yield* RuntimeMetadata;
	const path_service = yield* Path.Path;
	const projects = yield* ProjectRepository;
	const terminals = yield* TerminalSessionService;
	const RequireProject = (workspace_id: string) =>
		projects.FindByWorkspaceId({ workspace_id }).pipe(
			Effect.flatMap(
				Option.match({
					onNone: () =>
						Effect.fail(new ToolIneligible({ reason_code: "workspace.unavailable" })),
					onSome: Effect.succeed,
				}),
			),
		);
	const RequireWorkspace = (context: { readonly workspace_id?: string | undefined }) =>
		context.workspace_id === undefined
			? Effect.fail(new ToolIneligible({ reason_code: "workspace.required" }))
			: RequireProject(context.workspace_id);
	const ResolveWorkingDirectory = (root_path: string, cwd: string | undefined) =>
		Effect.gen(function* () {
			const requested_root = path_service.resolve(root_path);
			const root = yield* file_system.realPath(root_path);

			if (!same_canonical_path(path_service, requested_root, root)) {
				return yield* Effect.fail(
					new ToolIneligible({ reason_code: "workspace.unavailable" }),
				);
			}

			const target =
				cwd === undefined
					? root
					: yield* file_system.realPath(path_service.resolve(root, cwd));
			const info = yield* file_system.stat(target);

			if (info.type !== "Directory" || !path_is_within(path_service, root, target)) {
				return yield* Effect.fail(
					new ToolIneligible({ reason_code: "workspace.unavailable" }),
				);
			}

			return target;
		});
	const HandleCommand = (
		invocation: Parameters<ToolRegistration["adapter"]["Invoke"]>[0],
		workspace_id: string,
		payload: Parameters<typeof terminals.HandleCanonical>[0]["payload"],
	) =>
		Effect.gen(function* () {
			const sent_at = yield* metadata.Now;
			const acceptance = yield* terminals.HandleCanonical(
				{
					agent_id: invocation.context.agent_id,
					kind: "command",
					message_id: invocation.invocation_id,
					origin: "frontend",
					payload,
					protocol_version: 1,
					run_id: invocation.context.run_id,
					schema_version: 1,
					sent_at,
					thread_id: invocation.context.thread_id,
				},
				workspace_id,
			);

			return { status: acceptance.status, terminal: terminal_metadata(acceptance.terminal) };
		});
	const start_adapter = make_effect_tool_adapter({
		handler: (invocation, arguments_) =>
			Effect.gen(function* () {
				const project = yield* RequireWorkspace(invocation.context);
				const workspace_id = invocation.context.workspace_id!;
				const working_directory = yield* ResolveWorkingDirectory(
					project.project.root_path,
					arguments_.cwd,
				);

				return yield* HandleCommand(invocation, workspace_id, {
					args: arguments_.args,
					cols: arguments_.cols ?? default_terminal_cols,
					executable: arguments_.executable,
					rows: arguments_.rows ?? default_terminal_rows,
					terminal_id: `terminal_${invocation.invocation_id}`,
					type: "terminal.open",
					working_directory,
					workspace_id,
				});
			}),
		parameters: TerminalStartToolArguments,
		success: TerminalCommandToolResult,
	});
	const list_adapter = make_effect_tool_adapter({
		handler: (invocation) =>
			RequireWorkspace(invocation.context).pipe(
				Effect.flatMap(() =>
					terminals.List(invocation.context.thread_id, invocation.context.workspace_id!),
				),
				Effect.map((terminal_list) => ({
					terminals: terminal_list
						.slice(0, terminal_tool_list_maximum_items)
						.map(terminal_metadata),
					truncated: terminal_list.length > terminal_tool_list_maximum_items,
				})),
			),
		parameters: Tool.EmptyParams,
		success: TerminalListToolResult,
	});
	const recent_adapter = make_effect_tool_adapter({
		handler: (invocation, arguments_) =>
			RequireWorkspace(invocation.context).pipe(
				Effect.flatMap(() =>
					terminals.RecentOutput(
						arguments_.terminal_id,
						invocation.context.thread_id,
						invocation.context.workspace_id!,
						arguments_.max_bytes ?? terminal_tool_recent_output_maximum_bytes,
					),
				),
				Effect.map((recent) => ({
					data: Encoding.encodeBase64(recent.output),
					encoding: "base64" as const,
					state: recent.state,
					terminal: terminal_metadata(recent.terminal),
					truncated: recent.truncated,
				})),
			),
		parameters: TerminalReadRecentToolArguments,
		success: TerminalReadRecentToolResult,
	});
	const mutation_adapter = (type: "terminal.close" | "terminal.restart" | "terminal.write") =>
		make_effect_tool_adapter({
			handler: (
				invocation,
				arguments_:
					| typeof TerminalIdentifierArguments.Type
					| typeof TerminalWriteToolArguments.Type,
			) =>
				Effect.gen(function* () {
					yield* RequireWorkspace(invocation.context);
					const workspace_id = invocation.context.workspace_id!;
					const payload =
						type === "terminal.write"
							? {
									data: (arguments_ as typeof TerminalWriteToolArguments.Type)
										.data,
									terminal_id: arguments_.terminal_id,
									type,
									workspace_id,
								}
							: { terminal_id: arguments_.terminal_id, type, workspace_id };

					return yield* HandleCommand(invocation, workspace_id, payload);
				}),
			parameters:
				type === "terminal.write"
					? TerminalWriteToolArguments
					: TerminalIdentifierArguments,
			success: TerminalCommandToolResult,
		});
	const registration = (
		adapter: ToolRegistration["adapter"],
		tool_id: string,
		label: string,
		summary: string,
		effect: ToolRegistration["descriptor"]["effect"],
		approval_policy: ToolRegistration["descriptor"]["approval_policy"],
		recovery_policy: ToolRegistration["recovery_policy"],
	) =>
		({
			adapter,
			descriptor: {
				approval_policy,
				effect,
				input_schema: Schema.decodeUnknownSync(ToolInputSchema)(adapter.input_schema),
				label,
				revision: 1,
				source: "artisan",
				summary,
				tool_id,
			},
			IsEligible: (context: Parameters<ToolRegistration["IsEligible"]>[0]) =>
				RequireWorkspace(context).pipe(Effect.asVoid, Effect.mapError(eligibility_error)),
			recovery_policy,
		}) satisfies ToolRegistration;

	return [
		registration(
			start_adapter,
			"terminal.start",
			"Start terminal",
			"Start a terminal in the workspace.",
			"workspace_mutation",
			"required",
			"outcome_unknown",
		),
		registration(
			list_adapter,
			"terminal.list",
			"List terminals",
			"List terminals in the current thread.",
			"read",
			"automatic",
			"retry",
		),
		registration(
			recent_adapter,
			"terminal.read_recent",
			"Recent terminal output",
			"Read recent terminal output.",
			"read",
			"automatic",
			"retry",
		),
		registration(
			mutation_adapter("terminal.write"),
			"terminal.write",
			"Write terminal",
			"Write text to a terminal.",
			"workspace_mutation",
			"required",
			"outcome_unknown",
		),
		registration(
			mutation_adapter("terminal.restart"),
			"terminal.restart",
			"Restart terminal",
			"Restart a terminal.",
			"workspace_mutation",
			"required",
			"outcome_unknown",
		),
		registration(
			mutation_adapter("terminal.close"),
			"terminal.stop",
			"Stop terminal",
			"Stop a terminal.",
			"workspace_mutation",
			"required",
			"outcome_unknown",
		),
	] as const;
});
