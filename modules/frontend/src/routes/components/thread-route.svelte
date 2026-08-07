<script lang="ts" effect>
	import { navigating, page } from "$app/state";
	import { untrack } from "svelte";
	import type {
		ImageAttachmentReference,
		SurfaceUsageAggregate,
		ThreadListItem,
		ThreadSessionPolicy,
		ThreadSessionSnapshot,
		ThreadWorkItem,
	} from "@artisan/protocol";
	import type { ConversationPatch } from "@artisan/protocol";
	import { ArtisanClient, type ConversationUpdate } from "@artisan/transport/client";
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
	import {
		BuildThreadMessageCommand,
		MakeSubmitGate,
		ObserveAcceptedProjection,
		SubmitDurableCommand,
		ThreadInteractionError,
	} from "$lib/thread-interaction/commands";
	import { ConversationUserMessageWithSourceReference } from "$lib/conversation/scroll-position";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import { BannerService } from "$lib/banner/service";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import {
		RunUsageController,
		type RunUsageState,
	} from "$lib/context-usage/run-usage-controller";
	import {
		DraftThreadController,
		type DraftSubmissionClaim,
	} from "$lib/root/draft-thread";
	import { MakeLatestRequestGate } from "$lib/lifecycle/latest-request-gate";
	import {
		CreateBrowserObjectUrl,
		ReleaseBrowserObjectUrl,
	} from "$lib/browser/object-url";
	import { ComposerDraftStore } from "$lib/composer/draft-store";
	import {
		ResolveThreadRoute,
		ThreadRouteOwnsTarget,
		ThreadRoutePath,
		ThreadRoutePathFor,
	} from "$lib/root/thread-navigation";
	import { Effect, Exit, Option, Scope, Semaphore, Stream } from "effect";
	import ThreadWorkspace from "./thread-workspace.svelte";

	let {
		thread_id: route_thread_id,
	}: {
		readonly thread_id: string;
	} = $props();
	const route_id = untrack(() => route_thread_id);

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	const navigation = yield* RouteNavigation;
	const draft_thread = yield* DraftThreadController;
	const composer_drafts = yield* ComposerDraftStore;
	const run_usage = yield* RunUsageController;
	const run_usage_lease = yield* run_usage.Acquire(undefined);
	const thread_scope = yield* Scope.make();

	const threads = yield* client.ListThreads;
	const initial_thread = yield* Option.match(
		ResolveThreadRoute(threads, route_id),
		{
			onNone: () =>
				Effect.gen(function* () {
					return yield* Effect.fail(
					new ThreadInteractionError({
						message: `Thread ${route_id} does not exist.`,
					}),
					);
				}),
			onSome: (candidate) =>
				Effect.gen(function* () {
					return candidate;
				}),
		},
	);
	const thread_id = initial_thread.thread_id;
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
	let session = $state.raw<ThreadSessionSnapshot | undefined>(
		yield* client.GetThreadSession(thread_id),
	);
	let thread = $state.raw<ThreadListItem | undefined>(initial_thread);
	let work = $state.raw<ThreadWorkItem | undefined>(
		Option.getOrUndefined(yield* client.GetThreadWork(thread_id)),
	);
	const run_active = $derived(
		work?.status === "running" || work?.status === "waiting",
	);
	let context_usage = $state.raw<SurfaceUsageAggregate | undefined>(undefined);
	const ApplyRunUsage = (state: RunUsageState) =>
		Effect.gen(function* () {
			context_usage = state._tag === "Ready" ? state.aggregate : undefined;
		});
	yield* run_usage.Changes.pipe(
		Stream.runForEach(ApplyRunUsage),
		Effect.forkScoped,
	);
	yield* run_usage_lease.Select(work?.run_id);
	yield* Effect.addFinalizer(run_usage_lease.Release);
	const initial_snapshot = yield* client.GetConversation({ thread_id });
	if (initial_snapshot.thread_id !== thread_id) {
		yield* Effect.fail(
			new ThreadInteractionError({
				message: `Conversation snapshot belongs to ${initial_snapshot.thread_id}, not ${thread_id}.`,
			}),
		);
	}
	const conversation_id = initial_snapshot.conversation_id;
	let snapshot = $state.raw(initial_snapshot);
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

	const UpdateImageAttachmentVisibility = (
		attachments: ReadonlyArray<ImageAttachmentReference>,
		visible: boolean,
	) =>
		Effect.gen(function* () {
		for (const attachment of attachments) {
			if (visible) {
				visible_image_ids.add(attachment.id);
				yield* RequestImageAttachment(attachment);
			} else {
				visible_image_ids.delete(attachment.id);
				yield* ClearImageLoadState(attachment.id);
				yield* ReleaseImageAttachment(attachment.id);
			}
		}
		});

	const ReplaceSnapshot = (next: typeof snapshot) =>
		Effect.gen(function* () {
			if (!CanReplaceConversationSnapshot(snapshot, next)) return;
			const initialized = MakeConversationViewState(next);
			snapshot = next;
			view_state = initialized._tag === "applied" ? initialized.state : undefined;
		});

	yield* ReplaceSnapshot(snapshot);

	const Resync = Effect.gen(function* () {
		yield* ReplaceSnapshot(yield* client.GetConversation({ thread_id }));
	});

	/**
	 * Reads may complete out of order because conversation patches, command
	 * receipts, and recovery all refresh this context. A request is allowed to
	 * read concurrently, but only the newest request may commit its
	 * session/thread/run tuple and select a usage lease.
	 */
	const interaction_refresh_requests = yield* MakeLatestRequestGate;
	const interaction_refresh_commit = yield* Semaphore.make(1);
	const RefreshInteractionContext = Effect.gen(function* () {
		const generation = yield* interaction_refresh_requests.Begin;
		const [next_session, threads, next_work] = yield* Effect.all([
			client.GetThreadSession(thread_id),
			client.ListThreads,
			client.GetThreadWork(thread_id),
		]);
		yield* interaction_refresh_commit.withPermit(
			Effect.gen(function* () {
				if (!(yield* interaction_refresh_requests.IsCurrent(generation))) return;
				session = next_session;
				thread = threads.find((candidate) => candidate.thread_id === thread_id);
				if (thread !== undefined) yield* CanonicalizeThreadPath(thread);
				work = Option.getOrUndefined(next_work);
				yield* run_usage_lease.Select(work?.run_id);
			}),
		);
	});

	const SendMessage = (submission: ComposerSubmission, command_id?: string) =>
		Effect.gen(function* () {
			if (session === undefined) {
				return yield* Effect.fail(
					new ThreadInteractionError({ message: "Thread session context is still loading." }),
				);
			}

			const result = BuildThreadMessageCommand({ session, thread, thread_id, work }, submission);
			if (result._tag === "invalid") return yield* Effect.fail(result.error);
			const expects_user_message =
				result.command.payload.type === "thread.send_message";
			const ReconcileAcceptedUserMessage = (command_id: string) =>
				Effect.gen(function* () {
					const has_accepted_user_message = (candidate: typeof snapshot) =>
						Option.isSome(
							ConversationUserMessageWithSourceReference(
								candidate.items,
								command_id,
							),
						);
					if (!expects_user_message || has_accepted_user_message(snapshot)) {
						return;
					}
					const observed = yield* ObserveAcceptedProjection(
						client.GetConversation({ thread_id }),
						has_accepted_user_message,
					);
					if (Option.isSome(observed)) yield* ReplaceSnapshot(observed.value);
				});

			/**
			 * Command acceptance is the durable submission boundary. The stream may
			 * still be establishing (or may have missed this exact handoff), so
			 * reconcile until the canonical projection contains this user turn.
			 * Later assistant patches remain stream-driven through `ApplyUpdate`.
			 */
			const receipt = yield* SubmitDurableCommand(
				client.Command({ ...result.command, ...(command_id === undefined ? {} : { command_id }) }),
				(receipt) =>
					Effect.gen(function* () {
						yield* ReconcileAcceptedUserMessage(receipt.command_id);
					}),
			);
			yield* Effect.forkIn(
				RefreshInteractionContext.pipe(Effect.ignore),
				thread_scope,
			);
			return {
				expects_user_message,
				...(expects_user_message
					? { user_message_reference: receipt.command_id }
					: {}),
			};
		});

	const UpdateSessionPolicy = (policy: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			const authoritative_policy = yield* Effect.gen(function* () {
				yield* client.UpdateThreadSessionPolicy({ policy, thread_id });
				const refreshed = yield* client.GetThreadSession(thread_id);
				session = refreshed;
				return refreshed.policy;
			}).pipe(
				Effect.catch((error) =>
					Effect.gen(function* () {
						const reconciled = yield* client.GetThreadSession(thread_id).pipe(Effect.option);
						if (Option.isSome(reconciled)) {
							session = reconciled.value;
							return reconciled.value.policy;
						}
						return yield* Effect.fail(error);
					}),
				),
			);
			yield* RefreshInteractionContext;
			return authoritative_policy;
		});

	const PersistSessionPolicy = (policy: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			return yield* UpdateSessionPolicy(policy);
		});

	const RespondApproval = (approval_id: string, approved: boolean) =>
		Effect.gen(function* () {
			yield* client.Command({
				payload: { approval_id, approved, type: "run.respond_approval" },
				thread_id,
			});
			yield* RefreshInteractionContext;
		});

	const CancelRun = () =>
		Effect.gen(function* () {
			yield* client.Command({ payload: { type: "run.cancel" }, thread_id });
			yield* RefreshInteractionContext;
		});

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
			const catalog = yield* client.ListProjects;
			const project = catalog.projects.find(
				(candidate) => candidate.project_id === project_ref.project_id,
			);
			if (project === undefined) {
				return yield* Effect.fail(
					new ThreadInteractionError({
						message: `${project_ref.display_name} is no longer attached to Forge.`,
					}),
				);
			}
			yield* draft_thread.SelectProject(project);
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
		Effect.gen(function* () {
			yield* client.Command({ payload, thread_id });
			yield* RefreshInteractionContext;
		});

	/** Turn outcomes after which no further work lands, whatever the outcome. */
	const settled_lifecycles = new Set(["completed", "failed", "cancelled"]);
	const PatchSettlesTurn = (patch: ConversationPatch) =>
		(patch.type === "turn_lifecycle" && settled_lifecycles.has(patch.lifecycle)) ||
		(patch.type === "turn_upsert" && settled_lifecycles.has(patch.turn.lifecycle));

	const RefreshAuthoritativeThread = Effect.gen(function* () {
		yield* Resync;
		yield* RefreshInteractionContext;
	});

	const ApplyUpdate = (update: ConversationUpdate) =>
		Effect.gen(function* () {
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
		});

	yield* Effect.forkIn(
		RunConversationSubscription(
			client.SubscribeConversation(thread_id),
			ApplyUpdate,
			Resync,
		),
		thread_scope,
	);
	yield* Effect.forkIn(
		RunAuthoritativeSubscription(
			Effect.gen(function* () {
				return client.Events.pipe(
					Stream.filter((event) => event.thread_id === thread_id),
					Stream.debounce("50 millis"),
				);
			}),
			() =>
				Effect.gen(function* () {
					yield* RefreshAuthoritativeThread;
				}),
			RefreshAuthoritativeThread,
		),
		thread_scope,
	);

	const RespondQuestion = (question_id: string, answer: string) =>
		Effect.gen(function* () {
			yield* RunCommand({
				answers: { [question_id]: [answer] },
				type: "run.respond_question",
			}).pipe(
				Effect.catch((error) =>
					Effect.gen(function* () {
						yield* banner.error("Could not answer question", {
							description: error.message,
						});
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
	let pending_first_submission_error = $state<string | undefined>(undefined);
	let first_submission_attempting = $state(false);
	let first_submission_blocked = $state(false);
	const first_submission_gate = yield* MakeSubmitGate;
	const ClaimPendingFirstSubmission = Effect.gen(function* () {
		pending_first_submission = yield* draft_thread.AwaitPendingSubmissionClaim(thread_id);
		first_submission_blocked = pending_first_submission !== undefined;
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
	const RetryPendingFirstSubmission = Effect.gen(function* () {
		if (!(yield* first_submission_gate.Acquire)) return;
		first_submission_attempting = true;
		pending_first_submission_error = undefined;
		yield* DeliverPendingFirstSubmission.pipe(
			Effect.catch((error) =>
				Effect.gen(function* () {
					pending_first_submission_error = error.message;
					const claimed = pending_first_submission;
					if (claimed !== undefined) yield* claimed.Release;
					pending_first_submission = undefined;
					yield* banner.error("Could not send message", { description: error.message });
				}),
			),
			Effect.ensuring(
				Effect.gen(function* () {
					first_submission_attempting = false;
					yield* first_submission_gate.Release;
				}),
			),
		);
	});
	yield* ClaimPendingFirstSubmission;
	yield* Effect.addFinalizer(() =>
		Effect.gen(function* () {
			const claimed = pending_first_submission;
			if (claimed !== undefined) yield* claimed.Release;
		}),
	);
	if (pending_first_submission !== undefined) {
		yield* RetryPendingFirstSubmission;
	}
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
	onabort={CancelRun}
	{snapshot}
	disabled={session === undefined || first_submission_blocked}
	onapproval={RespondApproval}
	onnewthread={StartThreadWithPrompt}
	onquestion={RespondQuestion}
	onimagevisibilitychange={UpdateImageAttachmentVisibility}
	onpolicychange={PersistSessionPolicy}
	onsubmit={SendMessage}
	policy={session?.policy}
	{run_active}
/>

{#if first_submission_blocked}
	<div class="fixed inset-x-0 bottom-6 z-30 mx-auto flex w-fit items-center gap-3 rounded-lg border border-destructive/40 bg-background px-4 py-3 shadow-lg" role="alert">
		<span>
			{pending_first_submission_error === undefined
				? "The first message is being delivered. It must finish before another message can be sent."
				: "The first message was not accepted. Retry it before sending another message."}
		</span>
		<button
			type="button"
			disabled={first_submission_attempting}
			onclick={yield* RetryPendingFirstSubmission}
		>
			{first_submission_attempting ? "Retrying first message…" : "Retry first message"}
		</button>
	</div>
{/if}
