<script lang="ts" effect>
	import { Duration, Effect } from "effect";
	import { IconFileSearch as FileSearch, IconSearch as Search } from "@tabler/icons-svelte";

	import type { WorkspaceFileReference } from "$lib/workspace/workspace-tab-model";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";

	let {
		files,
		on_open,
	}: {
		files: ReadonlyArray<WorkspaceFileReference>;
		on_open: (file_id: string) => Effect.Effect<void>;
	} = $props();

	let is_open = $state(false);
	let is_closing = $state(false);
	let query = $state("");
	let selected_index = $state(0);
	let results = $state.raw<ReadonlyArray<WorkspaceFileReference>>([]);
	let transition_generation = $state(0);
	let trigger = $state<HTMLButtonElement>();
	let return_focus = $state<HTMLElement>();
	let search_input = $state<HTMLInputElement>();
	let dialog = $state<HTMLDivElement>();

	const FilterFiles = (value: string) =>
		Effect.gen(function* () {
			yield* Effect.void;

			const normalized = value.trim().toLocaleLowerCase();
			const filtered: Array<WorkspaceFileReference> = [];
			for (const file of files) {
				if (
					normalized.length === 0 ||
					file.name.toLocaleLowerCase().includes(normalized) ||
					file.path.toLocaleLowerCase().includes(normalized)
				) {
					filtered.push(file);
				}
			}

			return filtered;
		});

	const RevealSelectedResult = Effect.gen(function* () {
		yield* Effect.sleep(Duration.millis(0));
		document
			.getElementById(`quick-open-result-${selected_index}`)
			?.scrollIntoView({ block: "nearest" });
	});

	const UpdateQuery = (value: string) =>
		Effect.gen(function* () {
			query = value;
			selected_index = 0;
			results = yield* FilterFiles(value);
			yield* RevealSelectedResult;
		});

	const OpenQuickOpen = Effect.gen(function* () {
		const active_element = document.activeElement;
		if (
			return_focus === undefined ||
			dialog === undefined ||
			!dialog.contains(active_element)
		) {
			return_focus = active_element instanceof HTMLElement ? active_element : trigger;
		}
		transition_generation += 1;
		is_closing = false;
		is_open = true;
		query = "";
		selected_index = 0;
		results = yield* FilterFiles("");
		yield* Effect.sleep(Duration.millis(0));
		search_input?.focus();
	});

	const ModalCloseDuration = Effect.gen(function* () {
		const raw_duration = getComputedStyle(document.documentElement)
			.getPropertyValue("--modal-close-dur")
			.trim();
		const parsed_duration = Number.parseFloat(raw_duration);
		if (!Number.isFinite(parsed_duration)) {
			return 150;
		}

		return raw_duration.endsWith("s") && !raw_duration.endsWith("ms")
			? parsed_duration * 1000
			: parsed_duration;
	});

	const CloseQuickOpen = Effect.gen(function* () {
		if (!is_open || is_closing) {
			return;
		}

		is_open = false;
		is_closing = true;
		transition_generation += 1;
		const close_generation = transition_generation;
		const close_duration = yield* ModalCloseDuration;
		yield* Effect.sleep(Duration.millis(close_duration));
		if (transition_generation !== close_generation || is_open) {
			return;
		}
		is_closing = false;
		if (return_focus?.isConnected) {
			return_focus.focus();
		} else {
			trigger?.focus();
		}
		return_focus = undefined;
	});

	const ChooseFile = (file_id: string) =>
		Effect.gen(function* () {
			yield* on_open(file_id);
			yield* CloseQuickOpen;
		});

	const MoveSelection = (direction: -1 | 1) =>
		Effect.gen(function* () {
			if (results.length === 0) {
				selected_index = 0;
				return;
			}

			selected_index = (selected_index + direction + results.length) % results.length;
			yield* RevealSelectedResult;
		});

	const TrapDialogFocus = (keyboard_event: KeyboardEvent) =>
		Effect.gen(function* () {
			yield* Effect.void;

			if (dialog === undefined) {
				return;
			}

			const focusable = dialog.querySelectorAll<HTMLElement>(
				'input, button:not([disabled]), [tabindex]:not([tabindex="-1"])',
			);
			if (focusable.length === 0) {
				return;
			}

			const first = focusable[0]!;
			const last = focusable[focusable.length - 1]!;
			const active = document.activeElement;

			if (!dialog.contains(active)) {
				keyboard_event.preventDefault();
				if (keyboard_event.shiftKey) {
					last.focus();
				} else {
					first.focus();
				}
			} else if (keyboard_event.shiftKey && active === first) {
				keyboard_event.preventDefault();
				last.focus();
			} else if (!keyboard_event.shiftKey && active === last) {
				keyboard_event.preventDefault();
				first.focus();
			}
		});

	const HandleGlobalKeydown = (keyboard_event: KeyboardEvent) =>
		Effect.gen(function* () {
			if ((keyboard_event.ctrlKey || keyboard_event.metaKey) && keyboard_event.key.toLocaleLowerCase() === "p") {
				keyboard_event.preventDefault();
				if (!is_open) {
					yield* OpenQuickOpen;
				}
				return;
			}

			if (!is_open) {
				return;
			}

			if (keyboard_event.key === "Tab") {
				yield* TrapDialogFocus(keyboard_event);
			} else if (keyboard_event.key === "ArrowDown") {
				keyboard_event.preventDefault();
				yield* MoveSelection(1);
			} else if (keyboard_event.key === "ArrowUp") {
				keyboard_event.preventDefault();
				yield* MoveSelection(-1);
			} else if (keyboard_event.key === "Enter") {
				keyboard_event.preventDefault();
				const selected = results[selected_index];
				if (selected !== undefined) {
					yield* ChooseFile(selected.id);
				}
			} else if (keyboard_event.key === "Escape") {
				keyboard_event.preventDefault();
				yield* CloseQuickOpen;
			}
		});

	const CloseFromBackdrop = (pointer_event: PointerEvent) =>
		Effect.gen(function* () {
			if (pointer_event.target === pointer_event.currentTarget) {
				yield* CloseQuickOpen;
			}
		});
</script>

<svelte:window onkeydown={yield* HandleGlobalKeydown(event)} />

	<Button variant="outline" size="xs" class="quick-open-trigger" bind:ref={trigger} onclick={yield* OpenQuickOpen} aria-haspopup="dialog" aria-expanded={is_open}>
	<FileSearch size={14} stroke={1.7} aria-hidden="true" />
	<span>Quick open</span>
	<kbd>Ctrl P</kbd>
	</Button>

{#if is_open || is_closing}
	<div class:closing={is_closing} class="quick-open-backdrop" role="presentation" onpointerdown={yield* CloseFromBackdrop(event)}>
		<div bind:this={dialog} class:is-open={is_open} class:is-closing={is_closing} class="quick-open-dialog t-modal" role="dialog" aria-modal="true" aria-labelledby="quick-open-title" tabindex="-1">
			<header>
				<Search size={16} stroke={1.7} aria-hidden="true" />
				<Input bind:ref={search_input} value={query} oninput={yield* UpdateQuery(event.currentTarget.value)} aria-label="Search fixture files" placeholder="Search files by name or path" autocomplete="off" />
				<kbd>Esc</kbd>
			</header>
			<div class="quick-open-heading">
				<strong id="quick-open-title">Fixture files</strong>
				<span>{results.length} matches</span>
			</div>
			<div class="quick-open-results" id="quick-open-results">
				{#each results as file, index}
					<Button variant="ghost" id={`quick-open-result-${index}`} aria-current={selected_index === index ? "true" : undefined} onclick={yield* ChooseFile(file.id)}>
						<span>{file.name}</span><small>{file.path}</small>
					</Button>
				{:else}
					<p>No fixture files match “{query}”.</p>
				{/each}
			</div>
		</div>
	</div>
{/if}

<style>
	.quick-open-trigger {
		display: flex;
		height: 28px;
		align-items: center;
		gap: 7px;
		padding: 0 7px 0 9px;
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		background: var(--pane-inset);
		color: var(--text-muted);
		font-size: 10px;
		cursor: pointer;
	}

	.quick-open-trigger:hover {
		background: var(--raised);
		color: var(--text-primary);
	}

	kbd {
		padding: 2px 5px;
		border: 1px solid var(--line);
		border-radius: 4px;
		background: var(--canvas);
		color: var(--text-muted);
		font: 8px/1.2 var(--font-mono);
	}

	.quick-open-backdrop {
		position: fixed;
		z-index: 80;
		inset: 0;
		display: grid;
		place-items: start center;
		padding: min(16vh, 140px) 18px 18px;
		background: color-mix(in oklch, var(--canvas) 72%, transparent);
		backdrop-filter: blur(5px);
		opacity: 1;
		transition: opacity var(--modal-close-dur) var(--modal-ease);
	}

	.quick-open-backdrop.closing {
		opacity: 0;
	}

	.t-modal {
		transform-origin: center;
		transform: scale(var(--modal-scale));
		opacity: 0;
		pointer-events: none;
		transition:
			transform var(--modal-open-dur) var(--modal-ease),
			opacity var(--modal-open-dur) var(--modal-ease);
		will-change: transform, opacity;
	}

	.t-modal.is-open {
		transform: scale(1);
		opacity: 1;
		pointer-events: auto;
	}

	.t-modal.is-closing {
		transform: scale(var(--modal-scale-close));
		opacity: 0;
		pointer-events: none;
		transition:
			transform var(--modal-close-dur) var(--modal-ease),
			opacity var(--modal-close-dur) var(--modal-ease);
	}

	.quick-open-dialog {
		width: min(620px, 100%);
		max-height: min(560px, 72vh);
		overflow: hidden;
		border: 1px solid var(--line-strong);
		border-radius: var(--radius-pane);
		background: var(--raised);
		box-shadow: var(--shadow-overlay);
	}

	.quick-open-dialog header {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto;
		align-items: center;
		gap: 10px;
		padding: 10px 12px;
		border-bottom: 1px solid var(--line);
	}

	.quick-open-dialog input {
		min-width: 0;
		border: 0;
		outline: 0;
		background: transparent;
		color: var(--text-primary);
		font-size: 13px;
	}

	.quick-open-heading {
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: 9px 12px 6px;
		color: var(--text-muted);
		font-size: 9px;
		text-transform: uppercase;
		letter-spacing: 0.07em;
	}

	.quick-open-results {
		max-height: 420px;
		padding: 4px 6px 8px;
		overflow-y: auto;
	}

	.quick-open-results button {
		display: grid;
		width: 100%;
		grid-template-columns: minmax(110px, 0.36fr) minmax(0, 1fr);
		align-items: center;
		gap: 12px;
		min-height: 36px;
		padding: 0 9px;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		font-size: 11px;
		text-align: left;
		cursor: pointer;
	}

	.quick-open-results button[aria-current="true"],
	.quick-open-results button:hover {
		background: var(--selection);
		color: var(--text-primary);
	}

	.quick-open-results small {
		overflow: hidden;
		color: var(--text-muted);
		font-size: 9px;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.quick-open-results p {
		margin: 20px 10px;
		color: var(--text-muted);
		font-size: 11px;
	}

	@media (prefers-reduced-motion: reduce) {
		.t-modal,
		.quick-open-backdrop {
			transition: none !important;
		}
	}

	@media (max-width: 799px) {
		.quick-open-trigger span,
		.quick-open-trigger kbd {
			display: none;
		}
	}
</style>
