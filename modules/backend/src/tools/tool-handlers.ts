import { Context, Effect, Layer } from "effect";

import type {
	ArtisanToolExecutionInput,
	ArtisanToolInvocationOutcome,
	RawOrigin,
} from "@artisan/protocol";

import { GitService } from "../git/service";
import { JournalStore } from "../persistence/journal-store";
import { RuntimeMetadata } from "../runtime/metadata";
import { TerminalSessionService } from "../terminal/sessions";
import { WorkspaceEvidenceRecorder } from "../workspace/evidence";
import { WorkspaceFileDiscovery } from "../workspace/files/discovery";
import { WorkspaceFileService } from "../workspace/files/service";
import { WorkspaceFilesystemRegistry } from "../filesystem/workspace-filesystem-registry";

/** Dispatches a claimed invocation to a direct, backend-owned V1 capability. */
export class ExecuteTool extends Context.Service<
	ExecuteTool,
	{
		readonly Execute: (input: {
			readonly agent_id?: string | undefined;
			readonly input: typeof ArtisanToolExecutionInput.Type;
			readonly invocation_id: string;
			readonly raw_origin?: typeof RawOrigin.Type | undefined;
			readonly run_id?: string | undefined;
			readonly thread_id: string;
		}) => Effect.Effect<typeof ArtisanToolInvocationOutcome.Type>;
	}
>()("Artisan/ExecuteTool") {}

const Success = (code: string) => ({ code, status: "succeeded" as const });
const Unsupported = (code: string, detail: string) => ({
	code,
	detail,
	status: "unsupported" as const,
});
const Failed = () => ({ code: "tool_failed", status: "failed" as const });

/** Adapts direct backend services while preserving their established evidence ownership. */
export const ExecuteToolLive = Layer.effect(
	ExecuteTool,
	Effect.gen(function* () {
		const discovery = yield* WorkspaceFileDiscovery;
		const evidence = yield* WorkspaceEvidenceRecorder;
		const files = yield* WorkspaceFileService;
		const git = yield* GitService;
		const journal = yield* JournalStore;
		const metadata = yield* RuntimeMetadata;
		const terminals = yield* TerminalSessionService;
		const workspace_filesystems = yield* WorkspaceFilesystemRegistry;
		const Execute = (request: {
			readonly agent_id?: string | undefined;
			readonly input: typeof ArtisanToolExecutionInput.Type;
			readonly invocation_id: string;
			readonly raw_origin?: typeof RawOrigin.Type | undefined;
			readonly run_id?: string | undefined;
			readonly thread_id: string;
		}) => {
			const input = request.input;
			const trace = {
				...(request.agent_id === undefined ? {} : { agent_id: request.agent_id }),
				operation_id: request.invocation_id,
				...(request.raw_origin === undefined ? {} : { raw_origin: request.raw_origin }),
				...(request.run_id === undefined ? {} : { run_id: request.run_id }),
				thread_id: request.thread_id,
			};
			switch (input.tool_id) {
				case "question.ask":
					return journal
						.AppendEvent({
							...trace,
							causation_id: request.invocation_id,
							correlation_id: request.invocation_id,
							idempotency_key: `${request.invocation_id}:question.ask`,
							payload: {
								question_id: input.question_id,
								state: "requested",
								text: input.text,
								type: "interaction.question",
							},
						})
						.pipe(
							Effect.as(Success("question_asked")),
							Effect.catch(() => Effect.succeed(Failed())),
						);
				case "assumption.record":
					return journal
						.AppendEvent({
							...trace,
							causation_id: request.invocation_id,
							correlation_id: request.invocation_id,
							idempotency_key: `${request.invocation_id}:assumption.record`,
							payload: {
								assumption_id: input.assumption_id,
								invocation_id: request.invocation_id,
								...(request.raw_origin === undefined
									? {}
									: { raw_origin: request.raw_origin }),
								...(input.reason === undefined ? {} : { reason: input.reason }),
								statement: input.statement,
								type: "artisan.assumption.recorded",
							},
						})
						.pipe(
							Effect.as(Success("assumption_recorded")),
							Effect.catch(() => Effect.succeed(Failed())),
						);
				case "engine.native_action.record":
					return journal
						.AppendEvent({
							...trace,
							causation_id: request.invocation_id,
							correlation_id: request.invocation_id,
							idempotency_key: `${request.invocation_id}:engine.native_action.record`,
							payload: {
								action: input.action,
								...(input.detail === undefined ? {} : { detail: input.detail }),
								invocation_id: request.invocation_id,
								...(request.raw_origin === undefined
									? {}
									: { raw_origin: request.raw_origin }),
								tool_id: input.tool_id,
								type: "engine.native_action",
							},
						})
						.pipe(
							Effect.as(Success("native_action_recorded")),
							Effect.catch(() => Effect.succeed(Failed())),
						);
				case "approval.request":
					return Effect.succeed(Success("approval_requested"));
				case "workspace.file.read":
					return files.Read({ path: input.path, workspace_id: input.workspace_id }).pipe(
						Effect.as(Success("workspace_file_read")),
						Effect.catch(() => Effect.succeed(Failed())),
					);
				case "workspace.file.list":
					return discovery
						.Discover({
							...(input.after_path === undefined
								? {}
								: { after_path: input.after_path }),
							...(input.limit === undefined ? {} : { limit: input.limit }),
							...(input.prefix === undefined ? {} : { prefix: input.prefix }),
							workspace_id: input.workspace_id,
						})
						.pipe(
							Effect.as(Success("workspace_file_list")),
							Effect.catch(() => Effect.succeed(Failed())),
						);
				case "workspace.language.status":
					return discovery
						.LanguageCapabilities({ workspace_id: input.workspace_id })
						.pipe(
							Effect.as(
								Unsupported(
									"language_capabilities_unavailable",
									"No backend language service is configured.",
								),
							),
							Effect.catch(() => Effect.succeed(Failed())),
						);
				case "workspace.file.write":
					return Effect.gen(function* () {
						const sent_at = yield* metadata.Now;
						yield* files.Replace({
							change_id: input.change_id,
							content: input.content,
							expected_before: input.expected_before,
							path: input.path,
							workspace_id: input.workspace_id,
							agent_id: request.agent_id ?? "artisan",
							message_id: request.invocation_id,
							...(request.raw_origin === undefined
								? {}
								: { raw_origin: request.raw_origin }),
							run_id: request.run_id ?? "artisan",
							sent_at,
							thread_id: request.thread_id,
						});
						return Success("workspace_file_write");
					}).pipe(Effect.catch(() => Effect.succeed(Failed())));
				case "git.status.read":
					return Effect.gen(function* () {
						const sent_at = yield* metadata.Now;
						yield* git.Query({
							kind: "git.workspace.query",
							message_id: request.invocation_id,
							origin: "frontend",
							payload: {
								thread_id: input.thread_id,
								workspace_id: input.workspace_id,
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at,
						});
						return Success("git_status_read");
					}).pipe(Effect.catch(() => Effect.succeed(Failed())));
				case "git.diff.read":
					return Effect.gen(function* () {
						const sent_at = yield* metadata.Now;
						yield* git.Diff({
							kind: "git.diff.query",
							message_id: request.invocation_id,
							origin: "frontend",
							payload: {
								expected_snapshot_id: input.expected_snapshot_id,
								expected_workspace_version: input.expected_workspace_version,
								...(input.max_bytes === undefined
									? {}
									: { max_bytes: input.max_bytes }),
								scope: input.scope,
								workspace_id: input.workspace_id,
							},
							protocol_version: 1,
							schema_version: 1,
							sent_at,
						});
						return Success("git_diff_read");
					}).pipe(Effect.catch(() => Effect.succeed(Failed())));
				case "git.index.stage":
				case "git.index.unstage":
					return Effect.gen(function* () {
						const sent_at = yield* metadata.Now;
						yield* git.Request({
							...(request.agent_id === undefined
								? {}
								: { agent_id: request.agent_id }),
							kind:
								input.tool_id === "git.index.stage"
									? "git.index.stage.request"
									: "git.index.unstage.request",
							message_id: request.invocation_id,
							origin: "frontend",
							payload: {
								approval_id: input.approval_id,
								expected_snapshot_id: input.expected_snapshot_id,
								expected_workspace_version: input.expected_workspace_version,
								mutation_id: input.mutation_id,
								paths: input.paths,
								workspace_id: input.workspace_id,
							},
							protocol_version: 1,
							...(request.raw_origin === undefined
								? {}
								: { raw_origin: request.raw_origin }),
							...(request.run_id === undefined ? {} : { run_id: request.run_id }),
							schema_version: 1,
							sent_at,
							thread_id: request.thread_id,
						});
						const resolved_at = yield* metadata.Now;
						yield* git.Resolve({
							...(request.agent_id === undefined
								? {}
								: { agent_id: request.agent_id }),
							kind: "git.mutation.resolve",
							message_id: `${request.invocation_id}:resolve`,
							origin: "frontend",
							payload: {
								approval_id: input.approval_id,
								approved: true,
								mutation_id: input.mutation_id,
							},
							protocol_version: 1,
							...(request.raw_origin === undefined
								? {}
								: { raw_origin: request.raw_origin }),
							...(request.run_id === undefined ? {} : { run_id: request.run_id }),
							schema_version: 1,
							sent_at: resolved_at,
							thread_id: request.thread_id,
						});
						return Success(input.tool_id.replaceAll(".", "_"));
					}).pipe(Effect.catch(() => Effect.succeed(Failed())));
				case "terminal.open":
					return Effect.gen(function* () {
						const sent_at = yield* metadata.Now;
						yield* workspace_filesystems.Authorize({
							working_directory: input.working_directory,
							workspace_id: input.workspace_id,
						});
						const acceptance = yield* terminals.Handle({
							...(request.agent_id === undefined
								? {}
								: { agent_id: request.agent_id }),
							kind: "command",
							message_id: request.invocation_id,
							origin: "frontend",
							payload: {
								args: input.args,
								cols: input.cols,
								...(input.env === undefined ? {} : { env: input.env }),
								executable: input.executable,
								rows: input.rows,
								terminal_id: input.terminal_id,
								type: "terminal.open",
								working_directory: input.working_directory,
								workspace_id: input.workspace_id,
							},
							protocol_version: 1,
							...(request.raw_origin === undefined
								? {}
								: { raw_origin: request.raw_origin }),
							...(request.run_id === undefined ? {} : { run_id: request.run_id }),
							schema_version: 1,
							sent_at,
							thread_id: request.thread_id,
						});
						yield* evidence.RecordProcessOwnership({
							...trace,
							source: "artisan_tool",
							working_directory: acceptance.terminal.working_directory,
						});
						return Success("terminal_open");
					}).pipe(Effect.catch(() => Effect.succeed(Failed())));
				case "terminal.write":
					return Effect.gen(function* () {
						const sent_at = yield* metadata.Now;
						const acceptance = yield* terminals.Handle({
							...(request.agent_id === undefined
								? {}
								: { agent_id: request.agent_id }),
							kind: "command",
							message_id: request.invocation_id,
							origin: "frontend",
							payload: {
								data: input.data,
								terminal_id: input.terminal_id,
								type: "terminal.write",
							},
							protocol_version: 1,
							...(request.raw_origin === undefined
								? {}
								: { raw_origin: request.raw_origin }),
							...(request.run_id === undefined ? {} : { run_id: request.run_id }),
							schema_version: 1,
							sent_at,
							thread_id: request.thread_id,
						});
						yield* evidence.RecordProcessOwnership({
							...trace,
							source: "artisan_tool",
							working_directory: acceptance.terminal.working_directory,
						});
						return Success("terminal_write");
					}).pipe(Effect.catch(() => Effect.succeed(Failed())));
				case "terminal.restart":
					return Effect.gen(function* () {
						const sent_at = yield* metadata.Now;
						const acceptance = yield* terminals.Handle({
							...(request.agent_id === undefined
								? {}
								: { agent_id: request.agent_id }),
							kind: "command",
							message_id: request.invocation_id,
							origin: "frontend",
							payload: { terminal_id: input.terminal_id, type: "terminal.restart" },
							protocol_version: 1,
							...(request.raw_origin === undefined
								? {}
								: { raw_origin: request.raw_origin }),
							...(request.run_id === undefined ? {} : { run_id: request.run_id }),
							schema_version: 1,
							sent_at,
							thread_id: request.thread_id,
						});
						yield* evidence.RecordProcessOwnership({
							...trace,
							source: "artisan_tool",
							working_directory: acceptance.terminal.working_directory,
						});
						return Success("terminal_restart");
					}).pipe(Effect.catch(() => Effect.succeed(Failed())));
				case "terminal.stop":
					return Effect.gen(function* () {
						const sent_at = yield* metadata.Now;
						const acceptance = yield* terminals.Handle({
							...(request.agent_id === undefined
								? {}
								: { agent_id: request.agent_id }),
							kind: "command",
							message_id: request.invocation_id,
							origin: "frontend",
							payload: {
								...(input.signal === undefined ? {} : { signal: input.signal }),
								terminal_id: input.terminal_id,
								type: "terminal.kill",
							},
							protocol_version: 1,
							...(request.raw_origin === undefined
								? {}
								: { raw_origin: request.raw_origin }),
							...(request.run_id === undefined ? {} : { run_id: request.run_id }),
							schema_version: 1,
							sent_at,
							thread_id: request.thread_id,
						});
						yield* evidence.RecordProcessOwnership({
							...trace,
							source: "artisan_tool",
							working_directory: acceptance.terminal.working_directory,
						});
						return Success("terminal_stop");
					}).pipe(Effect.catch(() => Effect.succeed(Failed())));
				case "terminal.read":
					return terminals.ReadOutput(input.terminal_id).pipe(
						Effect.as(Success("terminal_read")),
						Effect.catch(() => Effect.succeed(Failed())),
					);
				case "preview.open":
				case "preview.inspect":
				case "preview.stop":
					return Effect.succeed(
						Unsupported("preview_unavailable", "No preview adapter is configured."),
					);
			}
		};
		return { Execute };
	}),
);
