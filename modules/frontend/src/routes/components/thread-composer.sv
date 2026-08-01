<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import type { SurfaceUsageAggregate, ThreadSessionPolicy } from "@artisan/protocol";
	import { BannerService } from "$lib/banner/service";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { ReleaseBrowserObjectUrl } from "$lib/browser/object-url";
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
		ValidateImageAttachmentCandidates,
		type ComposerImageAttachment,
		type ComposerSubmission,
	} from "$lib/composer/image-attachments";
	import { ReadComposerImageFile } from "$lib/composer/attachment-reader";
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
		onpolicychange,
		onsubmit,
		policy,
		run_active = false,
	}: {
		context_usage?: SurfaceUsageAggregate;
		disabled?: boolean;
		engine_locked?: boolean;
		onabort?: () => Effect.Effect<unknown, { readonly message: string }>;
		onpolicychange?: (
			policy: ThreadSessionPolicy,
		) => Effect.Effect<ThreadSessionPolicy, { readonly message: string }>;
		onsubmit?: (
			submission: ComposerSubmission,
		) => Effect.Effect<unknown, { readonly message: string }>;
		policy?: ThreadSessionPolicy;
		run_active?: boolean;
	} = $props();

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
	let cancelling = $state(false);
	let draft = $state("");
	let image_viewer_open = $state(false);
	let submitting = $state(false);
	let viewed_attachment = $state<ComposerImageAttachment | undefined>();
	let placeholder = $state(MakeComposerPlaceholderState());
	const submit_gate: SubmitGate = yield* MakeSubmitGate;

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

	const AddFiles = (files: ReadonlyArray<File>, range?: Range) =>
		Effect.gen(function* () {
			if (disabled || submitting) return;
			const validation = ValidateImageAttachmentCandidates(
				[...attachments.values()],
				files.map((file) => ({ name: file.name, size_bytes: file.size, type: file.type })),
			);
			if (validation !== undefined) {
				yield* banner.error("Could not attach image", { description: validation });
				return;
			}
			const loaded: Array<ComposerImageAttachment> = [];
			yield* Effect.gen(function* () {
				for (const file of files) loaded.push(yield* ReadComposerImageFile(file));
			}).pipe(
				Effect.matchEffect({
					onFailure: (cause) =>
						Effect.gen(function* () {
							for (const attachment of loaded) yield* RevokeAttachment(attachment);
							yield* banner.error("Could not attach image", { description: cause.message });
						}),
					onSuccess: () =>
						Effect.gen(function* () {
							attachments = new Map([...attachments, ...loaded.map((attachment) => [attachment.id, attachment])]);
							yield* InsertAttachments(
								loaded,
								range ?? (yield* ComposerSelectedRange(editor)),
							);
						}),
				}),
			);
		});

	const character_delay = (index: number, length: number) => {
		if (length <= 1) return 0;
		const progress = index / (length - 1);
		return Math.round(((1 - Math.exp(-5 * progress)) / (1 - Math.exp(-5))) * (length - 1) * 20);
	};

	const Submit = Effect.gen(function* () {
		const document = yield* SyncEditor();
		const text = document.text;
		const attachment_parts = MakeImageAttachmentParts(attachments, document.tokens);
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

	const HandleComposerKey = (event: KeyboardEvent) =>
		Effect.gen(function* () {
			if (event.isComposing || event.key !== "Enter" || event.shiftKey) return;
			yield* RunBrowserDom(() => event.preventDefault());
			yield* Submit;
		});

	const HandlePaste = (event: ClipboardEvent) =>
		Effect.gen(function* () {
			const files = [...(event.clipboardData?.files ?? [])].filter((file) => file.type.startsWith("image/"));
			if (files.length === 0) return;
			yield* RunBrowserDom(() => event.preventDefault());
			yield* AddFiles(files);
		});

	const HandleDrop = (event: DragEvent) =>
		Effect.gen(function* () {
			const files = [...(event.dataTransfer?.files ?? [])].filter((file) =>
				file.type.startsWith("image/"),
			);
			if (files.length === 0) return;
			yield* RunBrowserDom(() => event.preventDefault());
			const range =
				(yield* ComposerDropRange(editor, event)) ??
				(yield* ComposerSelectedRange(editor));
			yield* AddFiles(files, range);
		});

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
	const HandleDragOver = (event: DragEvent) =>
		Effect.gen(function* () {
			if ([...(event.dataTransfer?.types ?? [])].includes("Files")) {
				yield* RunBrowserDom(() => event.preventDefault());
			}
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

<div class="pointer-events-none absolute inset-x-0 bottom-0 z-20 px-4 pb-4 sm:px-6 sm:pb-6">
	<ShaderGlassSurface
		class="t-resize thread-composer pointer-events-auto mx-auto w-full max-w-3xl rounded-(--composer-radius)"
		data-has-attachments={attachments.size > 0}
	>
		<div class="relative z-10 flex min-h-32 flex-col p-2">
			<AttachmentTray {attachments} onremove={RemoveAttachment} onview={ViewAttachment} />

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
					onkeydown={yield* HandleComposerKey(event)}
					onkeyup={yield* QueueEditorSync}
					onpaste={yield* HandlePaste(event)}
					ondragover={yield* HandleDragOver(event)}
					ondrop={yield* HandleDrop(event)}
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
	:global(:root) {
		--resize-dur: 300ms;
		--resize-ease: cubic-bezier(0.22, 1, 0.36, 1);
	}

	:global(.t-resize) {
		transition:
			width var(--resize-dur) var(--resize-ease),
			height var(--resize-dur) var(--resize-ease);
		will-change: width, height;
	}

	:global(.thread-composer) {
		--composer-radius: calc(var(--radius-3xl) - 0.375rem);
		--composer-resize-dur: var(--resize-dur);
		--composer-resize-ease: var(--resize-ease);
	}
	.composer-editor { white-space: pre-wrap; overflow-wrap: anywhere; cursor: text; }
	.composer-editor:empty::before { content: ""; }
	:global(.composer-image-marker) { display: inline-flex; width: 1rem; height: 1rem; margin: 0 .12rem; padding: 0; overflow: hidden; vertical-align: -.12rem; border: 0; border-radius: .22rem; cursor: pointer; background: #27272a; }
	:global(.composer-image-marker:focus-visible) { outline: 2px solid var(--ring); outline-offset: 2px; }
	:global(.composer-image-marker img) { width: 100%; height: 100%; object-fit: cover; pointer-events: none; }
	.placeholder-reveal-line { display: block; white-space: pre; }
	.placeholder-character { display: inline-block; animation: placeholder-reveal-in 280ms cubic-bezier(.34,1.56,.64,1) var(--placeholder-delay) both; will-change: opacity, transform; }
	@keyframes placeholder-reveal-in { from { opacity: 0; transform: translateY(2px); } to { opacity: 1; transform: translateY(0); } }
	@media (prefers-reduced-motion: reduce) {
		:global(.t-resize),
		.placeholder-character {
			transition: none !important;
			will-change: auto;
		}
	}
</style>
