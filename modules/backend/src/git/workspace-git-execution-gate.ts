import { Context, Effect, Layer, Schedule } from "effect";
import { isSqlError, type SqlError } from "effect/unstable/sql/SqlError";
import { layer as make_sqlite_layer, SqliteClient } from "@effect/sql-sqlite-node/SqliteClient";

export interface WorkspaceGitExecutionGateOptions {
	readonly database_path: string;
}

/** Serializes irreversible Git execution and recovery through a crash-released SQLite lock. */
export class WorkspaceGitExecutionGate extends Context.Service<
	WorkspaceGitExecutionGate,
	{
		readonly Run: <A, E, R>(
			gate_id: string,
			holder_id: string,
			effect: Effect.Effect<A, E, R>,
		) => Effect.Effect<A, E | SqlError, R>;
	}
>()("Artisan/WorkspaceGitExecutionGate") {}

const gate_contention_schedule = Schedule.exponential("5 millis").pipe(
	Schedule.upTo({ duration: "1 second", times: 8 }),
);

/** Builds the process-safe Git execution gate beside the primary Artisan database. */
export function make_workspace_git_execution_gate_layer(options: WorkspaceGitExecutionGateOptions) {
	const gate_database_path = `${options.database_path}.git-execution-gate.sqlite`;
	const SqliteLive = make_sqlite_layer({ filename: gate_database_path });
	const GateLive = Layer.effect(
		WorkspaceGitExecutionGate,
		Effect.gen(function* () {
			const client = yield* SqliteClient;

			yield* client.unsafe(
				`CREATE TABLE IF NOT EXISTS workspace_git_execution_gates (
					gate_id TEXT PRIMARY KEY NOT NULL,
					holder_id TEXT NOT NULL
				) WITHOUT ROWID`,
			).raw;

			const Run = <A, E, R>(
				gate_id: string,
				holder_id: string,
				effect: Effect.Effect<A, E, R>,
			) => {
				let execution_started = false;
				const transaction = client.withTransaction(
					Effect.gen(function* () {
						yield* client.unsafe(
							`INSERT INTO workspace_git_execution_gates (gate_id, holder_id)
							 VALUES (?, ?)
							 ON CONFLICT(gate_id) DO UPDATE SET holder_id = excluded.holder_id`,
							[gate_id, holder_id],
						).raw;
						yield* Effect.sync(() => {
							execution_started = true;
						});

						return yield* effect;
					}),
				);

				return transaction.pipe(
					Effect.retry({
						schedule: gate_contention_schedule,
						while: (error) =>
							!execution_started && isSqlError(error) && error.isRetryable,
					}),
				);
			};

			return { Run };
		}),
	);

	return GateLive.pipe(Layer.provide(SqliteLive));
}
