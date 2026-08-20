import {
	Context,
	Data,
	Effect,
	Layer,
	Ref,
	Scope,
	Semaphore,
	Stream,
	SubscriptionRef,
} from "effect";

import { SnowflakeId, type ProjectRef, type ThreadSessionPolicy } from "@artisan/protocol";
import { ArtisanClient, type ArtisanClientError } from "@artisan/transport/client";
import type { ComposerSubmission } from "../composer/image-attachments";

export interface DraftThreadReady {
	readonly _tag: "Ready";
	readonly policy: ThreadSessionPolicy | undefined;
	readonly project: ProjectRef | undefined;
}

export interface DraftThreadCreated {
	readonly command_id: string;
	readonly _tag: "Created";
	readonly policy: ThreadSessionPolicy | undefined;
	readonly project: ProjectRef;
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
		/** Applies a route alignment only while its captured draft revision is current. */
		readonly AlignAtRevision: (
			revision: number,
			project: ProjectRef,
			policy: ThreadSessionPolicy | undefined,
		) => Effect.Effect<boolean>;
		readonly Changes: Stream.Stream<DraftThreadState>;
		readonly CurrentRevision: Effect.Effect<number>;
		readonly AwaitPendingSubmissionClaim: (
			thread_id: string,
		) => Effect.Effect<DraftSubmissionClaim | undefined, never, Scope.Scope>;
		readonly Current: Effect.Effect<DraftThreadState>;
		readonly Initialize: (
			project: ProjectRef | undefined,
			policy: ThreadSessionPolicy | undefined,
		) => Effect.Effect<void>;
		readonly PendingSubmission: (
			thread_id: string,
		) => Effect.Effect<ComposerSubmission | undefined>;
		readonly Retry: Effect.Effect<DraftThreadCreated, DraftThreadLocked | ArtisanClientError>;
		/**
		 * Discards a pre-creation draft so the next route may seed a fresh one.
		 * A retained first submission is never disposable: it must be delivered or
		 * recovered before another draft can take its place.
		 */
		readonly Reset: (discard: Effect.Effect<void>) => Effect.Effect<void, DraftThreadLocked>;
		readonly RevisionChanges: Stream.Stream<number>;
		readonly SelectProject: (project: ProjectRef) => Effect.Effect<void, DraftThreadLocked>;
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
		const revision = yield* SubscriptionRef.make(0);
		const submit_lock = yield* Semaphore.make(1);
		const active_claim = yield* Ref.make<
			{ readonly claim_id: number; readonly thread_id: string } | undefined
		>(undefined);
		const next_claim_id = yield* Ref.make(0);

		const Initialize = (
			project: ProjectRef | undefined,
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

		const SelectProject = (project: ProjectRef) =>
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

		const AlignAtRevision = (
			expected_revision: number,
			project: ProjectRef,
			policy: ThreadSessionPolicy | undefined,
		) =>
			Effect.gen(function* () {
				return yield* submit_lock.withPermits(1)(
					Effect.gen(function* () {
						/**
						 * The guard is against a *stale* alignment: one scheduled before an
						 * explicit reset whose seed policy only finished afterwards, which
						 * must not reinstate what that reset discarded. Callers therefore
						 * have to pass the revision they observed when this attempt began,
						 * not one mirrored through a stream that lags this controller.
						 */
						if ((yield* SubscriptionRef.get(revision)) !== expected_revision)
							return false;
						const current = yield* SubscriptionRef.get(state);
						if (current._tag === "Created") return false;
						yield* SubscriptionRef.set(state, {
							_tag: "Ready",
							policy: current._tag === "Ready" ? (current.policy ?? policy) : policy,
							project,
						});
						return true;
					}),
				);
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
						...(current._tag === "Ready" && current.policy !== undefined
							? { policy: current.policy }
							: {}),
						project_id: project.project_id,
						title: "New thread",
					});
					created = {
						command_id: yield* snowflake_id.Make("command"),
						_tag: "Created",
						policy: current._tag === "Ready" ? current.policy : undefined,
						project,
						submission,
						thread_id: created_thread.thread_id,
					};
					yield* SubscriptionRef.set(state, created);
				}

				return created;
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
					return current;
				}),
			);
		});

		const Reset = (discard: Effect.Effect<void>) =>
			Effect.gen(function* () {
				yield* submit_lock.withPermits(1)(
					Effect.gen(function* () {
						yield* Effect.uninterruptible(
							Effect.gen(function* () {
								const current = yield* SubscriptionRef.get(state);
								if (current._tag === "Created") {
									return yield* Effect.fail(
										new DraftThreadLocked({
											message:
												"The first message is pending. Retry it before starting a fresh draft.",
										}),
									);
								}
								/** Clear the old composer before subscribers are told to remount it. */
								yield* discard;
								yield* SubscriptionRef.set(state, { _tag: "Uninitialized" });
								yield* SubscriptionRef.update(
									revision,
									(current_revision) => current_revision + 1,
								);
							}),
						);
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

		/** Takes the controller lock only; callers must use the scoped waiting boundary below. */
		const TryClaimPendingSubmission = (thread_id: string) =>
			Effect.gen(function* () {
				return yield* submit_lock.withPermit(
					Effect.gen(function* () {
						const current = yield* SubscriptionRef.get(state);
						const claimed = yield* Ref.get(active_claim);
						if (current._tag !== "Created" || current.thread_id !== thread_id)
							return undefined;
						/**
						 * A claim held for some other thread cannot belong to this draft:
						 * the draft is `Created` for `thread_id`, so whatever scope took
						 * that claim is gone and its finalizer never ran. Refusing on it
						 * wedged the draft permanently — every later first message spun
						 * in `AwaitPendingSubmissionClaim` and none was ever sent, which
						 * looked like new threads silently failing to start.
						 */
						if (claimed !== undefined && claimed.thread_id === thread_id)
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
									/**
									 * Delivery having succeeded is a fact about the draft, not
									 * about this claim still being current. A release racing in
									 * first — a route scope closing around an in-flight send —
									 * must not leave the state `Created` after the message
									 * landed: that wedge locked every later new thread behind a
									 * submission that no longer existed to retry.
									 */
									yield* SubscriptionRef.update(state, (candidate) =>
										candidate._tag === "Created" &&
										candidate.thread_id === thread_id
											? ({ _tag: "Uninitialized" } as const)
											: candidate,
									);
									const active = yield* Ref.get(active_claim);
									if (active?.claim_id === claim_id) {
										yield* Ref.set(active_claim, undefined);
									}
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
			Effect.uninterruptibleMask((restore) =>
				Effect.gen(function* () {
					/** Waiting belongs to the route lifetime and must remain interruptible. */
					const claim = yield* restore(
						Effect.gen(function* () {
							while ((yield* PendingSubmission(thread_id)) !== undefined) {
								const candidate = yield* TryClaimPendingSubmission(thread_id);
								if (candidate !== undefined) return candidate;
								yield* Effect.sleep("10 millis");
							}
							return undefined;
						}),
					);
					if (claim === undefined) return undefined;
					/**
					 * A route may be replaced immediately after acquisition. Install its
					 * release in that route's scope before publishing the claim to it, so
					 * interruption cannot strand the retained first submission.
					 */
					yield* Effect.addFinalizer(() => claim.Release);
					return claim;
				}),
			);

		return DraftThreadController.of({
			AlignAtRevision,
			AwaitPendingSubmissionClaim,
			Changes: SubscriptionRef.changes(state),
			CurrentRevision: Effect.gen(function* () {
				return yield* SubscriptionRef.get(revision);
			}),
			Current: Effect.gen(function* () {
				return yield* SubscriptionRef.get(state);
			}),
			Initialize,
			PendingSubmission,
			Reset,
			RevisionChanges: SubscriptionRef.changes(revision),
			Retry,
			SelectProject,
			Submit,
			UpdatePolicy,
		});
	}),
);
