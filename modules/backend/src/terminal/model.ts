import { Data, Effect, Schema } from "effect";

import {
	TerminalSession,
	type CommandEnvelope,
	type EventEnvelope,
	type TerminalLifecycleEvent,
} from "@artisan/protocol";

import { TerminalSessions } from "../persistence/tables";

export type TerminalCommand = Extract<
	CommandEnvelope["payload"],
	{ readonly type: `terminal.${string}` }
>;

export type TerminalLifecycleAction = TerminalLifecycleEvent["action"];

export interface StoredTerminalSession {
	readonly env?: Readonly<Record<string, string>> | undefined;
	readonly terminal: TerminalSession;
}

export interface TerminalCommandClaim {
	readonly command_status: "completed" | "dispatching" | "failed";
	readonly generation: number;
	readonly status: "accepted" | "duplicate";
	readonly stored: StoredTerminalSession;
}

export type TerminalCommandTransition =
	| { readonly _tag: "active"; readonly pid: number }
	| { readonly _tag: "current" }
	| { readonly _tag: "failed"; readonly failure: string }
	| { readonly _tag: "pin"; readonly pinned: boolean }
	| { readonly _tag: "resize"; readonly cols: number; readonly rows: number };

export interface TerminalCommit {
	readonly event: EventEnvelope;
	readonly stored: StoredTerminalSession;
}

export class TerminalCommandConflict extends Data.TaggedError("TerminalCommandConflict")<{
	readonly message_id: string;
}> {}

export class TerminalNotFound extends Data.TaggedError("TerminalNotFound")<{
	readonly terminal_id: string;
}> {}

export class TerminalNotActive extends Data.TaggedError("TerminalNotActive")<{
	readonly terminal_id: string;
}> {}

export class TerminalInvariantError extends Data.TaggedError("TerminalInvariantError")<{
	readonly message: string;
}> {}

export class TerminalPersistenceFailure extends Data.TaggedError("TerminalPersistenceFailure")<{
	readonly cause: unknown;
}> {}

export type TerminalRepositoryError =
	| TerminalCommandConflict
	| TerminalInvariantError
	| TerminalNotActive
	| TerminalNotFound
	| TerminalPersistenceFailure;

const StringArray = Schema.Array(Schema.String);
const EnvironmentRecord = Schema.Record(Schema.String, Schema.String);
export const TerminalCommandStatus = Schema.Literals(["completed", "dispatching", "failed"]);
const StoredTerminalSnapshot = Schema.Struct({
	env: Schema.optional(EnvironmentRecord),
	terminal: TerminalSession,
});

const ParseJson = (json: string, context: string) =>
	Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(json).pipe(
		Effect.mapError(
			() => new TerminalInvariantError({ message: `${context} contains invalid JSON` }),
		),
	);

export const DecodeStoredSnapshot = (json: string, context: string) =>
	ParseJson(json, context).pipe(
		Effect.flatMap(
			Schema.decodeUnknownEffect(StoredTerminalSnapshot, {
				onExcessProperty: "error",
			}),
		),
		Effect.mapError(
			() =>
				new TerminalInvariantError({
					message: `${context} does not match the terminal snapshot schema`,
				}),
		),
	);

export const DecodeStoredSession = (row: typeof TerminalSessions.$inferSelect) =>
	Effect.gen(function* () {
		const args = yield* ParseJson(row.args_json, `Terminal ${row.terminal_id} args`).pipe(
			Effect.flatMap(Schema.decodeUnknownEffect(StringArray)),
			Effect.mapError(
				() =>
					new TerminalInvariantError({
						message: `Terminal ${row.terminal_id} args do not match the schema`,
					}),
			),
		);
		const env = row.env_json
			? yield* ParseJson(row.env_json, `Terminal ${row.terminal_id} environment`).pipe(
					Effect.flatMap(Schema.decodeUnknownEffect(EnvironmentRecord)),
					Effect.mapError(
						() =>
							new TerminalInvariantError({
								message: `Terminal ${row.terminal_id} environment does not match the schema`,
							}),
					),
				)
			: undefined;
		const terminal = yield* Schema.decodeUnknownEffect(TerminalSession, {
			onExcessProperty: "error",
		})({
			args,
			closed_at: row.closed_at ?? undefined,
			cols: row.cols,
			created_at: row.created_at,
			executable: row.executable,
			exit_code: row.exit_code ?? undefined,
			exit_reason: row.exit_reason ?? undefined,
			exit_signal: row.exit_signal ?? undefined,
			failure: row.failure ?? undefined,
			generation: row.generation,
			pid: row.pid ?? undefined,
			ownership:
				row.owner_kind === "agent" && row.owner_agent_id && row.owner_run_id
					? {
							kind: "agent" as const,
							agent_id: row.owner_agent_id,
							run_id: row.owner_run_id,
						}
					: { kind: "user" as const },
			pinned: row.pinned,
			rows: row.rows,
			state: row.state,
			terminal_id: row.terminal_id,
			thread_id: row.thread_id,
			updated_at: row.updated_at,
			workspace_id: row.workspace_id,
			working_directory: row.working_directory,
		}).pipe(
			Effect.mapError(
				() =>
					new TerminalInvariantError({
						message: `Terminal ${row.terminal_id} does not match the protocol schema`,
					}),
			),
		);

		return { ...(env ? { env } : {}), terminal } satisfies StoredTerminalSession;
	});

export const NormalizeTerminalError = (error: unknown): TerminalRepositoryError => {
	if (
		error instanceof TerminalCommandConflict ||
		error instanceof TerminalInvariantError ||
		error instanceof TerminalNotActive ||
		error instanceof TerminalNotFound
	) {
		return error;
	}

	return new TerminalPersistenceFailure({ cause: error });
};

export const FailedSnapshot = (
	stored: StoredTerminalSession,
	failure: string,
	updated_at: string,
): StoredTerminalSession => {
	const { pid: _pid, ...without_pid } = stored.terminal;

	return {
		...(stored.env ? { env: stored.env } : {}),
		terminal: {
			...without_pid,
			closed_at: updated_at,
			failure,
			state: "failed",
			updated_at,
		},
	};
};

export const TerminalCommandMatches = (
	command: CommandEnvelope,
	existing: {
		readonly agent_id: string | null;
		readonly causation_id: string | null;
		readonly origin: string;
		readonly payload_json: string;
		readonly raw_origin_json: string | null;
		readonly run_id: string | null;
		readonly schema_version: number;
		readonly sent_at: string;
		readonly thread_id: string;
	},
) =>
	existing.agent_id === (command.agent_id ?? null) &&
	existing.causation_id === (command.causation_id ?? null) &&
	existing.origin === command.origin &&
	existing.payload_json === JSON.stringify(command.payload) &&
	existing.raw_origin_json === (command.raw_origin ? JSON.stringify(command.raw_origin) : null) &&
	existing.run_id === (command.run_id ?? null) &&
	existing.schema_version === command.schema_version &&
	existing.sent_at === command.sent_at &&
	existing.thread_id === command.thread_id;

export const RequireTerminalRow = <Row>(row: Row | undefined, message: string) =>
	row === undefined ? Effect.fail(new TerminalInvariantError({ message })) : Effect.succeed(row);
