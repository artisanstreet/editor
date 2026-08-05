import { and, eq, sql } from "drizzle-orm";
import { Effect, Option, Schema } from "effect";
import type { EngineResumeToken } from "@artisan/engines";
import { Database } from "../database";
import { AppendJournalEventInTransaction } from "../journal-store";
import { JournalNotifier } from "../journal-notifier";
import {
	OrchestrationCoordinators,
	OrchestrationOutbox,
	OrchestrationRuns,
	ThreadErasureClaims,
	ThreadTombstones,
	Threads,
} from "../tables";
import {
	ThreadContinuationLaunches,
	ThreadRunContinuationState,
} from "../thread-continuation-schema";
import { RuntimeMetadata } from "../../runtime/metadata";
import { DecodePersistedJson, DecodeResumeToken } from "./codec";
import {
	EngineResumeTokenSchema,
	FailureCode,
	ThreadContinuationConflict,
	ThreadContinuationFailure,
} from "./contracts";

export const MakeLaunchStateOperations = Effect.gen(function* () {
	const database = yield* Database;
	const notifier = yield* JournalNotifier;
	const metadata = yield* RuntimeMetadata;
	const Fail = (code: string) => new ThreadContinuationFailure({ code });

	/**
	 * Whether this launch is a handoff worth announcing.
	 *
	 * A run started on its engine's default records no model id, so an unknown
	 * side is not evidence of a change: comparing it to a named model would
	 * announce a switch the user never made. Only two models we can both name,
	 * and that genuinely differ, count — an engine swap always does.
	 */
	const transition_endpoints = (
		source: { readonly engine_id: string; readonly model_id: string | null } | undefined,
		target: { readonly engine_id: string; readonly model_id: string | null },
		source_kind: string,
	) => {
		if (source === undefined) return undefined;
		if (source_kind !== "native" && source_kind !== "portable") return undefined;

		const model_changed =
			source.model_id != null &&
			target.model_id != null &&
			source.model_id !== target.model_id;
		if (source.engine_id === target.engine_id && !model_changed) return undefined;

		return {
			continuation: source_kind,
			source: {
				engine_id: source.engine_id,
				...(source.model_id === null ? {} : { model_id: source.model_id }),
			},
			target: {
				engine_id: target.engine_id,
				...(target.model_id === null ? {} : { model_id: target.model_id }),
			},
		} as const;
	};
	const EnsureLiveThread = (transaction: typeof database.client, thread_id: string) =>
		Effect.gen(function* () {
			const [[thread], [claim], [tombstone]] = yield* Effect.all([
				transaction.select().from(Threads).where(eq(Threads.thread_id, thread_id)).limit(1),
				transaction
					.select()
					.from(ThreadErasureClaims)
					.where(eq(ThreadErasureClaims.thread_id, thread_id))
					.limit(1),
				transaction
					.select()
					.from(ThreadTombstones)
					.where(eq(ThreadTombstones.thread_id, thread_id))
					.limit(1),
			]);
			if (!thread || claim || tombstone)
				return yield* Effect.fail(Fail("thread_unavailable"));
		});

	const MarkOpening = (target_run_id: string) =>
		Effect.gen(function* () {
			const updated_at = yield* metadata.Now;
			const opening_event = yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const [launch] = yield* transaction
						.select()
						.from(ThreadContinuationLaunches)
						.where(eq(ThreadContinuationLaunches.target_run_id, target_run_id))
						.limit(1);
					if (!launch) return yield* Effect.fail(Fail("launch_missing"));
					yield* EnsureLiveThread(transaction, launch.thread_id);
					const [[run], [outbox]] = yield* Effect.all([
						transaction
							.select()
							.from(OrchestrationRuns)
							.where(eq(OrchestrationRuns.run_id, launch.target_run_id))
							.limit(1),
						transaction
							.select()
							.from(OrchestrationOutbox)
							.where(
								and(
									eq(OrchestrationOutbox.run_id, launch.target_run_id),
									eq(OrchestrationOutbox.command_id, launch.request_id),
								),
							)
							.limit(1),
					]);
					if (
						!run ||
						run.status !== "queued" ||
						!outbox ||
						outbox.kind !== "start" ||
						outbox.status !== "dispatching"
					)
						return yield* Effect.fail(Fail("launch_not_openable"));
					const changed = yield* transaction
						.update(ThreadContinuationLaunches)
						.set({ state: "opening", updated_at })
						.where(
							and(
								eq(ThreadContinuationLaunches.target_run_id, target_run_id),
								eq(ThreadContinuationLaunches.state, "prepared"),
							),
						)
						.returning();
					if (changed.length !== 1)
						return yield* Effect.fail(Fail("launch_not_prepared"));

					/**
					 * The handoff opens here and lands at bind. Announcing the start is
					 * what lets the thread say it is changing hands, instead of looking
					 * idle or, worse, looking like the next model already thinks.
					 */
					const [source] =
						launch.source_run_id === null
							? []
							: yield* transaction
									.select({
										engine_id: OrchestrationRuns.engine_id,
										model_id: OrchestrationRuns.model_id,
									})
									.from(OrchestrationRuns)
									.where(eq(OrchestrationRuns.run_id, launch.source_run_id))
									.limit(1);
					const endpoints = transition_endpoints(
						source,
						{
							engine_id: run.engine_id,
							model_id: launch.target_model_id ?? run.model_id,
						},
						launch.source_kind,
					);

					return endpoints === undefined
						? undefined
						: yield* AppendJournalEventInTransaction(transaction, metadata, {
								agent_id: run.agent_id,
								causation_id: launch.request_id,
								correlation_id: launch.request_id,
								idempotency_key: `thread-model-transition-started:${target_run_id}`,
								payload: {
									...endpoints,
									state: "started",
									type: "thread.model_transition",
								},
								run_id: target_run_id,
								thread_id: launch.thread_id,
							});
				}),
			);
			if (opening_event !== undefined)
				yield* notifier.Publish(opening_event.journal_sequence);
		}).pipe(
			Effect.mapError((cause) =>
				cause instanceof ThreadContinuationFailure ? cause : Fail("launch_opening_failed"),
			),
		);

	const BindTarget = (input: {
		readonly command_id: string;
		readonly model_id?: string;
		readonly native_thread_id: string;
		readonly resume_token: EngineResumeToken;
		readonly target_run_id: string;
	}) =>
		Effect.gen(function* () {
			const resume_token = yield* Schema.decodeUnknownEffect(EngineResumeTokenSchema)(
				input.resume_token,
			).pipe(Effect.mapError(() => Fail("resume_token_invalid")));
			if (resume_token.native_thread_id !== input.native_thread_id)
				return yield* Effect.fail(Fail("resume_token_thread_mismatch"));
			const updated_at = yield* metadata.Now;
			const transition_event = yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const [target] = yield* transaction
						.select()
						.from(OrchestrationRuns)
						.where(eq(OrchestrationRuns.run_id, input.target_run_id))
						.limit(1);
					if (!target) return yield* Effect.fail(Fail("target_run_missing"));
					const [[launch], [outbox], [coordinator]] = yield* Effect.all([
						transaction
							.select()
							.from(ThreadContinuationLaunches)
							.where(
								eq(ThreadContinuationLaunches.target_run_id, input.target_run_id),
							)
							.limit(1),
						transaction
							.select()
							.from(OrchestrationOutbox)
							.where(
								and(
									eq(OrchestrationOutbox.run_id, input.target_run_id),
									eq(OrchestrationOutbox.command_id, input.command_id),
								),
							)
							.limit(1),
						transaction
							.select()
							.from(OrchestrationCoordinators)
							.where(eq(OrchestrationCoordinators.thread_id, target.thread_id))
							.limit(1),
					]);
					yield* EnsureLiveThread(transaction, target.thread_id);
					if (
						!launch ||
						!outbox ||
						!coordinator ||
						launch.request_id !== input.command_id ||
						launch.thread_id !== target.thread_id ||
						launch.target_engine_id !== target.engine_id ||
						launch.target_model_id !== (input.model_id ?? null) ||
						coordinator.thread_id !== target.thread_id ||
						(coordinator.active_run_id === target.run_id &&
							coordinator.engine_id !== target.engine_id)
					)
						return yield* Effect.fail(Fail("target_bind_intent_mismatch"));
					if (launch.state === "bound") {
						const persisted_resume = DecodeResumeToken(
							DecodePersistedJson(target.native_resume_json).pipe(
								Option.getOrUndefined,
							),
						);
						if (
							outbox.status !== "delivered" ||
							target.native_thread_id !== input.native_thread_id ||
							target.model_id !== (input.model_id ?? null) ||
							Option.isNone(persisted_resume) ||
							JSON.stringify(persisted_resume.value) !==
								JSON.stringify(resume_token) ||
							(coordinator.active_run_id === target.run_id &&
								(coordinator.native_thread_id !== input.native_thread_id ||
									coordinator.native_resume_json !==
										JSON.stringify(resume_token)))
						)
							return yield* Effect.fail(
								new ThreadContinuationConflict({
									target_run_id: input.target_run_id,
								}),
							);
						return;
					}
					const [source] =
						launch.source_run_id === null
							? []
							: yield* transaction
									.select({
										engine_id: OrchestrationRuns.engine_id,
										model_id: OrchestrationRuns.model_id,
									})
									.from(OrchestrationRuns)
									.where(eq(OrchestrationRuns.run_id, launch.source_run_id))
									.limit(1);
					if (launch.source_run_id !== null && source === undefined)
						return yield* Effect.fail(Fail("source_run_missing"));
					if (launch.state !== "opening" || outbox.status !== "dispatching")
						return yield* Effect.fail(Fail("launch_not_opening"));
					const [changed_launch] = yield* transaction
						.update(ThreadContinuationLaunches)
						.set({ state: "bound", updated_at })
						.where(
							and(
								eq(ThreadContinuationLaunches.target_run_id, input.target_run_id),
								eq(ThreadContinuationLaunches.state, "opening"),
							),
						)
						.returning();
					if (!changed_launch) return yield* Effect.fail(Fail("launch_not_opening"));
					const [run] = yield* transaction
						.update(OrchestrationRuns)
						.set({
							...(input.model_id === undefined ? {} : { model_id: input.model_id }),
							native_resume_json: JSON.stringify(resume_token),
							native_thread_id: input.native_thread_id,
							updated_at,
						})
						.where(eq(OrchestrationRuns.run_id, input.target_run_id))
						.returning();
					if (!run) return yield* Effect.fail(Fail("target_bind_failed"));
					if (coordinator.active_run_id === target.run_id) {
						const [updated_coordinator] = yield* transaction
							.update(OrchestrationCoordinators)
							.set({
								native_resume_json: JSON.stringify(resume_token),
								native_thread_id: input.native_thread_id,
								updated_at,
							})
							.where(
								and(
									eq(OrchestrationCoordinators.thread_id, target.thread_id),
									eq(OrchestrationCoordinators.active_run_id, target.run_id),
								),
							)
							.returning();
						if (!updated_coordinator)
							return yield* Effect.fail(Fail("coordinator_bind_failed"));
					}
					yield* transaction
						.insert(ThreadRunContinuationState)
						.values({
							created_at: updated_at,
							engine_id: target.engine_id,
							last_observation_sequence: 0,
							model_id: input.model_id ?? target.model_id,
							run_id: input.target_run_id,
							thread_id: target.thread_id,
							updated_at,
						})
						.onConflictDoUpdate({
							target: ThreadRunContinuationState.run_id,
							set: {
								engine_id: target.engine_id,
								model_id: input.model_id ?? target.model_id,
								updated_at,
							},
						});
					const [updated_outbox] = yield* transaction
						.update(OrchestrationOutbox)
						.set({ status: "delivered", updated_at })
						.where(
							and(
								eq(OrchestrationOutbox.command_id, input.command_id),
								eq(OrchestrationOutbox.run_id, input.target_run_id),
								eq(OrchestrationOutbox.status, "dispatching"),
							),
						)
						.returning();
					if (!updated_outbox) return yield* Effect.fail(Fail("outbox_bind_failed"));
					const target_model_id = input.model_id ?? target.model_id;
					const endpoints = transition_endpoints(
						source,
						{ engine_id: target.engine_id, model_id: target_model_id },
						launch.source_kind,
					);
					const emitted_transition =
						endpoints === undefined
							? undefined
							: yield* AppendJournalEventInTransaction(transaction, metadata, {
									agent_id: target.agent_id,
									causation_id: input.command_id,
									correlation_id: input.command_id,
									idempotency_key: `thread-model-transition-completed:${target.run_id}`,
									payload: {
										...endpoints,
										state: "completed",
										type: "thread.model_transition",
									},
									run_id: target.run_id,
									thread_id: target.thread_id,
								});
					return emitted_transition;
				}),
			);
			if (transition_event !== undefined)
				yield* notifier.Publish(transition_event.journal_sequence);
		}).pipe(
			Effect.mapError((cause) =>
				cause instanceof ThreadContinuationFailure ||
				cause instanceof ThreadContinuationConflict
					? cause
					: Fail("target_bind_transaction_failed"),
			),
		);

	const FailLaunch = (target_run_id: string, failure_code: string) =>
		Effect.gen(function* () {
			const code = yield* Schema.decodeUnknownEffect(FailureCode)(failure_code).pipe(
				Effect.mapError(() => Fail("failure_code_invalid")),
			);
			const updated_at = yield* metadata.Now;
			yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const [launch] = yield* transaction
						.select()
						.from(ThreadContinuationLaunches)
						.where(eq(ThreadContinuationLaunches.target_run_id, target_run_id))
						.limit(1);
					if (!launch) return yield* Effect.fail(Fail("launch_missing"));
					yield* EnsureLiveThread(transaction, launch.thread_id);
					if (launch.state === "failed" && launch.failure_code === code) return;
					if (launch.state !== "prepared" && launch.state !== "opening")
						return yield* Effect.fail(Fail("launch_not_failable"));
					const changed = yield* transaction
						.update(ThreadContinuationLaunches)
						.set({ failure_code: code, state: "failed", updated_at })
						.where(eq(ThreadContinuationLaunches.target_run_id, target_run_id))
						.returning();
					if (changed.length !== 1)
						return yield* Effect.fail(Fail("launch_not_failable"));
				}),
			);
		}).pipe(
			Effect.mapError((cause) =>
				cause instanceof ThreadContinuationFailure ? cause : Fail("launch_fail_failed"),
			),
		);

	const ReconcileStranded = () =>
		Effect.gen(function* () {
			const updated_at = yield* metadata.Now;
			return yield* database.client.transaction((transaction) =>
				Effect.gen(function* () {
					const launches = yield* transaction
						.select()
						.from(ThreadContinuationLaunches)
						.where(sql`${ThreadContinuationLaunches.state} IN ('prepared', 'opening')`);
					const reconciled: Array<string> = [];
					for (const launch of launches) {
						const [[thread], [claim], [tombstone]] = yield* Effect.all([
							transaction
								.select()
								.from(Threads)
								.where(eq(Threads.thread_id, launch.thread_id))
								.limit(1),
							transaction
								.select()
								.from(ThreadErasureClaims)
								.where(eq(ThreadErasureClaims.thread_id, launch.thread_id))
								.limit(1),
							transaction
								.select()
								.from(ThreadTombstones)
								.where(eq(ThreadTombstones.thread_id, launch.thread_id))
								.limit(1),
						]);
						if (!thread || claim || tombstone) continue;
						/**
						 * Recovery invokes this only after proving that the process owns no
						 * live EngineRun. Any prepared/opening row is therefore ownerless
						 * even if its run and outbox still look active.
						 */
						yield* transaction
							.update(ThreadContinuationLaunches)
							.set({
								failure_code: "stranded_recovery",
								state: "failed",
								updated_at,
							})
							.where(
								eq(ThreadContinuationLaunches.target_run_id, launch.target_run_id),
							);
						reconciled.push(launch.target_run_id);
					}
					return reconciled;
				}),
			);
		}).pipe(
			Effect.mapError((cause) =>
				cause instanceof ThreadContinuationFailure
					? cause
					: Fail("stranded_reconcile_failed"),
			),
		);

	return { BindTarget, FailLaunch, MarkOpening, ReconcileStranded };
});
