<script lang="ts" effect>
	import ArrowUp from "@tabler/icons-svelte/icons/arrow-up";
	import X from "@tabler/icons-svelte/icons/x";
	import { onDestroy } from "svelte";
	import { Effect } from "effect";
	import { SnowflakeId } from "@artisan/protocol";
	import type { ThreadSessionPolicy } from "@artisan/protocol";
	import { Button } from "$lib/components/ui/button";
	import { BannerService } from "$lib/banner/service";
	import {
		MakeSubmitGate,
		type SubmitGate,
	} from "$lib/thread-interaction/commands";
	import {
		MakeComposerPlaceholderState,
		UpdateComposerPlaceholderState,
	} from "$lib/composer-placeholder";
	import {
		IsSupportedImageMimeType,
		MakeImageAttachmentParts,
		ValidateImageAttachmentCandidates,
		type ComposerImageAttachment,
		type ComposerSubmission,
	} from "$lib/composer/image-attachments";
	import ImageViewer from "./image-viewer.sv";
	import ModelSelector from "./model-selector.sv";
	import ShaderGlassSurface from "./shader-glass-surface.sv";

	const snowflake_id = yield* SnowflakeId;
	const banner = yield* BannerService;

	let {
		disabled = false,
		onpolicychange,
		onsubmit,
		policy,
	}: {
		disabled?: boolean;
		onpolicychange?: (policy: ThreadSessionPolicy) => void;
		onsubmit?: (submission: ComposerSubmission) => Effect.Effect<void, { readonly message: string }>;
		policy?: ThreadSessionPolicy;
	} = $props();

	let editor = $state<HTMLDivElement | null>(null);
	let attachments = $state<ReadonlyMap<string, ComposerImageAttachment>>(new Map());
	let draft = $state("");
	let image_viewer_open = $state(false);
	let submitting = $state(false);
	let viewed_attachment = $state<ComposerImageAttachment | undefined>();
	let placeholder = $state(MakeComposerPlaceholderState());
	const submit_gate: SubmitGate = yield* MakeSubmitGate;

	type EditorDocument = {
		readonly text: string;
		readonly tokens: ReadonlyArray<{ readonly id: string; readonly position: number }>;
	};

	const update_draft = (value: string, has_attachments = attachments.size > 0) => {
		draft = value;
		placeholder = UpdateComposerPlaceholderState(placeholder, has_attachments ? "\uFFFC" : value);
	};

	const read_editor_document = (): EditorDocument => {
		if (editor === null) return { text: draft, tokens: [] };
		let text = "";
		const tokens: Array<{ id: string; position: number }> = [];
		const visit = (node: Node) => {
			if (node.nodeType === Node.TEXT_NODE) {
				text += node.textContent?.replaceAll("\u200B", "") ?? "";
				return;
			}
			if (!(node instanceof HTMLElement)) return;
			const attachment_id = node.dataset.attachmentId;
			if (attachment_id !== undefined) {
				tokens.push({ id: attachment_id, position: text.length });
				return;
			}
			if (node.tagName === "BR") {
				text += "\n";
				return;
			}
			for (const child of node.childNodes) visit(child);
		};
		for (const node of editor.childNodes) visit(node);
		return { text, tokens };
	};

	const revoke_attachment = (attachment: ComposerImageAttachment | undefined) => {
		if (attachment !== undefined) URL.revokeObjectURL(attachment.preview_url);
	};

	const sync_editor = () => {
		const document = read_editor_document();
		const present = new Set(document.tokens.map((token) => token.id));
		const next = new Map<string, ComposerImageAttachment>();
		for (const [id, attachment] of attachments) {
			if (present.has(id)) next.set(id, attachment);
			else {
				if (viewed_attachment?.id === id) {
					image_viewer_open = false;
					viewed_attachment = undefined;
				}
				revoke_attachment(attachment);
			}
		}
		attachments = next;
		update_draft(document.text, next.size > 0);
		return document;
	};

	const focus_range = (range: Range) => {
		const selection = globalThis.getSelection();
		if (selection === null) return;
		selection.removeAllRanges();
		selection.addRange(range);
	};

	const end_range = () => {
		const range = document.createRange();
		if (editor === null) return range;
		range.selectNodeContents(editor);
		range.collapse(false);
		return range;
	};

	const selected_range = () => {
		const selection = globalThis.getSelection();
		if (
			editor === null ||
			selection === null ||
			selection.rangeCount === 0 ||
			!editor.contains(selection.getRangeAt(0).commonAncestorContainer)
		)
			return end_range();
		return selection.getRangeAt(0).cloneRange();
	};

	const attachment_marker = (attachment: ComposerImageAttachment) => {
		const marker = document.createElement("button");
		marker.type = "button";
		marker.contentEditable = "false";
		marker.dataset.attachmentId = attachment.id;
		marker.className = "composer-image-marker card";
		marker.setAttribute("aria-label", `View attached image ${attachment.name}`);
		marker.title = attachment.name;
		const thumbnail = document.createElement("img");
		thumbnail.alt = "";
		thumbnail.src = attachment.preview_url;
		marker.append(thumbnail);
		return marker;
	};

	const insert_attachments = (
		next_attachments: ReadonlyArray<ComposerImageAttachment>,
		range = selected_range(),
	) => {
		if (editor === null || next_attachments.length === 0) return;
		range.deleteContents();
		const fragment = document.createDocumentFragment();
		for (const attachment of next_attachments) {
			fragment.append(attachment_marker(attachment), document.createTextNode("\u200B"));
		}
		const trailing_caret = document.createTextNode("");
		fragment.append(trailing_caret);
		range.insertNode(fragment);
		const caret = document.createRange();
		caret.setStart(trailing_caret, 0);
		caret.collapse(true);
		focus_range(caret);
		sync_editor();
	};

	const remove_attachment = (attachment_id: string) => {
		const attachment = attachments.get(attachment_id);
		if (viewed_attachment?.id === attachment_id) image_viewer_open = false;
		for (const marker of editor?.querySelectorAll<HTMLElement>("[data-attachment-id]") ?? []) {
			if (marker.dataset.attachmentId === attachment_id) marker.remove();
		}
		revoke_attachment(attachment);
		attachments = new Map([...attachments].filter(([id]) => id !== attachment_id));
		sync_editor();
		editor?.focus();
	};

	const view_attachment = (attachment: ComposerImageAttachment) => {
		viewed_attachment = attachment;
		image_viewer_open = true;
	};

	const ReadFile = (file: File) =>
		Effect.gen(function* () {
			const id = yield* snowflake_id.Make("attachment");
			return yield* Effect.callback<ComposerImageAttachment, Error>((resume) => {
				const reader = new FileReader();
				reader.onerror = () =>
					resume(Effect.fail(reader.error ?? new Error(`Could not read ${file.name}.`)));
				reader.onload = () => {
					const source = typeof reader.result === "string" ? reader.result : undefined;
					const content_base64 = source?.slice(source.indexOf(",") + 1);
					if (!content_base64 || !IsSupportedImageMimeType(file.type)) {
						resume(Effect.fail(new Error(`Could not read ${file.name} as an image.`)));
						return;
					}
					resume(Effect.succeed({
						content_base64,
						id,
						mime_type: file.type,
						name: file.name || "Image",
						preview_url: URL.createObjectURL(file),
						size_bytes: file.size,
					}));
				};
				reader.readAsDataURL(file);

				return Effect.sync(() => {
					if (reader.readyState === FileReader.LOADING) reader.abort();
				});
			});
		});

	const AddFiles = (files: ReadonlyArray<File>, range = selected_range()) =>
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
			yield* Effect.forEach(files, (file) =>
				ReadFile(file).pipe(Effect.tap((attachment) => Effect.sync(() => loaded.push(attachment)))),
			).pipe(
				Effect.matchEffect({
					onFailure: (cause) =>
						Effect.gen(function* () {
						for (const attachment of loaded) revoke_attachment(attachment);
						yield* banner.error("Could not attach image", { description: cause.message });
					}),
					onSuccess: () => {
						attachments = new Map([...attachments, ...loaded.map((attachment) => [attachment.id, attachment])]);
						insert_attachments(loaded, range);
					},
				}),
			);
		});

	const reveal_placeholder = (node: HTMLElement) => {
		node.classList.remove("is-shown");
		void node.offsetHeight;
		const frame = requestAnimationFrame(() => node.classList.add("is-shown"));
		return { destroy: () => cancelAnimationFrame(frame) };
	};

	const character_delay = (index: number, length: number) => {
		if (length <= 1) return 0;
		const progress = index / (length - 1);
		return Math.round(((1 - Math.exp(-5 * progress)) / (1 - Math.exp(-5))) * (length - 1) * 20);
	};

	const Submit = Effect.gen(function* () {
		const document = sync_editor();
		const text = document.text;
		const attachment_parts = MakeImageAttachmentParts(attachments, document.tokens);
		if (disabled || submitting || (text.length === 0 && attachment_parts.length === 0) || onsubmit === undefined)
			return;
		if (!(yield* submit_gate.Acquire)) return;
		submitting = true;

		yield* onsubmit({ attachments: attachment_parts, text }).pipe(
			Effect.matchEffect({
				onFailure: (error) =>
					banner.error("Could not send message", { description: error.message }),
				onSuccess: () => Effect.sync(() => {
					for (const attachment of attachments.values()) revoke_attachment(attachment);
					attachments = new Map();
					editor?.replaceChildren();
					update_draft("");
				}),
			}),
			Effect.ensuring(Effect.all([submit_gate.Release, Effect.sync(() => { submitting = false; })])),
		);
	});

	const HandleComposerKey = (event: KeyboardEvent) =>
		Effect.gen(function* () {
			if (event.isComposing || event.key !== "Enter" || event.shiftKey) return;
			event.preventDefault();
			yield* Submit;
		});

	const HandlePaste = (event: ClipboardEvent) => Effect.gen(function* () {
		const files = [...(event.clipboardData?.files ?? [])].filter((file) => file.type.startsWith("image/"));
		if (files.length === 0) return;
		event.preventDefault();
		yield* AddFiles(files);
	});

	const HandleDrop = (event: DragEvent) => Effect.gen(function* () {
		const files = [...(event.dataTransfer?.files ?? [])].filter((file) => file.type.startsWith("image/"));
		if (files.length === 0) return;
		event.preventDefault();
		const point = document.caretRangeFromPoint?.(event.clientX, event.clientY);
		yield* AddFiles(files, point !== null && point !== undefined && editor?.contains(point.commonAncestorContainer) ? point : selected_range());
	});

	const handle_editor_click = (event: MouseEvent) => {
		const target = event.target;
		if (!(target instanceof HTMLElement)) return;
		const marker = target.closest<HTMLElement>("[data-attachment-id]");
		if (marker?.dataset.attachmentId === undefined) return;
		const attachment = attachments.get(marker.dataset.attachmentId);
		if (attachment !== undefined) view_attachment(attachment);
	};

	onDestroy(() => {
		for (const attachment of attachments.values()) revoke_attachment(attachment);
	});
</script>

<div class="pointer-events-none absolute inset-x-0 bottom-0 z-20 px-4 pb-4 sm:px-6 sm:pb-6">
	<ShaderGlassSurface
		class="t-resize thread-composer pointer-events-auto mx-auto w-full max-w-3xl rounded-3xl"
		data-has-attachments={attachments.size > 0}
	>
		<div class="relative z-10 flex min-h-32 flex-col p-2">
			<div class="composer-attachment-tray" aria-label="Attached images">
				<div class="composer-attachment-tray-content">
					{#each [...attachments.values()] as attachment (attachment.id)}
						<div class="composer-attachment-preview card">
							<button type="button" class="composer-attachment-preview-trigger" aria-label={`View ${attachment.name}`} onclick={() => view_attachment(attachment)}>
								<img src={attachment.preview_url} alt={attachment.name} />
							</button>
							<Button variant="secondary" size="icon-sm" class="composer-attachment-remove" aria-label={`Remove ${attachment.name}`} onclick={() => remove_attachment(attachment.id)}>
								<X class="size-3.5" />
							</Button>
						</div>
					{/each}
				</div>
			</div>

			<div class="relative min-h-16 flex-1">
				{#if placeholder.visible}
					{#key placeholder.generation}
						{@const characters = Array.from(placeholder.phrase)}
						<div use:reveal_placeholder aria-hidden="true" class="placeholder-reveal pointer-events-none absolute inset-x-3 top-2 text-base text-muted-foreground">
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
					oninput={sync_editor}
					onkeydown={yield* HandleComposerKey(event)}
					onkeyup={() => queueMicrotask(sync_editor)}
					onpaste={yield* HandlePaste(event)}
					ondragover={(event) => { if ([...(event.dataTransfer?.types ?? [])].includes("Files")) event.preventDefault(); }}
					ondrop={yield* HandleDrop(event)}
					onclick={handle_editor_click}
				></div>
			</div>

			<div class="flex items-center justify-between gap-2">
				<ModelSelector {disabled} {policy} {onpolicychange} />
				<Button size="icon" aria-label="Send message" disabled={disabled || submitting || (draft.trim().length === 0 && attachments.size === 0) || onsubmit === undefined} onclick={yield* Submit}><ArrowUp /></Button>
			</div>
		</div>
	</ShaderGlassSurface>
</div>

<ImageViewer
	bind:open={image_viewer_open}
	source={viewed_attachment?.preview_url}
	name={viewed_attachment?.name}
	onclose={() => {
		viewed_attachment = undefined;
	}}
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
		--composer-resize-dur: var(--resize-dur);
		--composer-resize-ease: var(--resize-ease);
	}
	.composer-attachment-tray { display: grid; grid-template-rows: 0fr; opacity: 0; transition: grid-template-rows var(--composer-resize-dur) var(--composer-resize-ease), opacity 150ms ease-out; }
	:global(.thread-composer[data-has-attachments="true"]) .composer-attachment-tray { grid-template-rows: 1fr; opacity: 1; }
	.composer-attachment-tray-content { min-height: 0; display: flex; gap: .5rem; overflow: hidden; padding: 0; transition: padding var(--composer-resize-dur) var(--composer-resize-ease); }
	:global(.thread-composer[data-has-attachments="true"]) .composer-attachment-tray-content { padding: .25rem .25rem .5rem; }
	.composer-attachment-preview { position: relative; width: 4.5rem; height: 4.5rem; flex: none; overflow: hidden; border-radius: .9rem; opacity: 0; transform: translateY(8px) scale(.96); transition: opacity 180ms var(--composer-resize-ease), transform var(--composer-resize-dur) var(--composer-resize-ease); }
	:global(.thread-composer[data-has-attachments="true"]) .composer-attachment-preview { opacity: 1; transform: translateY(0) scale(1); }
	.composer-attachment-preview-trigger { display: block; width: 100%; height: 100%; padding: 0; border: 0; background: transparent; cursor: pointer; }
	.composer-attachment-preview-trigger:focus-visible { outline: 2px solid var(--ring); outline-offset: -2px; }
	.composer-attachment-preview img { width: 100%; height: 100%; object-fit: cover; }
	:global(.composer-attachment-remove) { position: absolute; top: .2rem; right: .2rem; min-width: 1.35rem; width: 1.35rem; height: 1.35rem; border-radius: 999px; background: rgb(255 255 255 / .92); color: #18181b; }
	.composer-editor { white-space: pre-wrap; overflow-wrap: anywhere; cursor: text; }
	.composer-editor:empty::before { content: ""; }
	:global(.composer-image-marker) { display: inline-flex; width: 1rem; height: 1rem; margin: 0 .12rem; padding: 0; overflow: hidden; vertical-align: -.12rem; border: 0; border-radius: .22rem; cursor: pointer; background: #27272a; }
	:global(.composer-image-marker:focus-visible) { outline: 2px solid var(--ring); outline-offset: 2px; }
	:global(.composer-image-marker img) { width: 100%; height: 100%; object-fit: cover; pointer-events: none; }
	.placeholder-reveal-line { display: block; white-space: pre; }
	.placeholder-character { display: inline-block; opacity: 0; transform: translateY(2px); transition: opacity 160ms cubic-bezier(.22,1,.36,1), transform 280ms cubic-bezier(.34,1.56,.64,1); transition-delay: var(--placeholder-delay); will-change: opacity, transform; }
	.placeholder-reveal:global(.is-shown) .placeholder-character { opacity: 1; transform: translateY(0); }
	@media (prefers-reduced-motion: reduce) {
		:global(.t-resize),
		.composer-attachment-tray,
		.composer-attachment-tray-content,
		.composer-attachment-preview,
		.placeholder-character {
			transition: none !important;
			will-change: auto;
		}
	}
</style>
