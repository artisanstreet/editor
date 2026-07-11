import { Context, Effect, Layer, Option, PubSub, Ref, Schedule, Semaphore } from "effect";

import type { EventEnvelope, ProjectAffinityEvidenceKind, ProjectRef } from "@artisan/protocol";

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

interface ProjectDirectObservation {
	readonly kind: ProjectAffinityEvidenceKind;
	readonly project: ProjectRef;
}

type ProjectObservation = ProjectDirectObservation | ProjectPathObservation;

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

function observations_from_event(event: EventEnvelope): ReadonlyArray<ProjectObservation> {
	const payload = event.payload;

	if (payload.type === "thread.message_queued" || payload.type === "thread.message_steering") {
		return [
			{
				kind: "active_working_directory",
				path: payload.working_directory,
			},
			...(payload.mentioned_projects ?? []).map((project) => ({
				kind: "project_mention" as const,
				project,
			})),
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
			{ kind: "process_owner", path: payload.working_directory },
		];
	}

	if (payload.type === "terminal.lifecycle") {
		return [
			{
				kind: "terminal_working_directory",
				path: payload.terminal.working_directory,
			},
			{ kind: "process_owner", path: payload.terminal.working_directory },
		];
	}

	if (
		payload.type === "artifact.recorded" &&
		(payload.artifact.kind === "file" || payload.artifact.kind === "diff") &&
		payload.artifact.uri
	) {
		return [
			{ kind: "file_artifact", path: payload.artifact.uri },
			...(payload.artifact.kind === "diff"
				? ([{ kind: "file_mutation", path: payload.artifact.uri }] as const)
				: []),
		];
	}

	if (payload.type === "filesystem.mutation") {
		return [
			{ kind: "file_mutation", path: payload.path },
			...(payload.destination_path === undefined
				? []
				: ([{ kind: "file_mutation", path: payload.destination_path }] as const)),
		];
	}

	if (payload.type === "process.ownership") {
		return [{ kind: "process_owner", path: payload.working_directory }];
	}

	if (payload.type === "git.workspace.observed") {
		return [
			{ kind: "git_root", path: payload.root_path },
			{ kind: "git_worktree", path: payload.worktree_path },
			...(payload.branch === undefined
				? []
				: ([{ kind: "git_branch", path: payload.root_path }] as const)),
			...(!payload.has_diff && payload.changed_file_count === 0
				? []
				: ([{ kind: "git_diff", path: payload.root_path }] as const)),
		];
	}

	if (payload.type === "thread.metadata.updated") {
		return (payload.mentioned_projects ?? []).map((project) => ({
			kind: "thread_metadata" as const,
			project,
		}));
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
						"project" in observation
							? `${observation.kind}:project:${observation.project.project_id}`
							: `${observation.kind}:path:${observation.path}`,
						observation,
					]),
				);

				yield* Effect.forEach(unique_observations.values(), (observation) =>
					Effect.gen(function* () {
						const direct_project = "project" in observation;
						const location = direct_project
							? observation.project.root_path
							: observation.path;
						const located = yield* locator
							.Locate(location)
							.pipe(Effect.catch(() => Effect.succeed(Option.none())));

						if (Option.isNone(located)) {
							return;
						}

						if (
							direct_project &&
							located.value.project.project_id !== observation.project.project_id
						) {
							return;
						}

						const kinds = new Set<ProjectAffinityEvidenceKind>([
							observation.kind,
							...(!direct_project && located.value.source === "git_root"
								? (["git_root"] as const)
								: []),
						]);

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
