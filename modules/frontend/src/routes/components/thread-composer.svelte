<script lang="ts" effect>
	import ChevronDown from "@tabler/icons-svelte/icons/chevron-down";
	import { Effect, Stream } from "effect";
	import type { SurfaceUsageAggregate, ThreadSessionPolicy } from "@artisan/protocol";
	import { ReleaseBrowserObjectUrl } from "$lib/browser/object-url";
	import { Button } from "$lib/components/ui/button";
	import { LipCard } from "$lib/components/ui/lip-card";
	import {
		MakeComposerGestureIntake,
		type ComposerGesture,
	} from "$lib/composer/gesture-intake";
	import {
		MakeSubmitGate,
		type SubmitGate,
	} from "$lib/thread-interaction/commands";
	import type { SteeringPendingLipState } from "$lib/thread-interaction/steering-pending-lip";
	import { MakeSteeringStages } from "$lib/thread-interaction/steering-stages";
	import {
		ComposerPlaceholderCharacterDelay,
		MakeComposerPlaceholderState,
		UpdateComposerPlaceholderState,
	} from "$lib/composer-placeholder";
	import {
		MakeImageAttachmentParts,
		type ComposerImageAttachment,
		type ComposerSubmission,
		type ComposerSubmissionHandler,
	} from "$lib/composer/image-attachments";
	import type { ThreadMessageSubmissionOutcome } from "$lib/thread-interaction/commands";
	import type { ComposerActionFailure } from "$lib/composer/action-failure";
	import { MakeComposerAttachmentIntake } from "$lib/composer/attachment-intake";
	import {
		ComposerContextWindowTokens,
		ComposerSendBlockedReason,
	} from "$lib/composer/send-readiness";
	import {
		ClearComposerEditor,
		ComposerAttachmentIdAtEvent,
		ComposerDropRange,
		ComposerSelectedRange,
		FocusComposerEditor,
		FocusComposerRange,
		InsertComposerAttachmentMarkers,
		ReadComposerEditorDocument,
		RemoveComposerAttachmentMarkers,
		type ComposerEditorDocument,
	} from "./composer/dom";
	import { MakeQueuedSteerActions } from "./composer/queued-steer";
	import { MakeComposerDraftSession } from "./composer/draft-session";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import ImageViewer from "./image-viewer.svelte";
	import ShaderGlassSurface from "./shader-glass-surface.svelte";
	import ActionFailureAlert from "./composer/action-failure.svelte";
	import AttachmentTray from "./composer/attachment-tray.svelte";
	import ComposerControls from "./composer/controls.svelte";
	import SteeringLip from "./composer/steering-lip.svelte";

	const defaults_controller = yield* SessionDefaultsController;
	let defaults_state = $state.raw<SessionDefaultsState>(yield* defaults_controller.Current);
	const ApplyDefaults = (next: SessionDefaultsState) =>
		Effect.gen(function* () {
			defaults_state = next;
		});
	yield* defaults_controller.Changes.pipe(
		Stream.runForEach(ApplyDefaults),
		Effect.forkScoped,
	);
	const runtime_catalog = $derived(defaults_state.catalog);

	let {
		context_usage,
		disabled = false,
		draft_key,
		onabort,
		onjumptolatest,
		onnewthread,
		onpolicychange,
		onsteeringchange,
		onsubmit,
		onwithdraw,
		policy,
		run_active = false,
		show_jump_to_latest = false,
	}: {
		context_usage?: SurfaceUsageAggregate;
		disabled?: boolean;
		draft_key?: string;
		onabort?: () => Effect.Effect<unknown, { readonly message: string }>;
		onjumptolatest?: Effect.Effect<void>;
		onnewthread?: ComposerSubmissionHandler;
		onpolicychange?: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<ThreadSessionPolicy, { readonly message: string }>;
		onsteeringchange?: (pending: boolean, source_reference?: string) => void;
		onsubmit?: ComposerSubmissionHandler;
		/** Recalls one queued steer by the send's own command id, while Forge still holds it. */
		onwithdraw?: (command_id: string) => Effect.Effect<void, { readonly message: string }>;
		policy?: ThreadSessionPolicy;
		run_active?: boolean;
		show_jump_to_latest?: boolean;
	} = $props();

	const JumpToLatest = Effect.gen(function* () {
		if (onjumptolatest !== undefined) yield* onjumptolatest;
	});

	const send_blocked_reason = $derived(
		ComposerSendBlockedReason(defaults_state.available, runtime_catalog, policy),
	);

	const context_window_tokens = $derived(
		ComposerContextWindowTokens(runtime_catalog, policy, context_usage),
	);
	const context_percent = $derived(
		context_usage?.context_tokens === undefined || context_window_tokens === undefined
			? undefined
			: Math.min(100, (context_usage.context_tokens / context_window_tokens) * 100),
	);

	let editor = $state<HTMLDivElement | null>(null);
	let attachments = $state<ReadonlyMap<string, ComposerImageAttachment>>(new Map());
	let cancelling = $state(false);
	let draft = $state("");
	let image_viewer_open = $state(false);
	let submitting = $state(false);
	let viewed_attachment = $state<ComposerImageAttachment | undefined>();
	let placeholder = $state(MakeComposerPlaceholderState());
	let pending_lip_state = $state.raw<SteeringPendingLipState<ComposerSubmission>>({
		next_generation: 0,
		pending: [],
	});
	/** The stages borrow the reactive lip through accessors; the template renders component-owned state. */
	const steering = MakeSteeringStages<ComposerSubmission>({
		Lip: () => pending_lip_state,
		ReplaceLip: (next) => {
			pending_lip_state = next;
		},
		SteeringChanged: (pending, source_reference) =>
			onsteeringchange?.(pending, source_reference),
		Withdraw: (command_id) =>
			onwithdraw === undefined
				? Effect.fail({ message: "This surface cannot recall a queued message." })
				: onwithdraw(command_id),
	});
	const submit_gate: SubmitGate = yield* MakeSubmitGate;
	/** What the last send or attachment refused; the surface itself explains why. */
	let action_failure = $state.raw<ComposerActionFailure | undefined>(undefined);
	const ReportActionFailure = (title: string, description: string) =>
		Effect.gen(function* () {
			action_failure = { description, title };
		});
	const DismissActionFailure = Effect.gen(function* () {
		action_failure = undefined;
	});

	/**
	 * An image has no bytes for the few milliseconds it is being encoded. That is
	 * far too short to disarm the send button over — flipping it would read as a
	 * glitch — so arming ignores it and the submit itself declines instead.
	 */
	const attachments_ready = $derived(
		[...attachments.values()].every((attachment) => attachment.ready),
	);

	/**
	 * Everything a send needs, in one place: the button's disabled state and
	 * its arming visual (white → gradient reveal) must flip together.
	 */
	const send_ready = $derived(
		!disabled &&
			!submitting &&
			send_blocked_reason === undefined &&
			(draft.trim().length > 0 || attachments.size > 0) &&
			onsubmit !== undefined,
	);

	const UpdateDraft = (value: string, has_attachments = attachments.size > 0) =>
		Effect.gen(function* () {
			draft = value;
			placeholder = UpdateComposerPlaceholderState(
				placeholder,
				has_attachments ? "\uFFFC" : value,
			);
		});

	const RevokeAttachment = (attachment: ComposerImageAttachment | undefined) =>
		Effect.gen(function* () {
			if (attachment !== undefined) {
				yield* ReleaseBrowserObjectUrl(attachment.preview_url).pipe(Effect.ignore);
			}
		});

	/**
	 * Thread switching remounts this component, so the typed text and its
	 * attachments would die with the instance. The session mirrors the document
	 * into the draft store after every change and gives it back on remount.
	 *
	 * Named rather than written inline at the yield site: the SER transform
	 * collects the identifiers of the yielded expression as reactive inputs,
	 * and these accessors close over state that changes on every keystroke. A
	 * stable reference keeps the session from being recreated mid-typing.
	 */
	const make_draft_session = () =>
		MakeComposerDraftSession({
			draft_key,
			Attachments: () => attachments,
			DraftText: () => draft,
			Editor: () => editor,
			Restored: (text: string, revived: ReadonlyMap<string, ComposerImageAttachment>) =>
				Effect.gen(function* () {
					attachments = revived;
					yield* UpdateDraft(text, revived.size > 0);
				}),
			Revoke: RevokeAttachment,
		});
	const drafts = yield* make_draft_session();

	const SyncEditor = (): Effect.Effect<ComposerEditorDocument> =>
		Effect.gen(function* () {
			const editor_document = yield* ReadComposerEditorDocument(editor, draft);
			const present = new Set(editor_document.tokens.map((token) => token.id));
			const next = new Map<string, ComposerImageAttachment>();
			for (const [id, attachment] of attachments) {
				if (present.has(id)) next.set(id, attachment);
				else {
					if (viewed_attachment?.id === id) {
						image_viewer_open = false;
						viewed_attachment = undefined;
					}
					yield* RevokeAttachment(attachment);
				}
			}
			attachments = next;
			yield* UpdateDraft(editor_document.text, next.size > 0);
			yield* drafts.Persist;
			return editor_document;
		});

	const InsertAttachments = (
		next_attachments: ReadonlyArray<ComposerImageAttachment>,
		range: Range,
	) =>
		Effect.gen(function* () {
			if (editor === null || next_attachments.length === 0) return;
			const caret = yield* InsertComposerAttachmentMarkers(next_attachments, range);
			yield* FocusComposerRange(caret);
			yield* SyncEditor();
		});

	const RemoveAttachment = (attachment_id: string) =>
		Effect.gen(function* () {
			const attachment = attachments.get(attachment_id);
			if (viewed_attachment?.id === attachment_id) image_viewer_open = false;
			yield* RemoveComposerAttachmentMarkers(editor, attachment_id);
			yield* RevokeAttachment(attachment);
			attachments = new Map([...attachments].filter(([id]) => id !== attachment_id));
			yield* SyncEditor();
			yield* FocusComposerEditor(editor);
		});

	const ViewAttachment = (attachment: ComposerImageAttachment) =>
		Effect.gen(function* () {
			viewed_attachment = attachment;
			image_viewer_open = true;
		});

	/** Replaces one attachment in place, leaving its marker and position alone. */
	const SettleAttachment = (settled: ComposerImageAttachment) =>
		Effect.gen(function* () {
			if (!attachments.has(settled.id)) return;
			attachments = new Map(attachments).set(settled.id, settled);
			yield* drafts.Persist;
		});

	const DropAttachment = (attachment: ComposerImageAttachment) =>
		Effect.gen(function* () {
			yield* RemoveComposerAttachmentMarkers(editor, attachment.id);
			yield* RevokeAttachment(attachment);
			attachments = new Map([...attachments].filter(([id]) => id !== attachment.id));
			yield* drafts.Persist;
		});

	/**
	 * Placing an image is its own ordered job — validate, recognise a re-paste,
	 * show, encode, settle — so it lives beside the other composer intake. What
	 * stays here is the part only the component can do: its state and editor placement.
	 */
	const intake = MakeComposerAttachmentIntake({
		Attachments: () => attachments,
		Blocked: () => disabled || submitting,
		EngineId: () => policy?.engine_id,
		Present: (added, range) =>
			Effect.gen(function* () {
				attachments = new Map([
					...attachments,
					...added.map((attachment) => [attachment.id, attachment] as const),
				]);
				yield* InsertAttachments(added, range ?? (yield* ComposerSelectedRange(editor)));
			}),
		Remove: DropAttachment,
		Report: (description) => ReportActionFailure("Could not attach image", description),
		Revoke: RevokeAttachment,
		Settle: SettleAttachment,
	});
	const AddFiles = intake.AddFiles;

	const queued_steer = MakeQueuedSteerActions({
		AddFiles,
		Editor: () => editor,
		Report: ReportActionFailure,
		SyncEditor: () => SyncEditor(),
		Withdraw: steering.Withdraw,
	});

	const CompleteSubmission = Effect.gen(function* () {
		for (const attachment of attachments.values()) yield* RevokeAttachment(attachment);
		attachments = new Map();
		yield* ClearComposerEditor(editor);
		yield* UpdateDraft("");
		yield* drafts.Clear;
	});
	const ReleaseSubmission = (
		retain_pending_lip: boolean,
		pending_lip_generation: number | undefined,
	) =>
		Effect.gen(function* () {
			yield* submit_gate.Release;
			submitting = false;
			if (!retain_pending_lip && pending_lip_generation !== undefined)
				yield* steering.ReleaseLip(pending_lip_generation);
		});

	/**
	 * The one path a composed document leaves through, whatever receives it.
	 * A not-yet-encoded attachment declines silently rather than refusing: the
	 * encode finishes in a moment. The receiver is read through a thunk at
	 * execution time so each firing sees the current props.
	 */
	const DeliverSubmission = (
		receiver: () =>
			| ((
					submission: ComposerSubmission,
				) => Effect.Effect<ThreadMessageSubmissionOutcome | void, { readonly message: string }>)
			| undefined,
		failure_title: string,
		show_pending_lip = false,
	) =>
		Effect.gen(function* () {
			const deliver = receiver();
			if (deliver === undefined) return;
			const editor_document = yield* SyncEditor();
			const attachment_parts = MakeImageAttachmentParts(attachments, editor_document.tokens);
			const submission: ComposerSubmission = {
				attachments: attachment_parts,
				text: editor_document.text,
			};
			if (!attachments_ready) return;
			if (editor_document.text.length === 0 && attachment_parts.length === 0) return;
			if (submitting) return;
			/**
			 * Enter still lands here while the send button is disarmed; a composed
			 * message that cannot leave must say so, not silently go nowhere.
			 */
			if (disabled || send_blocked_reason !== undefined) {
				yield* ReportActionFailure(
					failure_title,
					send_blocked_reason ??
						"This surface is still preparing to send. Your message is kept in the composer.",
				);
				return;
			}
			if (!(yield* submit_gate.Acquire)) return;
			submitting = true;
			/** A refusal describes the attempt that produced it, never the next one. */
			action_failure = undefined;
			/** The lip alone marks a queued steer; the label waits for the echo. */
			let pending_lip_generation: number | undefined;
			if (show_pending_lip && run_active) {
				pending_lip_generation = steering.Begin(submission, Date.now());
			}

			let retain_pending_lip = false;
			yield* deliver(submission).pipe(
				Effect.matchEffect({
					onFailure: (error) => ReportActionFailure(failure_title, error.message),
						onSuccess: (outcome) =>
							Effect.gen(function* () {
								yield* CompleteSubmission;
								const steering_echo = outcome?.steering_echo;
								const steering_settlement = outcome?.steering_settlement;
								if (
									steering_echo === undefined ||
									steering_settlement === undefined ||
									pending_lip_generation === undefined
								)
									return;
								retain_pending_lip = true;
								const steer_generation = pending_lip_generation;
								const reference = outcome?.user_message_reference;
								/** A discard asked for while the send was nameless completes here. */
								if (
									reference !== undefined &&
									steering.Bind(steer_generation, reference) &&
									(yield* queued_steer.TryWithdraw(steer_generation))
								)
									return;
								/**
								 * Two settlements, in order: the echo moves the message out of
								 * the queued lip and raises the label, the acknowledgement
								 * lowers it. An unconfirmed stage must say so rather than
								 * leave the lip or the label standing.
								 */
								const settlement = yield* steering_echo.pipe(
									Effect.andThen(steering.TakeUp(steer_generation)),
									Effect.andThen(steering_settlement),
									Effect.catch((error) =>
										ReportActionFailure("Could not confirm the steer", error.message),
									),
									Effect.ensuring(steering.Settle(steer_generation)),
									Effect.forkScoped,
								);
								steering.Adopt(steer_generation, settlement);
							}),
					}),
				Effect.ensuring(
					Effect.suspend(() => ReleaseSubmission(retain_pending_lip, pending_lip_generation)),
				),
			);
		});

	const Submit = DeliverSubmission(() => onsubmit, "Could not send message", true);

	/**
	 * The escape hatch while a run holds the primary button: carry the typed
	 * draft into a fresh thread instead of waiting the run out. Delivery and
	 * navigation belong to the route; this only packages the submission the same
	 * way a send does.
	 */
	const new_thread_ready = $derived(
		run_active &&
			!disabled &&
			!submitting &&
			send_blocked_reason === undefined &&
			(draft.trim().length > 0 || attachments.size > 0) &&
			onnewthread !== undefined,
	);
	const StartNewThread = DeliverSubmission(
		() => (run_active ? onnewthread : undefined),
		"Could not start a new thread",
	);

	const Cancel = Effect.gen(function* () {
		if (disabled || cancelling || onabort === undefined) return;
		cancelling = true;

		yield* onabort().pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					cancelling = false;
				}),
			),
		);
	});

	const ActivatePrimaryAction = Effect.gen(function* () {
		if (run_active) {
			yield* Cancel;
			return;
		}
		yield* Submit;
	});

	/**
	 * Paste, drop, and Enter settle their DOM contract inside the event's own
	 * dispatch, which a SER event effect is already too late for, so the intake
	 * owns their synchronous half. Everything effectful about them lands here,
	 * one gesture at a time, so a send can never overtake the attachment it was
	 * typed under.
	 */
	const RunComposerGesture = (gesture: ComposerGesture) =>
		Effect.gen(function* () {
			if (gesture._tag === "submit") {
				yield* Submit;
				return;
			}
			if (gesture.point === undefined) {
				yield* AddFiles(gesture.files);
				return;
			}
			const range =
				(yield* ComposerDropRange(editor, gesture.point)) ??
				(yield* ComposerSelectedRange(editor));
			yield* AddFiles(gesture.files, range);
		});

	const gestures = yield* MakeComposerGestureIntake(RunComposerGesture);

	const HandleEditorClick = (event: MouseEvent) =>
		Effect.gen(function* () {
			const attachment_id = yield* ComposerAttachmentIdAtEvent(event);
			if (attachment_id === undefined) return;
			const attachment = attachments.get(attachment_id);
			if (attachment !== undefined) yield* ViewAttachment(attachment);
		});

	const QueueEditorSync = Effect.gen(function* () {
		yield* Effect.yieldNow;
		yield* SyncEditor();
	});
	const ClearViewedAttachment = Effect.gen(function* () {
		viewed_attachment = undefined;
	});
	const ResetCancelling = Effect.gen(function* () {
		if (!run_active) cancelling = false;
	});

	yield* Effect.addFinalizer(() => drafts.ReleaseUnretained);

	yield* ResetCancelling;
	yield* drafts.Restore(editor);
</script>

<div class="prose-column-frame pointer-events-none absolute inset-x-0 bottom-0 z-20 flex flex-col items-center gap-2 pb-4 sm:pb-6">
	{#if show_jump_to_latest && onjumptolatest !== undefined}
		<div class="prose-column flex w-full max-w-(--prose-width) justify-center">
			<ShaderGlassSurface class="pointer-events-auto size-8 rounded-full shadow-lg">
				<Button
					variant="ghost"
					size="icon-sm"
					class="size-full rounded-full bg-transparent text-muted-foreground hover:bg-transparent hover:text-foreground"
					aria-label="Jump to latest"
					title="Jump to latest"
					onclick={yield* JumpToLatest}
				>
					<ChevronDown class="size-4" aria-hidden="true" />
				</Button>
			</ShaderGlassSurface>
		</div>
	{/if}
	<ActionFailureAlert failure={action_failure} ondismiss={DismissActionFailure} />
	<LipCard
		class="group/lip prose-column pointer-events-auto w-full max-w-(--prose-width) flex-col-reverse radius-surface data-[open=false]:overflow-visible [--radius-surface:var(--radius-2xl)] [--radius-gap:calc(var(--spacing)*2)]"
			open={pending_lip_state.pending.length > 0}
			variant={pending_lip_state.pending.length === 0 ? "glass" : "solid"}
	>
		<ShaderGlassSurface
			class="t-resize thread-composer group/composer w-full radius-surface [--radius-surface:var(--radius-2xl)] [--radius-gap:calc(var(--spacing)*2)] group-data-[open=true]/lip:rounded-t-none [--composer-resize-dur:var(--resize-dur)] [--composer-resize-ease:var(--resize-ease)]"
			data-has-attachments={attachments.size > 0}
		>
			<div class="relative z-10 flex min-h-32 flex-col p-2">
				<!-- The editor stays live under the stack: queued steers must not block composing the next one. -->
				<div class="flex flex-1 flex-col">
					<AttachmentTray {attachments} onremove={RemoveAttachment} onview={ViewAttachment} />

					<div class="relative min-h-16 flex-1">
						{#if placeholder.visible}
							{#key placeholder.generation}
								{@const characters = Array.from(placeholder.phrase)}
								<div aria-hidden="true" class="placeholder-reveal pointer-events-none absolute inset-x-3 top-2 text-base text-muted-foreground">
									<span class="placeholder-reveal-line">
										{#each characters as character, index}
											<span class="placeholder-character" style={`--placeholder-delay: ${ComposerPlaceholderCharacterDelay(index, characters.length)}ms`}>{character}</span>
										{/each}
									</span>
								</div>
							{/key}
						{/if}
						<div
							bind:this={editor}
							class="composer-editor size-full min-h-16 cursor-text px-3 py-2 text-base whitespace-pre-wrap [overflow-wrap:anywhere] outline-none empty:before:content-['']"
							contenteditable={disabled || submitting ? "false" : "plaintext-only"}
							role="textbox"
							tabindex="0"
							aria-label="Message thread"
							aria-multiline="true"
							aria-disabled={disabled || submitting}
							oninput={yield* SyncEditor()}
							onkeydown={gestures.SubmitKey}
							onkeyup={yield* QueueEditorSync}
							onpaste={gestures.Paste}
							ondragover={gestures.AcceptFileDrag}
							ondrop={gestures.Drop}
							onclick={yield* HandleEditorClick(event)}
						></div>
					</div>
				</div>

				<ComposerControls
					abort_available={onabort !== undefined}
					{cancelling}
					{context_percent}
					{context_usage}
					{context_window_tokens}
					{disabled}
					{new_thread_ready}
					{onpolicychange}
					onprimaryaction={ActivatePrimaryAction}
					onstartnewthread={StartNewThread}
					{policy}
					{run_active}
					{runtime_catalog}
					{send_blocked_reason}
					{send_ready}
				/>
			</div>
		</ShaderGlassSurface>

		{#snippet lip()}
			{#each pending_lip_state.pending as pending_steer (pending_steer.generation)}
				<div class="t-lip-row">
					<div class="t-lip-row-inner">
						<SteeringLip
							editable={onwithdraw !== undefined}
							ondiscard={queued_steer.Discard(pending_steer)}
							onedit={queued_steer.Edit(pending_steer)}
							text={pending_steer.submission.text.trim() || "Attached image"}
						/>
					</div>
				</div>
			{/each}
		{/snippet}
	</LipCard>
</div>

<ImageViewer
	bind:open={image_viewer_open}
	source={viewed_attachment?.preview_url}
	name={viewed_attachment?.name}
	onclose={ClearViewedAttachment}
/>
