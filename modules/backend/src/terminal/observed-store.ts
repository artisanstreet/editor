import { and, eq, inArray, sql } from "drizzle-orm";
import { Context, Effect } from "effect";

import type { TerminalSession } from "@artisan/protocol";

import type { Database } from "../persistence/database";
import type { JournalNotifier } from "../persistence/journal-notifier";
import { TerminalSessions } from "../persistence/tables";
import type { RuntimeMetadata } from "../runtime/metadata";
import type { TerminalJournal } from "./journal";
import { DecodeStoredSession, NormalizeTerminalError } from "./model";
import type { ObservedTerminalSettlement } from "./observed";

/**
 * Persists the shells an engine ran inside its own harness.
 *
 * Kept apart from the command repository because it shares none of its
 * machinery: there is no claim to take, no generation to advance, and no PTY to
 * dispatch to. The command has already run somewhere this process does not own,
 * so the whole lifecycle arrives as a projection and the row is simply the
 * latest frame of it.
 */
export interface ObservedTerminalStoreDependencies {
	readonly database: Context.Service.Shape<typeof Database>;
	readonly journal: Context.Service.Shape<typeof TerminalJournal>;
	readonly metadata: Context.Service.Shape<typeof RuntimeMetadata>;
	readonly notifier: Context.Service.Shape<typeof JournalNotifier>;
}

export const MakeObservedTerminalStore = ({
	database,
	journal,
	metadata,
	notifier,
}: ObservedTerminalStoreDependencies) => {
	/** Conflicting on `terminal_id` keeps a replayed observation from opening a second row. */
	const UpsertRow = (
		transaction: typeof database.client,
		session: TerminalSession,
		instance_id: string,
	) =>
		transaction
			.insert(TerminalSessions)
			.values({
				args_json: JSON.stringify(session.args),
				closed_at: session.closed_at ?? null,
				cols: session.cols,
				created_at: session.created_at,
				env_json: null,
				executable: session.executable,
				exit_code: session.exit_code ?? null,
				exit_reason: session.exit_reason ?? null,
				exit_signal: session.exit_signal ?? null,
				failure: session.failure ?? null,
				generation: session.generation,
				owner_agent_id:
					session.ownership?.kind === "agent" ? session.ownership.agent_id : null,
				owner_instance_id: instance_id,
				owner_kind: session.ownership?.kind === "agent" ? "agent" : "user",
				owner_run_id: session.ownership?.kind === "agent" ? session.ownership.run_id : null,
				pid: null,
				pinned: false,
				rows: session.rows,
				state: session.state,
				stop_requested_generation: null,
				terminal_id: session.terminal_id,
				thread_id: session.thread_id,
				updated_at: session.updated_at,
				workspace_id: session.workspace_id,
				working_directory: session.working_directory,
			})
			.onConflictDoUpdate({
				set: {
					closed_at: session.closed_at ?? null,
					exit_code: session.exit_code ?? null,
					exit_reason: session.exit_reason ?? null,
					state: session.state,
					updated_at: session.updated_at,
				},
				target: TerminalSessions.terminal_id,
			})
			.pipe(Effect.asVoid);

	const AdoptObserved = (session: TerminalSession, instance_id: string) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					yield* UpsertRow(transaction, session, instance_id);
					/**
					 * The card follows the same lifecycle stream a real terminal
					 * publishes on, so an adopted shell appears while it is running
					 * rather than on the next time the panel is opened.
					 */
					const correlation_id = yield* metadata.MakeId("message");
					return yield* journal.Append(transaction, {
						action: session.state === "active" ? "opened" : "exited",
						causation_id: correlation_id,
						correlation_id,
						...(session.ownership?.kind === "agent"
							? {
									agent_id: session.ownership.agent_id,
									run_id: session.ownership.run_id,
								}
							: {}),
						terminal: session,
					});
				}),
			)
			.pipe(
				Effect.mapError(NormalizeTerminalError),
				Effect.tap((event) => notifier.Publish(event.journal_sequence)),
				Effect.asVoid,
			);

	/**
	 * A provider may terminate a tool container without delivering that tool's
	 * final frame. The run closing is still authoritative: an observed command
	 * cannot remain live after the run that owned it has settled.
	 */
	const SettleObservedRun = (run_id: string, settlement: ObservedTerminalSettlement) =>
		database.client
			.transaction((transaction) =>
				Effect.gen(function* () {
					const candidates = yield* transaction
						.select()
						.from(TerminalSessions)
						.where(
							and(
								eq(TerminalSessions.owner_run_id, run_id),
								inArray(TerminalSessions.state, ["opening", "active"]),
								sql`${TerminalSessions.terminal_id} GLOB 'observed_*'`,
							),
						);
					if (candidates.length === 0) {
						return {
							settled: [] as ReadonlyArray<TerminalSession>,
							watermark: undefined,
						};
					}

					const settled_at = yield* metadata.Now;
					const settled: Array<TerminalSession> = [];
					let watermark: number | undefined;
					for (const candidate of candidates) {
						const [updated] = yield* transaction
							.update(TerminalSessions)
							.set({
								closed_at: settled_at,
								exit_reason:
									settlement.state === "closed" ? settlement.exit_reason : null,
								failure: settlement.state === "failed" ? settlement.failure : null,
								state: settlement.state,
								updated_at: settled_at,
							})
							.where(
								and(
									eq(TerminalSessions.terminal_id, candidate.terminal_id),
									inArray(TerminalSessions.state, ["opening", "active"]),
								),
							)
							.returning();
						if (updated === undefined) continue;

						const session = (yield* DecodeStoredSession(updated)).terminal;
						const correlation_id = yield* metadata.MakeId("message");
						const event = yield* journal.Append(transaction, {
							action: settlement.action,
							...(session.ownership?.kind === "agent"
								? { agent_id: session.ownership.agent_id }
								: {}),
							causation_id: correlation_id,
							correlation_id,
							run_id,
							terminal: session,
						});
						settled.push(session);
						watermark = event.journal_sequence;
					}

					return { settled, watermark };
				}),
			)
			.pipe(
				Effect.mapError(NormalizeTerminalError),
				Effect.tap(({ watermark }) =>
					watermark === undefined ? Effect.void : notifier.Publish(watermark),
				),
				Effect.map(({ settled }) => settled),
			);

	return { AdoptObserved, SettleObservedRun };
};
