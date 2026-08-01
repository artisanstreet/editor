import { Context, Data, Effect, Layer, Ref, Semaphore, Stream, SubscriptionRef } from "effect";

import { SnowflakeId, type Project, type ThreadSessionPolicy } from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";
import type { ComposerSubmission } from "../composer/image-attachments";

export interface DraftThreadReady {
	readonly _tag: "Ready";
	readonly policy: ThreadSessionPolicy | undefined;
	readonly project: Project | undefined;
}

export interface DraftThreadCreated {
	readonly command_id: string;
	readonly _tag: "Created";
	readonly policy: ThreadSessionPolicy | undefined;
	readonly policy_applied: boolean;
	readonly project: Project;
	readonly submission: ComposerSubmission;
	readonly thread_id: string;
}

export interface DraftSubmissionClaim extends DraftThreadCreated {
	readonly Complete: Effect.Effect<void>;
	readonly Release: Effect.Effect<void>;
}

export type DraftThreadState =
	| { readonly _tag: "Uninitialized" }
	| DraftThreadReady
	| DraftThreadCreated;

export class DraftProjectRequired extends Data.TaggedError("DraftProjectRequired")<{
	readonly message: string;
}> {}

export class DraftThreadLocked extends Data.TaggedError("DraftThreadLocked")<{
	readonly message: string;
}> {}

export class DraftThreadController extends Context.Service<
	DraftThreadController,
	{
		readonly Changes: Stream.Stream<DraftThreadState>;
		readonly ClaimPendingSubmission: (
			thread_id: string,
		) => Effect.Effect<DraftSubmissionClaim | undefined>;
		readonly AwaitPendingSubmissionClaim: (
			thread_id: string,
		) => Effect.Effect<DraftSubmissionClaim | undefined>;
		readonly Current: Effect.Effect<DraftThreadState>;
		readonly Initialize: (
			project: Project | undefined,
			policy: ThreadSessionPolicy | undefined,
		) => Effect.Effect<void>;
		readonly PendingSubmission: (
			thread_id: string,
		) => Effect.Effect<ComposerSubmission | undefined>;
		readonly Retry: Effect.Effect<DraftThreadCreated, DraftThreadLocked | ArtisanClientError>;
		readonly SelectProject: (project: Project) => Effect.Effect<void, DraftThreadLocked>;
		readonly Submit: (
			submission: ComposerSubmission,
		) => Effect.Effect<DraftThreadCreated, DraftProjectRequired | ArtisanClientError>;
		readonly UpdatePolicy: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<void, DraftThreadLocked>;
	}
>()("Artisan/DraftThreadController") {}

/**
 * Owns the complete pre-creation lifecycle. A successfully created thread is
 * retained until its first message is durably accepted, so retries reuse the
 * same project, policy, thread id, and submission instead of minting a second
 * thread or losing attachment data during navigation.
 */
export const DraftThreadControllerLive = Layer.effect(
	DraftThreadController,
	Effect.gen(function* () {
		const client = yield* ArtisanClient;
		const snowflake_id = yield* SnowflakeId;
		const state = yield* SubscriptionRef.make<DraftThreadState>({
			_tag: "Uninitialized",
		});
		const submit_lock = yield* Semaphore.make(1);
		const active_claim = yield* Ref.make<
			{ readonly claim_id: number; readonly thread_id: string } | undefined
		>(undefined);
		const next_claim_id = yield* Ref.make(0);

		const Initialize = (
			project: Project | undefined,
			policy: ThreadSessionPolicy | undefined,
		) =>
			Effect.gen(function* () {
				yield* SubscriptionRef.update(state, (current) => {
					if (current._tag === "Created") return current;
					if (current._tag === "Uninitialized") {
						return { _tag: "Ready", policy, project } satisfies DraftThreadReady;
					}

					return {
						_tag: "Ready",
						policy: current.policy ?? policy,
						project: current.project ?? project,
					} satisfies DraftThreadReady;
				});
			});

		const SelectProject = (project: Project) =>
			Effect.gen(function* () {
				const current = yield* SubscriptionRef.get(state);
				if (current._tag === "Created") {
					return yield* Effect.fail(
						new DraftThreadLocked({
							message:
								"The first message is pending. Retry it before changing the project.",
						}),
					);
				}
				yield* SubscriptionRef.update(state, (candidate) => {
					return {
						_tag: "Ready",
						policy: candidate._tag === "Ready" ? candidate.policy : undefined,
						project,
					} satisfies DraftThreadReady;
				});
			});

		const UpdatePolicy = (policy: ThreadSessionPolicy) =>
			Effect.gen(function* () {
				const current = yield* SubscriptionRef.get(state);
				if (current._tag === "Created") {
					return yield* Effect.fail(
						new DraftThreadLocked({
							message:
								"The first message is pending. Retry it before changing the model or policy.",
						}),
					);
				}
				yield* SubscriptionRef.update(state, (candidate) => {
					return {
						_tag: "Ready",
						policy,
						project: candidate._tag === "Ready" ? candidate.project : undefined,
					} satisfies DraftThreadReady;
				});
			});

		const ApplyCreatedPolicy = (created: DraftThreadCreated) =>
			Effect.gen(function* () {
				if (created.policy === undefined || created.policy_applied) return created;
				yield* client.UpdateThreadSessionPolicy({
					policy: created.policy,
					thread_id: created.thread_id,
				});
				const applied = { ...created, policy_applied: true } satisfies DraftThreadCreated;
				yield* SubscriptionRef.set(state, applied);
				return applied;
			});

		const SubmitUnlocked = (submission: ComposerSubmission) =>
			Effect.gen(function* () {
				const current = yield* SubscriptionRef.get(state);
				let created: DraftThreadCreated;

				if (current._tag === "Created") {
					created = current;
				} else {
					const project = current._tag === "Ready" ? current.project : undefined;
					if (project === undefined) {
						return yield* Effect.fail(
							new DraftProjectRequired({
								message: "Select a project at the top of the panel before sending.",
							}),
						);
					}

					const created_thread = yield* client.CreateThread({
						project_id: project.project_id,
						title: "New thread",
					});
					created = {
						command_id: yield* snowflake_id.Make("command"),
						_tag: "Created",
						policy: current._tag === "Ready" ? current.policy : undefined,
						policy_applied: false,
						project,
						submission,
						thread_id: created_thread.thread_id,
					};
					yield* SubscriptionRef.set(state, created);
				}

				return yield* ApplyCreatedPolicy(created);
			});

		const Submit = (submission: ComposerSubmission) =>
			Effect.gen(function* () {
				return yield* submit_lock.withPermits(1)(SubmitUnlocked(submission));
			});

		const Retry = Effect.gen(function* () {
			return yield* submit_lock.withPermits(1)(
				Effect.gen(function* () {
					const current = yield* SubscriptionRef.get(state);
					if (current._tag !== "Created") {
						return yield* Effect.fail(
							new DraftThreadLocked({
								message: "There is no retained first message to retry.",
							}),
						);
					}
					return yield* ApplyCreatedPolicy(current);
				}),
			);
		});

		const PendingSubmission = (thread_id: string) =>
			Effect.gen(function* () {
				const current = yield* SubscriptionRef.get(state);
				return current._tag === "Created" && current.thread_id === thread_id
					? current.submission
					: undefined;
			});

		const ClaimPendingSubmission = (thread_id: string) =>
			Effect.gen(function* () {
				return yield* submit_lock.withPermit(
					Effect.gen(function* () {
						const current = yield* SubscriptionRef.get(state);
						const claimed = yield* Ref.get(active_claim);
						if (
							current._tag !== "Created" ||
							current.thread_id !== thread_id ||
							claimed !== undefined
						)
							return undefined;
						const claim_id = yield* Ref.updateAndGet(
							next_claim_id,
							(candidate) => candidate + 1,
						);
						yield* Ref.set(active_claim, { claim_id, thread_id });

						const Release = Effect.gen(function* () {
							yield* submit_lock.withPermit(
								Effect.gen(function* () {
									const active = yield* Ref.get(active_claim);
									if (active?.claim_id !== claim_id) return;
									yield* Ref.set(active_claim, undefined);
								}),
							);
						});
						const Complete = Effect.gen(function* () {
							yield* submit_lock.withPermit(
								Effect.gen(function* () {
									const active = yield* Ref.get(active_claim);
									if (active?.claim_id !== claim_id) return;
									yield* SubscriptionRef.update(state, (candidate) =>
										candidate._tag === "Created" &&
										candidate.thread_id === thread_id
											? ({ _tag: "Uninitialized" } as const)
											: candidate,
									);
									yield* Ref.set(active_claim, undefined);
								}),
							);
						});
						return { ...current, Complete, Release } satisfies DraftSubmissionClaim;
					}),
				);
			});

		/**
		 * Route replacement can briefly overlap the old and new component scopes.
		 * The new scope waits for the old claim's finalizer rather than presenting a
		 * retry action it cannot yet own. The wait remains interruptible by SER when
		 * this route is itself replaced.
		 */
		const AwaitPendingSubmissionClaim = (thread_id: string) =>
			Effect.gen(function* () {
				while ((yield* PendingSubmission(thread_id)) !== undefined) {
					const claim = yield* ClaimPendingSubmission(thread_id);
					if (claim !== undefined) return claim;
					yield* Effect.sleep("10 millis");
				}
				return undefined;
			});

		return DraftThreadController.of({
			AwaitPendingSubmissionClaim,
			ClaimPendingSubmission,
			Changes: SubscriptionRef.changes(state),
			Current: Effect.gen(function* () {
				return yield* SubscriptionRef.get(state);
			}),
			Initialize,
			PendingSubmission,
			Retry,
			SelectProject,
			Submit,
			UpdatePolicy,
		});
	}),
);
