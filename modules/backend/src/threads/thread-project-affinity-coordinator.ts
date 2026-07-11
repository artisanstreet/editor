import { Context, Effect, Layer, Option, PubSub, Ref, Schedule, Semaphore } from "effect";

import type { EventEnvelope, ProjectAffinityEvidenceKind } from "@artisan/protocol";

import { JournalNotifier } from "../persistence/journal-notifier";
import { JournalStore, type JournalStoreError } from "../persistence/journal-store";
import { ThreadReadModel, type ThreadReadModelError } from "../persistence/thread-read-model";
import { ProjectLocator } from "./project-locator";
import {
	ThreadProjectAffinityRepository,
	type ThreadProjectAffinityError,
} from "./thread-project-affinity-repository";

interface ProjectPathObservation {
	readonly kind: ProjectAffinityEvidenceKind;
	readonly path: string;
}

export type ThreadProjectAffinityCoordinatorError =
	| JournalStoreError
	| ThreadProjectAffinityError
	| ThreadReadModelError;

/** Replays canonical path-bearing events into deterministic project-affinity evidence. */
export class ThreadProjectAffinityCoordinator extends Context.Service<
	ThreadProjectAffinityCoordinator,
	{
		readonly CatchUp: Effect.Effect<number, ThreadProjectAffinityCoordinatorError>;
	}
>()("Artisan/ThreadProjectAffinityCoordinator") {}

/** Keeps the portable backend deterministic when no platform project locator is composed. */
export const ThreadProjectAffinityCoordinatorDisabled = Layer.succeed(
	ThreadProjectAffinityCoordinator,
	{
		CatchUp: Effect.succeed(0),
	},
);

function observations_from_event(event: EventEnvelope): ReadonlyArray<ProjectPathObservation> {
	const payload = event.payload;

	if (payload.type === "thread.message_queued" || payload.type === "thread.message_steering") {
		return [
			{
				kind: "active_working_directory",
				path: payload.working_directory,
			},
		];
	}

	if (payload.type === "run.lifecycle") {
		const active =
			payload.state === "queued" ||
			payload.state === "running" ||
			payload.state === "waiting";

		return [
			{
				kind: active ? "active_working_directory" : "historical_working_directory",
				path: payload.working_directory,
			},
		];
	}

	if (payload.type === "terminal.lifecycle") {
		return [
			{
				kind: "terminal_working_directory",
				path: payload.terminal.working_directory,
			},
		];
	}

	if (
		payload.type === "artifact.recorded" &&
		(payload.artifact.kind === "file" || payload.artifact.kind === "diff") &&
		payload.artifact.uri
	) {
		return [{ kind: "file_artifact", path: payload.artifact.uri }];
	}

	return [];
}

function evidence_id(event: EventEnvelope, kind: ProjectAffinityEvidenceKind, project_id: string) {
	return `affinity:${event.message_id}:${kind}:${project_id}`;
}

/** Builds the scoped journal consumer that activates automatic project affinity. */
export const ThreadProjectAffinityCoordinatorLive = Layer.effect(
	ThreadProjectAffinityCoordinator,
	Effect.gen(function* () {
		const journal = yield* JournalStore;
		const locator = yield* ProjectLocator;
		const notifier = yield* JournalNotifier;
		const read_model = yield* ThreadReadModel;
		const repository = yield* ThreadProjectAffinityRepository;
		const subscription = yield* notifier.Subscribe;
		const lock = yield* Semaphore.make(1);
		const journal_sequence = yield* Ref.make(0);

		const ObserveEvent = (event: EventEnvelope) =>
			Effect.gen(function* () {
				const observations = observations_from_event(event);

				if (observations.length === 0) {
					return;
				}

				const thread = yield* read_model.Lookup(event.thread_id);

				if (Option.isNone(thread)) {
					return;
				}

				const unique_observations = new Map(
					observations.map((observation) => [
						`${observation.kind}:${observation.path}`,
						observation,
					]),
				);

				yield* Effect.forEach(unique_observations.values(), (observation) =>
					Effect.gen(function* () {
						const located = yield* locator
							.Locate(observation.path)
							.pipe(Effect.catch(() => Effect.succeed(Option.none())));

						if (Option.isNone(located)) {
							return;
						}

						const kinds: ReadonlyArray<ProjectAffinityEvidenceKind> = [
							observation.kind,
							...(located.value.source === "git_root" ? (["git_root"] as const) : []),
						];

						yield* Effect.forEach(kinds, (kind) =>
							repository.ObserveEvidence({
								basis_affinity_version: thread.value.affinity_version,
								evidence_id: evidence_id(
									event,
									kind,
									located.value.project.project_id,
								),
								kind,
								observed_at: event.sent_at,
								project: located.value.project,
								source_event_id: event.message_id,
								source_journal_sequence: event.journal_sequence,
								thread_id: event.thread_id,
							}),
						);
					}),
				);
			});

		const CatchUpUnlocked = Effect.gen(function* () {
			const cursor = yield* Ref.get(journal_sequence);
			const events = yield* journal.ReadReplay({ after_journal_sequence: cursor });

			if (events.length === 0) {
				return 0;
			}

			yield* Effect.forEach(events, ObserveEvent);
			yield* Ref.set(journal_sequence, events.at(-1)!.journal_sequence);

			return events.length;
		});

		const CatchUp = Semaphore.withPermit(lock)(CatchUpUnlocked);
		const CatchUpReliably = CatchUp.pipe(
			Effect.retry({ schedule: Schedule.spaced("250 millis") }),
		);
		const Watch = Effect.gen(function* () {
			yield* CatchUpReliably;

			while (true) {
				yield* PubSub.take(subscription);
				yield* CatchUpReliably;
			}
		});

		yield* Watch.pipe(Effect.forkScoped);

		return { CatchUp };
	}),
);
