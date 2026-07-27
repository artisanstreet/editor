<script lang="ts" effect>
	import { onDestroy, untrack } from "svelte";
	import type {
		ImageAttachmentReference,
		ThreadListItem,
		ThreadSessionPolicy,
		ThreadSessionSnapshot,
		ThreadWorkItem,
	} from "@artisan/protocol";
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
		ObserveAcceptedProjection,
		SubmitDurableCommand,
		ThreadInteractionError,
	} from "$lib/thread-interaction/commands";
	import { ConversationUserMessageWithSourceReference } from "$lib/conversation/scroll-position";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import { ResolveThreadRoute } from "$lib/root/thread-navigation";
	import { Effect, Exit, Option, Queue, Scope, Stream } from "effect";
	import ThreadWorkspace from "../../components/thread-workspace.sv";

	let { thread_id: route_thread_id }: { readonly thread_id: string } = $props();
	const route_id = untrack(() => route_thread_id);

	const client = yield* ArtisanClient;
	const frontend_scope = yield* Scope.Scope;
	const thread_scope = yield* Scope.make();
	type ThreadAction =
		| { readonly _tag: "Run"; readonly effect: Effect.Effect<void> }
		| { readonly _tag: "Stop" };
	const action_queue = yield* Queue.unbounded<ThreadAction>();
	const Dispatch = (effect: Effect.Effect<void>) => {
		Queue.offerUnsafe(action_queue, { _tag: "Run", effect });
	};
	onDestroy(() => Queue.offerUnsafe(action_queue, { _tag: "Stop" }));
	yield* Effect.forkIn(
		Effect.gen(function* () {
			while (true) {
				const action = yield* Queue.take(action_queue);
				if (action._tag === "Stop") {
					yield* Scope.close(thread_scope, Exit.void);
					return;
				}
				yield* Effect.forkIn(action.effect, thread_scope);
			}
		}),
		frontend_scope,
	);

	const threads = yield* client.ListThreads;
	const initial_thread = yield* Option.match(
		ResolveThreadRoute(threads, route_id),
		{
			onNone: () =>
				Effect.fail(
					new ThreadInteractionError({
						message: `Thread ${route_id} does not exist.`,
					}),
				),
			onSome: Effect.succeed,
		},
	);
	const thread_id = initial_thread.thread_id;
	let session = $state.raw<ThreadSessionSnapshot | undefined>(
		yield* client.GetThreadSession(thread_id),
	);
	let thread = $state.raw<ThreadListItem | undefined>(initial_thread);
	let work = $state.raw<ThreadWorkItem | undefined>(
		Option.getOrUndefined(yield* client.GetThreadWork(thread_id)),
	);
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
	let view_state: ConversationViewState | undefined;
	let image_sources = $state.raw<ReadonlyMap<string, string>>(new Map());
	const requested_image_ids = new Set<string>();
	const visible_image_ids = new Set<string>();
	const image_load_attempts = new Map<string, number>();

	const ReleaseImageAttachment = (attachment_id: string) => {
		const source = image_sources.get(attachment_id);
		if (source === undefined) return;
		URL.revokeObjectURL(source);
		const next_sources = new Map(image_sources);
		next_sources.delete(attachment_id);
		image_sources = next_sources;
	};

	const RequestImageAttachment = (attachment: ImageAttachmentReference) => {
		if (
			image_sources.has(attachment.id) ||
			requested_image_ids.has(attachment.id)
		)
			return;
		requested_image_ids.add(attachment.id);
		const attempt = (image_load_attempts.get(attachment.id) ?? 0) + 1;
		image_load_attempts.set(attachment.id, attempt);

		const LoadImageAttachment = Effect.gen(function* () {
			const result = yield* client.GetMessageImageAttachment({
				attachment_id: attachment.id,
				thread_id,
			});
			if (Option.isNone(result) || !visible_image_ids.has(attachment.id)) {
				requested_image_ids.delete(attachment.id);
				return;
			}

			const bytes = Uint8Array.from(result.value.bytes);
			const source = URL.createObjectURL(
				new Blob([bytes], { type: result.value.media_type }),
			);
			yield* Scope.addFinalizer(
				thread_scope,
				Effect.sync(() => URL.revokeObjectURL(source)),
			);
			image_sources = new Map(image_sources).set(attachment.id, source);
			image_load_attempts.delete(attachment.id);
			requested_image_ids.delete(attachment.id);
		}).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					if (!visible_image_ids.has(attachment.id) || attempt >= 3) {
						requested_image_ids.delete(attachment.id);
						return;
					}
					yield* Effect.sleep(attempt * 500);
					requested_image_ids.delete(attachment.id);
					if (!visible_image_ids.has(attachment.id)) return;
					RequestImageAttachment(attachment);
				}),
			),
		);

		Dispatch(LoadImageAttachment);
	};

	const UpdateImageAttachmentVisibility = (
		attachments: ReadonlyArray<ImageAttachmentReference>,
		visible: boolean,
	) => {
		for (const attachment of attachments) {
			if (visible) {
				visible_image_ids.add(attachment.id);
				RequestImageAttachment(attachment);
			} else {
				visible_image_ids.delete(attachment.id);
				image_load_attempts.delete(attachment.id);
				ReleaseImageAttachment(attachment.id);
			}
		}
	};

	const ReplaceSnapshot = (next: typeof snapshot) =>
		!CanReplaceConversationSnapshot(snapshot, next)
			? Effect.void
			: Effect.sync(() => {
					const initialized = MakeConversationViewState(next);
					snapshot = next;
					view_state = initialized._tag === "applied" ? initialized.state : undefined;
				});

	yield* ReplaceSnapshot(snapshot);

	const Resync = Effect.gen(function* () {
		yield* ReplaceSnapshot(yield* client.GetConversation({ thread_id }));
	});

	const RefreshInteractionContext = Effect.gen(function* () {
		const [next_session, threads, next_work] = yield* Effect.all([
			client.GetThreadSession(thread_id),
			client.ListThreads,
			client.GetThreadWork(thread_id),
		]);
		session = next_session;
		thread = threads.find((candidate) => candidate.thread_id === thread_id);
		work = Option.getOrUndefined(next_work);
	});

	const SendMessage = (submission: ComposerSubmission) =>
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
				Effect.suspend(() => {
					const has_accepted_user_message = (candidate: typeof snapshot) =>
						Option.isSome(
							ConversationUserMessageWithSourceReference(
								candidate.items,
								command_id,
							),
						);
					if (!expects_user_message || has_accepted_user_message(snapshot)) {
						return Effect.void;
					}
					return ObserveAcceptedProjection(
						client.GetConversation({ thread_id }),
						has_accepted_user_message,
					).pipe(
						Effect.flatMap(
							Option.match({
								onNone: () => Effect.void,
								onSome: ReplaceSnapshot,
							}),
						),
					);
				});

			/**
			 * Command acceptance is the durable submission boundary. The stream may
			 * still be establishing (or may have missed this exact handoff), so
			 * reconcile until the canonical projection contains this user turn.
			 * Later assistant patches remain stream-driven through `ApplyUpdate`.
			 */
			const receipt = yield* SubmitDurableCommand(
				client.Command(result.command),
				(receipt) => ReconcileAcceptedUserMessage(receipt.command_id),
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
			yield* client.UpdateThreadSessionPolicy({ policy, thread_id });
			yield* RefreshInteractionContext;
		});

	const PersistSessionPolicy = (policy: ThreadSessionPolicy) => {
		Dispatch(UpdateSessionPolicy(policy).pipe(Effect.catch(() => Effect.void)));
	};

	const RunCommand = (payload:
		| { readonly type: "run.cancel" }
		| {
				readonly type: "run.respond_approval";
				readonly approval_id: string;
				readonly approved: boolean;
		  }
		| {
				readonly type: "run.respond_question";
				readonly answers: Record<string, [string, ...string[]]>;
		  }) => {
		Dispatch(
			client.Command({ payload, thread_id }).pipe(
				Effect.andThen(RefreshInteractionContext),
				Effect.catch(() => Effect.void),
			),
		);
	};

	const ApplyUpdate = (update: ConversationUpdate) =>
		update.type === "snapshot"
			? ReplaceSnapshot(update.snapshot)
			: Effect.gen(function* () {
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
			Effect.succeed(
				client.Events.pipe(
					Stream.filter((event) => event.thread_id === thread_id),
					Stream.debounce("50 millis"),
				),
			),
			() => Resync,
			Resync,
		),
		thread_scope,
	);
</script>

<ThreadWorkspace
	{image_sources}
	{snapshot}
	disabled={session === undefined}
	onapproval={(approval_id, approved) =>
		RunCommand({ approval_id, approved, type: "run.respond_approval" })}
	onquestion={(question_id, answer) =>
		RunCommand({
			answers: { [question_id]: [answer] },
			type: "run.respond_question",
		})}
	onimagevisibilitychange={UpdateImageAttachmentVisibility}
	onpolicychange={PersistSessionPolicy}
	onsubmit={SendMessage}
	policy={session?.policy}
/>
