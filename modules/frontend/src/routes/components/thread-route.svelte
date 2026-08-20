<script lang="ts" effect>
	import { navigating, page } from "$app/state";
	import { untrack } from "svelte";
	import { SnowflakeId } from "@artisan/protocol";
	import type {
		ImageAttachmentReference,
		SurfaceUsageAggregate,
		ThreadListItem,
		ThreadOpenSnapshot,
		ThreadSessionPolicy,
		ThreadSessionSnapshot,
		ThreadWorkItem,
	} from "@artisan/protocol";
	import type { ConversationPatch } from "@artisan/protocol";
	import {
		ArtisanClient,
		type ArtisanClientError,
		type ConversationUpdate,
		type ThreadSessionUpdate,
	} from "@artisan/transport/client";
	import {
		ApplyConversationViewPatch,
		CanReplaceConversationSnapshot,
		MakeConversationViewState,
		type ConversationViewState,
	} from "$lib/conversation/store";
	import {
		RunAuthoritativeSubscription,
		RunConversationSubscription,
	} from "$lib/conversation/subscription";
	import { LatestConversationPlan, ThreadChecklist } from "$lib/conversation/checklist";
	import { ThreadOrchestrationRoster, type ThreadAgentInspection } from "$lib/orchestration/service";
	import {
		BuildThreadMessageCommand,
		MakeSubmitGate,
		ObserveAcceptedProjection,
		SubmitDurableCommand,
		ThreadInteractionError,
	} from "$lib/thread-interaction/commands";
	import { ThreadSessionProjection } from "$lib/thread-interaction/session-projection";
	import { ThreadOpenController } from "$lib/thread-interaction/thread-open-controller";
	import { ConversationUserMessageWithSourceReference } from "$lib/conversation/scroll-position";
	import { ConversationSteeringAcknowledged } from "$lib/conversation/steering";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import {
		RunUsageController,
		type RunUsageState,
	} from "$lib/context-usage/run-usage-controller";
	import {
		DraftThreadController,
		type DraftSubmissionClaim,
	} from "$lib/root/draft-thread";
	import {
		CreateBrowserObjectUrl,
		ReleaseBrowserObjectUrl,
	} from "$lib/browser/object-url";
	import { ComposerDraftStore } from "$lib/composer/draft-store";
	import {
		ThreadRouteId,
		ThreadRouteOwnsTarget,
		ThreadRoutePath,
		ThreadRoutePathFor,
	} from "$lib/root/thread-navigation";
	import { WorkspaceCatalogController } from "$lib/root/workspace-catalog-controller";
	import { Clock, Deferred, Effect, Exit, Option, Ref, Scope, Stream } from "effect";
	import ThreadWorkspace from "./thread-workspace.svelte";

	let {
		thread_id: route_thread_id,
		thread_open: route_thread_open,
	}: {
		readonly thread_id: string;
		readonly thread_open: ThreadOpenSnapshot;
	} = $props();
	const route_id = untrack(() => route_thread_id);
	/** The parent keys this component by route and supplies one immutable open aggregate. */
	const thread_open = untrack(() => route_thread_open);

	const client = yield* ArtisanClient;
	const snowflake_id = yield* SnowflakeId;
	const navigation = yield* RouteNavigation;
	const draft_thread = yield* DraftThreadController;
	const composer_drafts = yield* ComposerDraftStore;
	const workspace_catalog = yield* WorkspaceCatalogController;
	const thread_opens = yield* ThreadOpenController;
	const session_projection = yield* ThreadSessionProjection;
	const run_usage = yield* RunUsageController;
	const orchestration = yield* ThreadOrchestrationRoster;
	let inspection = $state.raw<ThreadAgentInspection | undefined>(
		yield* orchestration.CurrentInspection,
	);
	const ApplyInspection = (next: ThreadAgentInspection | undefined) =>
		Effect.gen(function* () {
			inspection = next?.thread_id === route_id ? next : undefined;
		});
	yield* orchestration.InspectionChanges.pipe(
		Stream.runForEach(ApplyInspection),
		Effect.forkScoped,
	);
	const run_usage_lease = yield* run_usage.Acquire(undefined);
	const thread_scope = yield* Scope.make();

	const initial_thread = thread_open.thread;
	const thread_id = initial_thread.thread_id;
	if (thread_id !== route_id && ThreadRouteId(thread_id) !== route_id) {
		yield* Effect.fail(
			new ThreadInteractionError({
				message: `Thread open belongs to ${thread_id}, not ${route_id}.`,
			}),
		);
	}
	const checklist = yield* ThreadChecklist;
	const checklist_lease = yield* checklist.Acquire(thread_id);
	yield* Effect.addFinalizer(checklist_lease.Release);
	/**
	 * Subscription fibers from this route can still be settling while the user
	 * has already moved to another thread — or to the editor surface of this
	 * one — SER keeps the old scope alive until the replacement finishes
	 * rendering. Ownership compares the surface too: a thread-param-only check
	 * let a stale conversation scope pull the user out of `/e/...` for the same
	 * thread on every update, and cancel thread switches mid-navigation.
	 */
	const owning_route = untrack(() => page.route.id);
	const route_owns_thread = () =>
		navigating.to === null
			? ThreadRouteOwnsTarget(
					{ route_id: owning_route, thread_route_id: route_id },
					{ route_id: page.route.id, thread_param: page.params.thread },
				)
			: ThreadRouteOwnsTarget(
					{ route_id: owning_route, thread_route_id: route_id },
					{
						route_id: navigating.to.route.id,
						thread_param: navigating.to.params?.thread,
					},
				);
	const CanonicalizeThreadPath = (candidate: ThreadListItem) =>
		Effect.gen(function* () {
			if (!route_owns_thread()) return;
			const canonical_path = ThreadRoutePathFor(candidate);
			if (page.url.pathname === canonical_path) return;
			yield* navigation.Navigate(canonical_path, {
					keepFocus: true,
					noScroll: true,
					replaceState: true,
				});
		});
	yield* CanonicalizeThreadPath(initial_thread);
	let session = $state.raw<ThreadSessionSnapshot | undefined>(thread_open.session);
	const PublishSession = (next: ThreadSessionSnapshot | undefined) =>
		next === undefined
			? Effect.void
			: session_projection.Publish(next).pipe(Effect.asVoid);
	const ApplySession = (next: ThreadSessionSnapshot) =>
		Effect.gen(function* () {
			if (
				session !== undefined &&
				session.journal_sequence > next.journal_sequence
			)
				return session;
			session = next;
			yield* PublishSession(next);
			return next;
		});
	yield* ApplySession(thread_open.session);
	let thread = $state.raw<ThreadListItem | undefined>(initial_thread);
	let work = $state.raw<ThreadWorkItem | undefined>(thread_open.work);
	const ApplyCatalog = (catalog: {
		readonly threads: ReadonlyArray<ThreadListItem>;
		readonly threads_loaded: boolean;
	}) =>
		Effect.gen(function* () {
			if (!catalog.threads_loaded) return;
			const next_thread = catalog.threads.find((candidate) => candidate.thread_id === thread_id);
			/**
			 * A loaded catalog without this thread has not caught up with it yet:
			 * the route was opened from an authoritative thread-open snapshot, so
			 * absence here is projection lag, never deletion. Replacing the known
			 * thread with nothing dropped `primary_project` out from under the
			 * first-message delivery of a just-created thread, which failed its
			 * project check mid-handoff and released the retained submission.
			 */
			if (next_thread === undefined) return;
			thread = next_thread;
			yield* CanonicalizeThreadPath(next_thread);
		});
	yield* Effect.forkIn(
		workspace_catalog.Changes.pipe(Stream.runForEach(ApplyCatalog)),
		thread_scope,
	);
	const run_active = $derived(
		work?.status === "queued" || work?.status === "running" || work?.status === "waiting",
	);
	let context_usage = $state.raw<SurfaceUsageAggregate | undefined>(undefined);
	/**
	 * A reading in flight is not the absence of a reading.
	 *
	 * Sending a message starts a run, which selects a new run's usage and puts
	 * the controller into `Loading` until that run first reports. Clearing on
	 * anything that was not `Ready` blanked the gauge for exactly the stretch
	 * where the reader is watching it — from pressing send until the model
	 * answers — and then brought it back, which read as the gauge flickering
	 * out rather than as a number being fetched.
	 *
	 * The previous aggregate stays on screen through that gap because it is
	 * still true: it is what the context was as of the last run that reported,
	 * and the gauge already refuses to draw a reading whose engine or model no
	 * longer matches the thread. Only a state that means no reading applies —
	 * no run selected, or a run that cannot report — takes it down.
	 */
	const ApplyRunUsage = (state: RunUsageState) =>
		Effect.gen(function* () {
			if (state._tag === "Ready") {
				context_usage = state.aggregate;
				return;
			}
			if (state._tag === "Loading") return;
			context_usage = undefined;
		});
	yield* run_usage.Changes.pipe(
		Stream.runForEach(ApplyRunUsage),
		Effect.forkScoped,
	);
	yield* run_usage_lease.Select(work?.run_id);
	yield* Effect.addFinalizer(run_usage_lease.Release);
	const initial_snapshot = thread_open.conversation;
	if (initial_snapshot.thread_id !== thread_id) {
		yield* Effect.fail(
			new ThreadInteractionError({
				message: `Conversation snapshot belongs to ${initial_snapshot.thread_id}, not ${thread_id}.`,
			}),
		);
	}
	const conversation_id = initial_snapshot.conversation_id;
	let snapshot = $state.raw(initial_snapshot);
	type AcceptedProjectionWaiter = {
		readonly deferred: Deferred.Deferred<void>;
		readonly listeners: number;
		/** What this waiter is waiting for, evaluated against every new snapshot. */
		readonly Satisfied: (candidate: typeof snapshot) => boolean;
	};
	const accepted_projection_waiters = yield* Ref.make(
		new Map<string, AcceptedProjectionWaiter>(),
	);
	const HasAcceptedUserMessage = (candidate: typeof snapshot, command_id: string) =>
		Option.isSome(ConversationUserMessageWithSourceReference(candidate.items, command_id));
	const ReleaseAcceptedProjectionWaiter = (key: string, deferred: Deferred.Deferred<void>) =>
		Ref.update(accepted_projection_waiters, (current) => {
			const existing = current.get(key);
			if (existing === undefined || existing.deferred !== deferred) return current;
			const next = new Map(current);
			if (existing.listeners === 1) next.delete(key);
			else next.set(key, { ...existing, listeners: existing.listeners - 1 });
			return next;
		});
	/**
	 * The subscription owns projection authority. A receipt waits on its next
	 * matching snapshot without issuing another control request, while a shared
	 * waiter keeps concurrent callers from racing the same acknowledgement.
	 */
	/**
	 * How long a projection waiter will hold before giving up on evidence that
	 * may never arrive.
	 *
	 * These waiters are satisfied by a conversation item appearing, which is
	 * something the renderer observes rather than performs. If the command that
	 * would produce it never lands — the send was lost, the run ended without
	 * echoing it, the stream stalled — the condition can never become true and
	 * the wait is permanent. That is not hypothetical: a steer whose message
	 * never reached Forge left the composer holding its pending lip forever, so
	 * the steering label stayed in the transcript with no error and no way back
	 * except restarting the app.
	 *
	 * Callers treat expiry as a failure, which releases what they were holding.
	 * The budget is long enough that a slow but working projection still wins.
	 */
	const projection_waiter_deadline = "45 seconds";

	const AwaitProjection = (key: string, Satisfied: (candidate: typeof snapshot) => boolean) =>
		Effect.uninterruptibleMask((restore) =>
			Effect.gen(function* () {
				const fresh = yield* Deferred.make<void>();
				const claimed = yield* Ref.modify(accepted_projection_waiters, (current) => {
					if (Satisfied(snapshot)) {
						return [{ _tag: "observed" } as const, current] as const;
					}
					const existing = current.get(key);
					const next = new Map(current);
					if (existing !== undefined) {
						next.set(key, { ...existing, listeners: existing.listeners + 1 });
						return [
							{ _tag: "waiting", deferred: existing.deferred } as const,
							next,
						] as const;
					}
					next.set(key, { deferred: fresh, listeners: 1, Satisfied });
					return [{ _tag: "waiting", deferred: fresh } as const, next] as const;
				});
				if (claimed._tag === "observed") return;
				yield* restore(Deferred.await(claimed.deferred)).pipe(
					Effect.timeoutOrElse({
						duration: projection_waiter_deadline,
						orElse: () =>
							Effect.fail(
								new ThreadInteractionError({
									message: "Artisan never saw this reach the conversation.",
								}),
							),
					}),
					Effect.ensuring(ReleaseAcceptedProjectionWaiter(key, claimed.deferred)),
				);
			}),
		);
	/**
	 * The queued boundary of a steer: the send stops being a held draft the
	 * moment its canonical message projects, and the composer's queued lip
	 * yields to the transcript there.
	 */
	const AwaitCanonicalUserMessage = (command_id: string) =>
		AwaitProjection(`user_message:${command_id}`, (candidate) =>
			HasAcceptedUserMessage(candidate, command_id),
		);
	/**
	 * The "Steering" label belongs to the window the user is actually waiting
	 * through: from submitting the steer to the engine taking it up. Settling it
	 * on Artisan's own echo of the message instead made it flash for the few
	 * milliseconds a local Forge needs to project a user turn.
	 */
	const AwaitSteeringAcknowledged = (command_id: string) =>
		AwaitProjection(`steering:${command_id}`, (candidate) =>
			ConversationSteeringAcknowledged(candidate.items, candidate.turns, command_id),
		);
	const ResolveAcceptedProjectionWaiters = (candidate: typeof snapshot) =>
		Effect.gen(function* () {
			const resolved = yield* Ref.modify(accepted_projection_waiters, (current) => {
				const next = new Map(current);
				const deferreds: Array<Deferred.Deferred<void>> = [];
				for (const [key, waiter] of current) {
					if (!waiter.Satisfied(candidate)) continue;
					next.delete(key);
					deferreds.push(waiter.deferred);
				}
				return [deferreds, next] as const;
			});
			yield* Effect.forEach(resolved, (deferred) => Deferred.succeed(deferred, undefined), {
				discard: true,
			});
		});
	yield* Effect.addFinalizer(
		Effect.gen(function* () {
			if (session === undefined) return;
			yield* thread_opens.Publish({
				conversation: snapshot,
				session,
				thread: thread ?? initial_thread,
				...(work === undefined ? {} : { work }),
			});
		}),
	);
	/**
	 * The route already maintains the canonical identity map while applying live
	 * patches. Keep it reactive and hand the same structure to the renderer
	 * instead of rebuilding a second full-history map for every streamed patch.
	 */
	let view_state = $state.raw<ConversationViewState | undefined>();
	let image_sources = $state.raw<ReadonlyMap<string, string>>(new Map());
	const requested_image_ids = new Set<string>();
	const visible_image_ids = new Set<string>();
	const image_load_attempts = new Map<string, number>();
	/** The request gate must never outlive the fiber that claimed it. */
	const ClearImageLoadState = (attachment_id: string) =>
		Effect.gen(function* () {
			requested_image_ids.delete(attachment_id);
			image_load_attempts.delete(attachment_id);
		});

	const ReleaseImageAttachment = (attachment_id: string) =>
		Effect.gen(function* () {
			const source = image_sources.get(attachment_id);
			if (source === undefined) return;
			yield* ReleaseBrowserObjectUrl(source).pipe(Effect.ignore);
			const next_sources = new Map(image_sources);
			next_sources.delete(attachment_id);
			image_sources = next_sources;
		});
	/** Replacing a URL transfers ownership and promptly releases the superseded blob. */
	const PublishImageAttachment = (attachment_id: string, source: string) =>
		Effect.gen(function* () {
			const previous_source = image_sources.get(attachment_id);
			image_sources = new Map(image_sources).set(attachment_id, source);
			if (previous_source !== undefined && previous_source !== source) {
				yield* ReleaseBrowserObjectUrl(previous_source).pipe(Effect.ignore);
			}
		});
	/**
	 * One route finalizer owns the currently retained URLs. Per-image finalizers
	 * accumulated one closure for every attachment ever viewed even after its URL
	 * had already been revoked on invisibility.
	 */
	const ReleaseAllImageAttachments = Effect.gen(function* () {
		const sources = [...image_sources.values()];
		image_sources = new Map();
		for (const source of sources) {
			yield* ReleaseBrowserObjectUrl(source).pipe(Effect.ignore);
		}
	});
	yield* Effect.addFinalizer(() =>
		Effect.gen(function* () {
			/** Interrupt attachment loads before releasing the URLs they published. */
			yield* Scope.close(thread_scope, Exit.void);
			yield* ReleaseAllImageAttachments;
		}),
	);

	const LoadImageAttachment = (attachment: ImageAttachmentReference, attempt: number) =>
		Effect.gen(function* () {
			const result = yield* client.GetMessageImageAttachment({
				attachment_id: attachment.id,
				thread_id,
			});
			if (Option.isNone(result) || !visible_image_ids.has(attachment.id)) {
				yield* ClearImageLoadState(attachment.id);
				return;
			}

			yield* Effect.gen(function* () {
				const bytes = Uint8Array.from(result.value.bytes);
				const source = yield* CreateBrowserObjectUrl(bytes, result.value.media_type);
				if (!visible_image_ids.has(attachment.id)) {
					yield* ClearImageLoadState(attachment.id);
					yield* ReleaseBrowserObjectUrl(source).pipe(Effect.ignore);
					return;
				}
				yield* PublishImageAttachment(attachment.id, source);
				yield* ClearImageLoadState(attachment.id);
			}).pipe(Effect.uninterruptible);
		}).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					if (!visible_image_ids.has(attachment.id) || attempt >= 3) {
						yield* ClearImageLoadState(attachment.id);
						return;
					}
					yield* Effect.sleep(attempt * 500);
					/** Let the next attempt increment the retained retry count. */
					requested_image_ids.delete(attachment.id);
					if (!visible_image_ids.has(attachment.id)) {
						yield* ClearImageLoadState(attachment.id);
						return;
					}
					yield* RequestImageAttachment(attachment);
				}),
			),
			/** Interruption is not an Effect failure, so cleanup belongs in a finalizer. */
			Effect.ensuring(ClearImageLoadState(attachment.id)),
		);

	const RequestImageAttachment = (attachment: ImageAttachmentReference) =>
		Effect.gen(function* () {
			if (
				image_sources.has(attachment.id) ||
				requested_image_ids.has(attachment.id)
			)
				return;
			requested_image_ids.add(attachment.id);
			const attempt = (image_load_attempts.get(attachment.id) ?? 0) + 1;
			image_load_attempts.set(attachment.id, attempt);

			yield* LoadImageAttachment(attachment, attempt);
		});
	/** Mark the whole visible group before admitting its independent IPC loads. */
	const LoadVisibleImageAttachments = (attachments: ReadonlyArray<ImageAttachmentReference>) =>
		Effect.gen(function* () {
			for (const attachment of attachments) visible_image_ids.add(attachment.id);
			yield* Effect.forEach(attachments, RequestImageAttachment, {
				concurrency: 4,
				discard: true,
			});
		});
	/** Hiding keeps deterministic URL revocation while each visible group remains scoped. */
	const HideImageAttachments = (attachments: ReadonlyArray<ImageAttachmentReference>) =>
		Effect.gen(function* () {
			for (const attachment of attachments) {
				visible_image_ids.delete(attachment.id);
				yield* ClearImageLoadState(attachment.id);
				yield* ReleaseImageAttachment(attachment.id);
			}
		});

	const UpdateImageAttachmentVisibility = (
		attachments: ReadonlyArray<ImageAttachmentReference>,
		visible: boolean,
	) =>
		Effect.gen(function* () {
			if (visible) return yield* LoadVisibleImageAttachments(attachments);
			yield* HideImageAttachments(attachments);
		});

	const ReplaceSnapshot = (next: typeof snapshot) =>
		Effect.gen(function* () {
			if (!CanReplaceConversationSnapshot(snapshot, next)) return;
			const initialized = MakeConversationViewState(next);
			snapshot = next;
			view_state = initialized._tag === "applied" ? initialized.state : undefined;
			yield* ResolveAcceptedProjectionWaiters(next);
			yield* checklist_lease.Publish(LatestConversationPlan(next.items, next.turns));
		});

	yield* ReplaceSnapshot(snapshot);

	/**
	 * A resync is asked for because the caller just saw something it could not
	 * place, so it must never be answered by a fetch that began before it asked.
	 *
	 * Patches are retained as a bounded window, so a client that falls behind it
	 * can only recover by snapshot; adopting an in-flight snapshot taken before
	 * the gap appeared leaves that gap exactly where it was. `ApplyUpdate` then
	 * spends both of its attempts on the same stale answer and drops the batch,
	 * and the transcript stops moving while the run carries on — which is what a
	 * reload was fixing, because it rebuilt from a genuinely fresh read.
	 */
	const conversation_resync_in_flight = yield* Ref.make<
		Deferred.Deferred<void, ArtisanClientError> | undefined
	>(undefined);
	const conversation_resync_queued = yield* Ref.make<
		Deferred.Deferred<void, ArtisanClientError> | undefined
	>(undefined);
	const ResyncOnce = Effect.gen(function* () {
		yield* ReplaceSnapshot(yield* client.GetConversation({ thread_id }));
	});
	const CompleteResync = (
		deferred: Deferred.Deferred<void, ArtisanClientError>,
	): Effect.Effect<void> =>
		ResyncOnce.pipe(
			Effect.exit,
			Effect.flatMap((exit) =>
				Effect.gen(function* () {
					const successor = yield* Ref.getAndSet(
						conversation_resync_queued,
						undefined,
					);
					yield* Ref.set(conversation_resync_in_flight, successor);
					yield* Deferred.done(deferred, exit);
					if (successor !== undefined) {
						yield* Effect.forkIn(CompleteResync(successor), thread_scope);
					}
				}),
			),
		);
	const Resync = Effect.gen(function* () {
		const deferred = yield* Deferred.make<void, ArtisanClientError>();
		const claimed = yield* Ref.modify(conversation_resync_in_flight, (current) =>
			current === undefined ? [deferred, deferred] : [current, current],
		);
		if (claimed === deferred) {
			yield* Effect.forkIn(CompleteResync(deferred), thread_scope);
			return yield* Deferred.await(deferred);
		}
		const queued = yield* Ref.modify(conversation_resync_queued, (current) =>
			current === undefined ? [deferred, deferred] : [current, current],
		);
		return yield* Deferred.await(queued);
	});

	/**
	 * Session and thread metadata arrive through their retained projection
	 * streams. Command receipts, lifecycle events, and terminal conversation
	 * patches therefore only need the small work projection, and overlapping
	 * triggers join one route-owned request.
	 */
	/**
	 * Overlapping triggers join one request, but only a request that has not
	 * started reading yet.
	 *
	 * Adopting the in-flight read instead would answer with state observed
	 * before the caller asked. Every trigger here reports something that just
	 * changed — a receipt, a lifecycle event, a turn reaching a terminal
	 * patch — and during a run they arrive constantly, so the settle almost
	 * always lands while an earlier read is open. Handing it that read's
	 * `running` answer left the composer offering to stop a run that had
	 * already finished, with no later read to correct it.
	 */
	const interaction_refresh_in_flight = yield* Ref.make<
		Deferred.Deferred<void, ArtisanClientError> | undefined
	>(undefined);
	const interaction_refresh_queued = yield* Ref.make<
		Deferred.Deferred<void, ArtisanClientError> | undefined
	>(undefined);
	const RefreshInteractionContextOnce = Effect.gen(function* () {
		const next_work = yield* client.GetThreadWork(thread_id);
		work = Option.getOrUndefined(next_work);
		yield* run_usage_lease.Select(work?.run_id);
	});
	const CompleteInteractionRefresh = (
		deferred: Deferred.Deferred<void, ArtisanClientError>,
	): Effect.Effect<void> =>
		RefreshInteractionContextOnce.pipe(
			Effect.exit,
			Effect.flatMap((exit) =>
				Effect.gen(function* () {
					/** Whoever asked during this read is owed one of their own. */
					const successor = yield* Ref.getAndSet(
						interaction_refresh_queued,
						undefined,
					);
					yield* Ref.set(interaction_refresh_in_flight, successor);
					yield* Deferred.done(deferred, exit);
					if (successor !== undefined) {
						yield* Effect.forkIn(
							CompleteInteractionRefresh(successor),
							thread_scope,
						);
					}
				}),
			),
		);
	const RefreshInteractionContext = Effect.gen(function* () {
		const deferred = yield* Deferred.make<void, ArtisanClientError>();
		const claimed = yield* Ref.modify(interaction_refresh_in_flight, (current) =>
			current === undefined ? [deferred, deferred] : [current, current],
		);
		if (claimed === deferred) {
			yield* Effect.forkIn(CompleteInteractionRefresh(deferred), thread_scope);
			return yield* Deferred.await(deferred);
		}
		/** A read is open; queue one that begins after it and await that. */
		const queued = yield* Ref.modify(interaction_refresh_queued, (current) =>
			current === undefined ? [deferred, deferred] : [current, current],
		);
		return yield* Deferred.await(queued);
	});
	/**
	 * Durable command receipts are the foreground completion boundary. The
	 * retained projections remain authoritative, while this route-owned fork
	 * makes a best-effort work read available sooner without holding controls.
	 */
	const ScheduleInteractionRefresh = RefreshInteractionContext.pipe(
		Effect.ignore,
		Effect.forkIn(thread_scope),
		Effect.asVoid,
	);
	const ReconcileConversationAndInteraction = Effect.all(
		[Resync, RefreshInteractionContext],
		{ concurrency: "unbounded", discard: true },
	);
	const ScheduleConversationAndInteractionReconciliation =
		ReconcileConversationAndInteraction.pipe(
			Effect.ignore,
			Effect.forkIn(thread_scope),
			Effect.asVoid,
		);
	const RefreshSession = client.GetThreadSession(thread_id).pipe(
		Effect.flatMap(ApplySession),
		Effect.asVoid,
	);
	const ApplySessionUpdate = (update: ThreadSessionUpdate) =>
		ApplySession(update.snapshot).pipe(Effect.asVoid);

	/**
	 * Decides whether a failed send actually failed.
	 *
	 * A rejected command is a decision and stands. A retryable one — a deadline
	 * passing, a socket dropping mid-flight — decides nothing: Forge may have
	 * accepted and journalled the message already, and reporting that as a
	 * failure is what left the composer offering to resend a message the
	 * transcript had. The transcript is the authority, so it is asked: a user
	 * message carrying this command's own id means the send landed, whatever the
	 * request did. Only a genuinely absent message keeps the failure.
	 */
	const RecoverAcceptedSend = (command_id: string, expects_user_message: boolean) =>
		Effect.catchIf(
			(error: ArtisanClientError) => error.retryable && expects_user_message,
			(error: ArtisanClientError) =>
				Effect.gen(function* () {
					const accepted = yield* ObserveAcceptedProjection(
						client.GetConversation({ thread_id }),
						(candidate) => HasAcceptedUserMessage(candidate, command_id),
					);
					if (Option.isNone(accepted)) return yield* Effect.fail(error);
					yield* ReplaceSnapshot(accepted.value);
					return {
						command_id,
						journal_sequence: accepted.value.last_patch_sequence,
						status: "accepted" as const,
					};
				}),
		);

	const SendMessage = (submission: ComposerSubmission, command_id?: string) =>
		Effect.gen(function* () {
			if (session === undefined) {
				return yield* Effect.fail(
					new ThreadInteractionError({ message: "Thread session context is still loading." }),
				);
			}

			const result = BuildThreadMessageCommand({ session, thread, thread_id, work }, submission);
			if (result._tag === "invalid") return yield* Effect.fail(result.error);
			const expects_user_message = result.command.payload.type === "thread.send_message";
			const is_steering = result.command.run_id !== undefined;
			/**
			 * Minted here rather than left to the transport so this route can still
			 * name the command after the request fails. Without a name of its own, a
			 * send that Forge accepted but did not answer in time is unfindable, and
			 * the only honest thing left to report is that it failed.
			 */
			const submitted_command_id = command_id ?? (yield* snowflake_id.Make("command"));

			/**
			 * Command acceptance is the durable submission boundary. The stream may
			 * still be establishing, so the composer separately subscribes to the
			 * route-owned acknowledgement waiter. Work reconciliation is best-effort
			 * background work; it cannot delay or duplicate this durable submission.
			 */
			const sent = client.Command({ ...result.command, command_id: submitted_command_id });
			const receipt = yield* SubmitDurableCommand(
				sent.pipe(RecoverAcceptedSend(submitted_command_id, expects_user_message)),
				() => RefreshInteractionContext.pipe(Effect.forkIn(thread_scope), Effect.asVoid),
			);
			return {
				expects_user_message,
				...(expects_user_message
					? { user_message_reference: receipt.command_id }
					: {}),
				...(expects_user_message && is_steering
					? {
							steering_echo: AwaitCanonicalUserMessage(receipt.command_id),
							steering_settlement: AwaitSteeringAcknowledged(receipt.command_id),
						}
					: {}),
			};
		});

	/**
	 * Recalls a queued steer while it is still Forge's to give back. Not a
	 * durable submission: a refused withdrawal decides nothing — the steer
	 * simply proceeds — so it takes no receipt recovery.
	 */
	const WithdrawQueuedMessage = (command_id: string) =>
		client
			.Command({ payload: { command_id, type: "thread.withdraw_message" }, thread_id })
			.pipe(Effect.asVoid);

	const UpdateSessionPolicy = (policy: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			const receipt = yield* client.UpdateThreadSessionPolicy({ policy, thread_id });
			if (session === undefined) {
				return yield* Effect.fail(
					new ThreadInteractionError({
						message: "Thread session context is still loading.",
					}),
				);
			}
			const accepted =
				session.journal_sequence >= receipt.journal_sequence
					? session
					: {
							...session,
							journal_sequence: receipt.journal_sequence,
							policy,
						};
			return (yield* ApplySession(accepted)).policy;
		});

	const PersistSessionPolicy = (policy: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			return yield* UpdateSessionPolicy(policy);
		});

	const RespondApproval = (approval_id: string, approved: boolean) =>
		SubmitDurableCommand(
			client.Command({
				payload: { approval_id, approved, type: "run.respond_approval" },
				thread_id,
			}),
			() => ScheduleInteractionRefresh,
		).pipe(Effect.asVoid);

	/**
	 * A usage interruption is a durable recovery decision, not a retry of the
	 * failed provider attempt. The revision makes competing renderer tabs and
	 * Forge's scheduler converge on one winner; any rejection is reconciled from
	 * the authoritative conversation rather than retried optimistically.
	 */
	const ResolveUsageInterruption = (
		interruption_id: string,
		expected_revision: number,
		action:
			| { readonly type: "set_auto_continue"; readonly enabled: boolean }
			| {
					readonly type: "continue";
					readonly target_engine_id: string;
					readonly target_model_id?: string;
			  }
			| { readonly type: "cancel" },
	) =>
		Effect.gen(function* () {
			return yield* SubmitDurableCommand(
				client.Command({
					payload: {
						action,
						expected_revision,
						interruption_id,
						type: "usage.interruption.resolve",
					},
					thread_id,
				}).pipe(
					Effect.catch((error) =>
						Effect.gen(function* () {
							/** A stale revision is expected under multi-client/scheduler races. */
							yield* ScheduleConversationAndInteractionReconciliation;
							return yield* Effect.fail(error);
						}),
					),
				),
				() => ScheduleConversationAndInteractionReconciliation,
			);
		});

	const CancelRun = () =>
		SubmitDurableCommand(
			client.Command({ payload: { type: "run.cancel" }, thread_id }),
			() => ScheduleInteractionRefresh,
		).pipe(Effect.asVoid);

	/** Replays one exact failed request without manufacturing another user turn. */
	const RetryRun = (run_id: string) =>
		SubmitDurableCommand(
			client.Command({ payload: { run_id, type: "run.retry" }, thread_id }),
			() => RefreshInteractionContext,
		).pipe(Effect.asVoid);

	/**
	 * Carries a draft typed during an active run into a brand-new thread in the
	 * same project, reusing the draft-thread pipeline so the submission is
	 * retained until the routed thread durably delivers it. The stored draft is
	 * cleared before navigating because the composer that would normally clear
	 * it is destroyed by the route change.
	 */
	const StartThreadWithPrompt = (submission: ComposerSubmission) =>
		Effect.gen(function* () {
			const project_ref = thread?.primary_project;
			if (project_ref === undefined) {
				return yield* Effect.fail(
					new ThreadInteractionError({
						message: "This thread has no project to open a new thread in.",
					}),
				);
			}
			yield* draft_thread.SelectProject(project_ref);
			const policy = session?.policy;
			if (policy !== undefined) yield* draft_thread.UpdatePolicy(policy);
			const created = yield* draft_thread.Submit(submission);
			yield* composer_drafts.Clear(thread_id);
			yield* navigation.Navigate(
				ThreadRoutePath(created.project.project_id, created.thread_id),
			);
		});

	const RunCommand = (payload: {
		readonly type: "run.respond_question";
		readonly answers: Record<string, [string, ...string[]]>;
	}) =>
		SubmitDurableCommand(client.Command({ payload, thread_id }), () => ScheduleInteractionRefresh).pipe(
			Effect.asVoid,
		);

	/**
	 * Turn outcomes after which no further work lands, whatever the outcome.
	 * `interrupted` belongs here: nothing more arrives on its own, and the resume
	 * that could revive it is an explicit act rather than work already in flight.
	 */
	const settled_lifecycles = new Set(["completed", "failed", "interrupted", "cancelled"]);
	const PatchSettlesTurn = (patch: ConversationPatch) =>
		(patch.type === "turn_lifecycle" && settled_lifecycles.has(patch.lifecycle)) ||
		(patch.type === "turn_upsert" && settled_lifecycles.has(patch.turn.lifecycle));

	/**
	 * A conversation stream that has gone silent is indistinguishable from a
	 * healthy idle one: the runner only recovers on stream end or error, and
	 * every backend delivery loss seen so far (a skipped journal window, a
	 * starved projection phase) presents as exactly this silence while the
	 * connection stays protocol-healthy. While durable work reports an active
	 * run the transcript is expected to move, so a silent stream is probed
	 * with one reconciliation per interval instead of freezing until reload.
	 */
	const conversation_liveness_interval_ms = 30_000;
	let last_conversation_delivery_ms = yield* Clock.currentTimeMillis;

	const ApplyUpdate = (update: ConversationUpdate) =>
		Effect.gen(function* () {
			last_conversation_delivery_ms = yield* Clock.currentTimeMillis;
			if (update.type === "snapshot") {
				yield* ReplaceSnapshot(update.snapshot);
				return;
			}
			for (let attempt = 0; attempt < 2; attempt += 1) {
				if (
					update.batch.thread_id !== thread_id ||
					update.batch.conversation_id !== conversation_id
				) {
					yield* Resync;
					return;
				}
				if (view_state === undefined) yield* Resync;

				if (snapshot.last_patch_sequence >= update.batch.to_sequence) return;
				const applicable = update.batch.patches.filter(
					(patch) => patch.sequence > snapshot.last_patch_sequence,
				);
				if (
					view_state === undefined ||
					applicable[0]?.sequence !== snapshot.last_patch_sequence + 1
				) {
					yield* Resync;
					continue;
				}

				let failed = false;
				for (const patch of applicable) {
					const result = ApplyConversationViewPatch(view_state, patch);
					if (result._tag === "resync_required" || result._tag === "invariant_error") {
						failed = true;
						break;
					}
					view_state = result.state;
				}
				if (
					failed ||
					view_state.rebuild.snapshot.last_patch_sequence !== update.batch.to_sequence
				) {
					yield* Resync;
					continue;
				}
				snapshot = view_state.rebuild.snapshot;
				yield* ResolveAcceptedProjectionWaiters(snapshot);
				yield* checklist_lease.Publish(
					LatestConversationPlan(snapshot.items, snapshot.turns),
				);
				/**
				 * A run reaching a terminal state is only ever announced through the
				 * projection. Without re-reading the durable work item here the
				 * transcript shows the turn as finished while `work` still reports
				 * it running — which leaves the composer stuck offering to stop a
				 * run that already ended.
				 */
				if (applicable.some(PatchSettlesTurn)) yield* RefreshInteractionContext;
				return;
			}
			/**
			 * Both attempts were spent without placing the batch. Falling out of
			 * the loop here used to drop it in silence, and because a dropped
			 * update is not a failure the subscription stayed healthy and never
			 * retried — the transcript simply stopped at whatever it was showing
			 * until the window was reloaded. A final resync converges on the
			 * durable conversation instead of freezing on a stale one.
			 */
			yield* Effect.logWarning("Conversation update could not be applied", {
				conversation_id,
				from_sequence: update.batch.from_sequence,
				thread_id,
				to_sequence: update.batch.to_sequence,
			});
			yield* Resync;
			yield* RefreshInteractionContext;
		});

	yield* Effect.forkIn(
		RunConversationSubscription(
			client.SubscribeConversation(thread_id, {
				conversation_id,
				last_patch_sequence: snapshot.last_patch_sequence,
			}),
			ApplyUpdate,
			Resync,
		),
		thread_scope,
	);
	yield* Effect.forkIn(
		RunAuthoritativeSubscription(
			client.SubscribeThreadSession(thread_id),
			ApplySessionUpdate,
			RefreshSession,
		),
		thread_scope,
	);
	yield* Effect.forkIn(
		RunAuthoritativeSubscription(
			Effect.gen(function* () {
				return client.Events.pipe(
					Stream.filter(
						(event) =>
							event.thread_id === thread_id &&
							(event.payload.type === "run.lifecycle" ||
								event.payload.type === "thread.erased"),
					),
					Stream.debounce("50 millis"),
				);
			}),
			() => RefreshInteractionContext,
			RefreshInteractionContext,
		),
		thread_scope,
	);
	const WatchConversationLiveness = Effect.gen(function* () {
		for (;;) {
			yield* Effect.sleep(conversation_liveness_interval_ms);
			const now = yield* Clock.currentTimeMillis;
			/** An idle thread owes no deliveries; keep the probe armed from run start. */
			if (work === undefined) {
				last_conversation_delivery_ms = now;
				continue;
			}
			const silent_ms = now - last_conversation_delivery_ms;
			if (silent_ms < conversation_liveness_interval_ms) continue;
			last_conversation_delivery_ms = now;
			yield* Effect.logWarning(
				"Conversation stream is silent during an active run; reconciling",
				{ silent_ms, thread_id },
			);
			yield* ReconcileConversationAndInteraction.pipe(Effect.ignore);
		}
	});
	yield* Effect.forkIn(WatchConversationLiveness, thread_scope);

	const RespondQuestion = (question_id: string, answers: ReadonlyArray<string>) =>
		Effect.gen(function* () {
			const [first, ...rest] = answers;
			/** An empty answer is not a decision; nothing is dispatched for one. */
			if (first === undefined) return;
			yield* RunCommand({
				answers: { [question_id]: [first, ...rest] },
				type: "run.respond_question",
			}).pipe(
				Effect.catch((error) =>
					Effect.gen(function* () {
						return yield* Effect.fail(error);
					}),
				),
			);
		});

	/**
	 * A thread reached from the draft route was created by its first submission;
	 * send that message through the normal durable pipeline exactly once.
	 */
	let pending_first_submission = $state.raw<DraftSubmissionClaim | undefined>(undefined);
	/**
	 * Retained past delivery: the workspace seeds its sent-turn anchor from this
	 * at mount, and the claim itself is released long before the message it
	 * names is projected into the conversation.
	 */
	let first_submission_reference = $state<string | undefined>(undefined);
	let pending_first_submission_error = $state<string | undefined>(undefined);
	let first_submission_attempting = $state(false);
	let first_submission_blocked = $state(false);
	const first_submission_gate = yield* MakeSubmitGate;
	/** The controller returns only claims already protected by this route's scope. */
	const ClaimPendingFirstSubmission = Effect.gen(function* () {
		const claim = yield* draft_thread.AwaitPendingSubmissionClaim(thread_id);
		pending_first_submission = claim;
		if (claim !== undefined) first_submission_reference = claim.command_id;
		first_submission_blocked = claim !== undefined;
		return claim;
	});
	const DeliverPendingFirstSubmission = Effect.gen(function* () {
		if (pending_first_submission === undefined) yield* ClaimPendingFirstSubmission;
		const claimed = pending_first_submission;
		if (claimed === undefined) return;
		yield* SendMessage(claimed.submission, claimed.command_id);
		yield* claimed.Complete;
		pending_first_submission = undefined;
		first_submission_blocked = false;
	});
	const DeliverClaimedFirstSubmission = DeliverPendingFirstSubmission.pipe(
		Effect.catch((error) =>
			Effect.gen(function* () {
				const claimed = pending_first_submission;
				if (claimed !== undefined) yield* claimed.Release;
				pending_first_submission = undefined;
				pending_first_submission_error = error.message;
				first_submission_blocked = true;
			}),
		),
	);
	const FinishFirstSubmissionRetry = Effect.gen(function* () {
		first_submission_attempting = false;
		yield* first_submission_gate.Release;
	});
	const ClaimAndDeliverFirstSubmissionRetry = Effect.gen(function* () {
		yield* ClaimPendingFirstSubmission;
		if (pending_first_submission === undefined) return;
		yield* DeliverClaimedFirstSubmission;
	}).pipe(Effect.ensuring(FinishFirstSubmissionRetry));
	const RetryPendingFirstSubmission = Effect.gen(function* () {
		if (!(yield* first_submission_gate.Acquire)) return;
		first_submission_attempting = true;
		pending_first_submission_error = undefined;
		yield* Effect.forkIn(ClaimAndDeliverFirstSubmissionRetry, thread_scope);
	});
	/**
	 * Claim and launch are one ordered Effect boundary. Separate top-level yields
	 * compile into independent SER sites: on a cold mount, the one-shot launch
	 * could observe no claim before asynchronous acquisition finished, then never
	 * rerun because that observation was intentionally untracked.
	 */
	const ClaimAndDeliverInitialFirstSubmission = Effect.gen(function* () {
		const claim = yield* ClaimPendingFirstSubmission;
		if (claim === undefined) return;
		yield* Effect.forkIn(DeliverClaimedFirstSubmission, thread_scope);
	});
	yield* ClaimAndDeliverInitialFirstSubmission;
</script>

<svelte:head>
	<title>{thread?.title ?? "Thread"} › Artisan Editor</title>
</svelte:head>

<ThreadWorkspace
	active_run_id={work?.run_id}
	active_run_status={work?.status}
	{context_usage}
	conversation_view_state={view_state}
	{image_sources}
	{inspection}
	onreturntoroot={orchestration.ReturnToRoot}
	onabort={CancelRun}
	{snapshot}
	disabled={session === undefined || first_submission_blocked}
	{first_submission_reference}
	onapproval={RespondApproval}
	onnewthread={StartThreadWithPrompt}
	onquestion={RespondQuestion}
	onretry={RetryRun}
	onusageinterruptionresolve={ResolveUsageInterruption}
	onimagevisibilitychange={UpdateImageAttachmentVisibility}
	onpolicychange={PersistSessionPolicy}
	onsubmit={SendMessage}
	onwithdraw={WithdrawQueuedMessage}
	policy={session?.policy}
	project_root_path={thread?.primary_project?.root_path}
	{run_active}
/>

{#if pending_first_submission_error !== undefined}
	<div
		class="fixed inset-x-0 bottom-6 z-30 mx-auto flex w-fit items-center gap-3 rounded-lg border border-destructive/40 bg-background px-4 py-3 shadow-lg"
		role="alert"
	>
		<span>The first message was not accepted: {pending_first_submission_error}</span>
		<button
			type="button"
			disabled={first_submission_attempting}
			onclick={yield* RetryPendingFirstSubmission}
		>
			{first_submission_attempting ? "Retrying first message…" : "Retry first message"}
		</button>
	</div>
{/if}
