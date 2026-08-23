import { Context, Effect, Layer, Option } from "effect";

import type { EngineObservation, EngineRunTerminalState } from "@artisan/engines";

import { ThreadReadModel } from "../persistence/thread-read-model";
import { RuntimeMetadata } from "../runtime/metadata";
import type { ObservedTerminalOwner } from "./observed";
import { TerminalSessionService } from "./sessions";

/** The run whose engine was observed running shells, and where it was running them. */
export interface ObservedTerminalRun {
	readonly agent_id: string;
	readonly run_id: string;
	readonly thread_id: string;
	readonly working_directory: string;
}

/**
 * Bridges the shells an engine runs underneath into the Terminals card.
 *
 * The normalizer has always reported these — `terminal_activity` observations
 * carrying the command, its output, and its exit — but the only consumer was
 * the conversation projection, which turns them into transcript rows. The card
 * reads `terminal_sessions`, so it stayed empty no matter what the engine was
 * running. This is the missing consumer.
 *
 * Adoption is best effort by construction: a run must not fail because a
 * terminal row could not be written, and a thread with no project has no
 * workspace to file one under.
 */
export class ObservedTerminalAdoption extends Context.Service<
	ObservedTerminalAdoption,
	{
		readonly AdoptBatch: (
			run: ObservedTerminalRun,
			observations: ReadonlyArray<EngineObservation>,
		) => Effect.Effect<void>;
		readonly SettleRun: (run_id: string, state: EngineRunTerminalState) => Effect.Effect<void>;
	}
>()("Artisan/ObservedTerminalAdoption") {}

export const ObservedTerminalAdoptionLive = Layer.effect(
	ObservedTerminalAdoption,
	Effect.gen(function* () {
		const metadata = yield* RuntimeMetadata;
		const terminals = yield* TerminalSessionService;
		const threads = yield* ThreadReadModel;

		/**
		 * The card lists terminals by thread *and* workspace, so an adopted row
		 * needs the project the thread belongs to. It is read per batch rather
		 * than per observation: one command emits several frames, and they all
		 * belong to the same thread.
		 */
		const WorkspaceFor = (thread_id: string) =>
			threads.Lookup(thread_id).pipe(
				Effect.map((thread) =>
					Option.flatMapNullishOr(thread, (found) => found.primary_project?.project_id),
				),
				Effect.catch(() => Effect.succeed(Option.none<string>())),
			);

		const AdoptBatch = (
			run: ObservedTerminalRun,
			observations: ReadonlyArray<EngineObservation>,
		) =>
			Effect.gen(function* () {
				const activities = observations.filter(
					(observation) => observation._tag === "terminal_activity",
				);
				if (activities.length === 0) return;

				const workspace_id = yield* WorkspaceFor(run.thread_id);
				if (Option.isNone(workspace_id)) return;

				const owner: ObservedTerminalOwner = {
					agent_id: run.agent_id,
					run_id: run.run_id,
				};
				const observed_at = yield* metadata.Now;

				for (const activity of activities) {
					yield* terminals.AdoptObserved(activity, {
						observed_at,
						owner,
						thread_id: run.thread_id,
						workspace_id: workspace_id.value,
						working_directory: run.working_directory,
					});
				}
			}).pipe(
				/**
				 * A terminal row is a view of work that already happened. Failing the
				 * run because the view could not be written would trade the actual
				 * work for its reflection.
				 */
				Effect.catchCause(() => Effect.void),
			);

		const SettleRun = (run_id: string, state: EngineRunTerminalState) =>
			terminals
				.SettleObservedRun(
					run_id,
					state === "completed"
						? { action: "exited", exit_reason: "exited", state: "closed" }
						: state === "cancelled"
							? { action: "killed", exit_reason: "killed", state: "closed" }
							: state === "closed"
								? { action: "closed", exit_reason: "closed", state: "closed" }
								: {
										action: "failed",
										failure:
											state === "interrupted"
												? "The owning engine run ended before this command reported completion."
												: "The owning engine run failed before this command reported completion.",
										state: "failed",
									},
				)
				.pipe(Effect.catchCause(() => Effect.void));

		return { AdoptBatch, SettleRun };
	}),
);
