<script lang="ts" effect>
	import ChevronDown from "@tabler/icons-svelte/icons/chevron-down";
	import { tick } from "svelte";
	import { Effect, Stream } from "effect";
	import type { SurfaceUsageAggregate, ThreadSessionPolicy } from "@artisan/protocol";
	import { BannerService } from "$lib/banner/service";
	import { ReleaseBrowserObjectUrl } from "$lib/browser/object-url";
	import { Button } from "$lib/components/ui/button";
	import {
		MakeComposerGestureIntake,
		type ComposerGesture,
	} from "$lib/composer/gesture-intake";
	import {
		MakeSubmitGate,
		type SubmitGate,
	} from "$lib/thread-interaction/commands";
	import {
		MakeComposerPlaceholderState,
		UpdateComposerPlaceholderState,
	} from "$lib/composer-placeholder";
	import {
		MakeImageAttachmentParts,
		type ComposerImageAttachment,
		type ComposerSubmission,
	} from "$lib/composer/image-attachments";
	import { MakeComposerAttachmentIntake } from "$lib/composer/attachment-intake";
	import {
		ClearComposerEditor,
		ComposerAttachmentIdAtEvent,
		ComposerDropRange,
		ComposerSelectedRange,
		FocusComposerEditor,
		FocusComposerRange,
		InsertComposerAttachmentMarkers,
		MarkComposerAttachmentBumps,
		ReadComposerEditorDocument,
		RemoveComposerAttachmentMarkers,
		type ComposerEditorDocument,
	} from "./composer/dom";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import ImageViewer from "./image-viewer.sv";
	import ShaderGlassSurface from "./shader-glass-surface.sv";
	import AttachmentTray from "./composer/attachment-tray.sv";
	import ComposerControls from "./composer/controls.sv";

	const banner = yield* BannerService;
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
		engine_locked = false,
		onabort,
		onjumptolatest,
		onpolicychange,
		onsubmit,
		policy,
		run_active = false,
		show_jump_to_latest = false,
	}: {
		context_usage?: SurfaceUsageAggregate;
		disabled?: boolean;
		engine_locked?: boolean;
		onabort?: () => Effect.Effect<unknown, { readonly message: string }>;
		onjumptolatest?: Effect.Effect<void>;
		onpolicychange?: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<ThreadSessionPolicy, { readonly message: string }>;
		onsubmit?: (
			submission: ComposerSubmission,
		) => Effect.Effect<unknown, { readonly message: string }>;
		policy?: ThreadSessionPolicy;
		run_active?: boolean;
		show_jump_to_latest?: boolean;
	} = $props();

	const JumpToLatest = Effect.gen(function* () {
		if (onjumptolatest !== undefined) yield* onjumptolatest;
	});

	/**
	 * Sending needs a live engine behind it. Without Forge there is no session
	 * to run at all, and within a connected catalog a model whose harness is
	 * unregistered can still be picked and read but never run.
	 */
	const send_blocked_reason = $derived.by(() => {
		if (!defaults_state.available) return "Forge is offline — reconnect to send";
		const engine_id = policy?.engine_id;
		if (engine_id === undefined) return undefined;
		if (runtime_catalog.runnable_harness_ids.includes(engine_id)) return undefined;
		const label =
			runtime_catalog.manifest.harnesses.find((harness) => harness.id === engine_id)?.label ??
			engine_id;
		return `${label} models are preview-only — this engine cannot run in Artisan yet`;
	});

	/**
	 * The context-window denominator. A provider that discloses its usable
	 * window on the wire (Codex) wins; otherwise the catalog's configured
	 * context-window option for the thread's model stands in (Claude never
	 * reports a window size). Models without the capability show no gauge.
	 */
	const context_window_tokens = $derived.by(() => {
		if (context_usage?.context_window_tokens !== undefined)
			return context_usage.context_window_tokens;
		const capability = runtime_catalog.manifest.models.find(
			(model) => model.native_model_id === policy?.model,
		)?.capabilities.context_window;
		if (capability === undefined) return undefined;
		const option =
			capability.options.find((candidate) =>
				policy?.context_window === undefined
					? candidate.native_suffix === ""
					: candidate.native_suffix === policy.context_window,
			) ?? capability.options.find((candidate) => candidate.id === capability.default);
		return option?.tokens;
	});
	const context_percent = $derived(
		context_usage?.context_tokens === undefined || context_window_tokens === undefined
			? undefined
			: Math.min(100, (context_usage.context_tokens / context_window_tokens) * 100),
	);

	let editor = $state<HTMLDivElement | null>(null);
	let attachments = $state<ReadonlyMap<string, ComposerImageAttachment>>(new Map());
	let rustling = $state(false);
	let bumped = $state.raw<ReadonlySet<string>>(new Set());
	let bump_generation = 0;
	let cancelling = $state(false);
	let draft = $state("");
	let image_viewer_open = $state(false);
	let submitting = $state(false);
	let viewed_attachment = $state<ComposerImageAttachment | undefined>();
	let placeholder = $state(MakeComposerPlaceholderState());
	const submit_gate: SubmitGate = yield* MakeSubmitGate;

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

	/**
	 * Replays the favourite rustle on the composer whenever images land. The flag
	 * drops for a frame first so a second paste restarts the animation instead of
	 * joining one already running.
	 */
	const StartRustle = Effect.gen(function* () {
		rustling = false;
		yield* Effect.promise(() => tick());
		rustling = true;
	});
	const EndRustle = Effect.gen(function* () {
		rustling = false;
	});

	/**
	 * Shakes the attachments a re-paste was answered by. The flag drops for a
	 * frame first so spamming the same image shakes it every time rather than
	 * once, and the generation makes the newest bump own the reset.
	 */
	const BumpAttachments = (ids: ReadonlyArray<string>) =>
		Effect.gen(function* () {
			const generation = (bump_generation += 1);
			bumped = new Set();
			yield* MarkComposerAttachmentBumps(editor, []);
			yield* Effect.promise(() => tick());
			if (generation !== bump_generation) return;
			bumped = new Set(ids);
			yield* MarkComposerAttachmentBumps(editor, ids);
			yield* Effect.sleep("420 millis");
			if (generation !== bump_generation) return;
			bumped = new Set();
			yield* MarkComposerAttachmentBumps(editor, []);
		});

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

	const SyncEditor = (): Effect.Effect<ComposerEditorDocument> =>
		Effect.gen(function* () {
			const document = yield* ReadComposerEditorDocument(editor, draft);
			const present = new Set(document.tokens.map((token) => token.id));
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
			yield* UpdateDraft(document.text, next.size > 0);
			return document;
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
		});

	const DropAttachment = (attachment: ComposerImageAttachment) =>
		Effect.gen(function* () {
			yield* RemoveComposerAttachmentMarkers(editor, attachment.id);
			yield* RevokeAttachment(attachment);
			attachments = new Map([...attachments].filter(([id]) => id !== attachment.id));
		});

	/**
	 * Placing an image is its own ordered job — validate, recognise a re-paste,
	 * show, encode, settle — so it lives beside the other composer intake. What
	 * stays here is the part only the component can do: its state and its motion.
	 */
	const intake = MakeComposerAttachmentIntake({
		Attachments: () => attachments,
		Blocked: () => disabled || submitting,
		Bump: BumpAttachments,
		EngineId: () => policy?.engine_id,
		Present: (added, range) =>
			Effect.gen(function* () {
				attachments = new Map([
					...attachments,
					...added.map((attachment) => [attachment.id, attachment] as const),
				]);
				yield* InsertAttachments(added, range ?? (yield* ComposerSelectedRange(editor)));
				yield* StartRustle;
			}),
		Remove: DropAttachment,
		Report: (description) =>
			Effect.gen(function* () {
				yield* banner.error("Could not attach image", { description });
			}),
		Revoke: RevokeAttachment,
		Settle: SettleAttachment,
	});
	const AddFiles = intake.AddFiles;

	const character_delay = (index: number, length: number) => {
		if (length <= 1) return 0;
		const progress = index / (length - 1);
		return Math.round(((1 - Math.exp(-5 * progress)) / (1 - Math.exp(-5))) * (length - 1) * 20);
	};

	const Submit = Effect.gen(function* () {
		const document = yield* SyncEditor();
		const text = document.text;
		const attachment_parts = MakeImageAttachmentParts(attachments, document.tokens);
		/** Silently declined rather than refused: the encode finishes in a moment. */
		if (!attachments_ready) return;
		if (disabled || submitting || (text.length === 0 && attachment_parts.length === 0) || onsubmit === undefined)
			return;
		if (!(yield* submit_gate.Acquire)) return;
		submitting = true;

		const CompleteSubmission = Effect.gen(function* () {
			for (const attachment of attachments.values()) yield* RevokeAttachment(attachment);
			attachments = new Map();
			yield* ClearComposerEditor(editor);
			yield* UpdateDraft("");
		});
		const ReleaseSubmission = Effect.gen(function* () {
			yield* submit_gate.Release;
			submitting = false;
		});

		yield* onsubmit({ attachments: attachment_parts, text }).pipe(
			Effect.matchEffect({
				onFailure: (error) =>
					Effect.gen(function* () {
						yield* banner.error("Could not send message", { description: error.message });
					}),
				onSuccess: () =>
					Effect.gen(function* () {
						yield* CompleteSubmission;
					}),
			}),
			Effect.ensuring(ReleaseSubmission),
		);
	});

	const Cancel = Effect.gen(function* () {
		if (disabled || cancelling || onabort === undefined) return;
		cancelling = true;

		yield* onabort().pipe(
			Effect.catch((error) =>
				Effect.gen(function* () {
					cancelling = false;
					yield* banner.error("Could not stop run", { description: error.message });
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

	yield* Effect.addFinalizer(() =>
		Effect.gen(function* () {
			for (const attachment of attachments.values()) yield* RevokeAttachment(attachment);
		}),
	);

	yield* ResetCancelling;
</script>

<div class="pointer-events-none absolute inset-x-0 bottom-0 z-20 flex flex-col items-center gap-2 px-4 pb-4 sm:px-6 sm:pb-6">
	{#if show_jump_to_latest && onjumptolatest !== undefined}
		<Button
			variant="outline"
			size="icon-sm"
			class="pointer-events-auto rounded-full bg-surface-100/90 text-muted-foreground shadow-lg backdrop-blur-xl hover:text-foreground"
			aria-label="Jump to latest"
			title="Jump to latest"
			onclick={yield* JumpToLatest}
		>
			<ChevronDown class="size-4" aria-hidden="true" />
		</Button>
	{/if}
	<ShaderGlassSurface
		class="t-resize thread-composer pointer-events-auto mx-auto w-full max-w-(--prose-width) rounded-(--composer-radius)"
		data-has-attachments={attachments.size > 0}
		data-rustle={rustling}
		onanimationend={yield* EndRustle}
	>
		<div class="relative z-10 flex min-h-32 flex-col p-2">
			<AttachmentTray {attachments} {bumped} onremove={RemoveAttachment} onview={ViewAttachment} />

			<div class="relative min-h-16 flex-1">
				{#if placeholder.visible}
					{#key placeholder.generation}
						{@const characters = Array.from(placeholder.phrase)}
						<div aria-hidden="true" class="placeholder-reveal pointer-events-none absolute inset-x-3 top-2 text-base text-muted-foreground">
							<span class="placeholder-reveal-line">
								{#each characters as character, index}
									<span class="placeholder-character" style={`--placeholder-delay: ${character_delay(index, characters.length)}ms`}>{character}</span>
								{/each}
							</span>
						</div>
					{/key}
				{/if}
				<div
					bind:this={editor}
					class="composer-editor size-full min-h-16 px-3 py-2 text-base outline-none"
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

			<ComposerControls
				abort_available={onabort !== undefined}
				{cancelling}
				{context_percent}
				{context_usage}
				{context_window_tokens}
				{disabled}
				{engine_locked}
				{onpolicychange}
				onprimaryaction={ActivatePrimaryAction}
				{policy}
				{run_active}
				{runtime_catalog}
				{send_blocked_reason}
				{send_ready}
			/>
		</div>
	</ShaderGlassSurface>
</div>

<ImageViewer
	bind:open={image_viewer_open}
	source={viewed_attachment?.preview_url}
	name={viewed_attachment?.name}
	onclose={ClearViewedAttachment}
/>

<style>
	.composer-editor {
		white-space: pre-wrap;
		overflow-wrap: anywhere;
		cursor: text;
	}
	.composer-editor:empty::before {
		content: "";
	}
	.placeholder-reveal-line {
		display: block;
		white-space: pre;
	}
	.placeholder-character {
		display: inline-block;
		animation: placeholder-reveal-in 280ms cubic-bezier(0.34, 1.56, 0.64, 1)
			var(--placeholder-delay) both;
		will-change: opacity, transform;
	}

	@keyframes placeholder-reveal-in {
		from {
			opacity: 0;
			transform: translateY(2px);
		}
		to {
			opacity: 1;
			transform: translateY(0);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.placeholder-character {
			transition: none !important;
			will-change: auto;
		}
	}
</style>
