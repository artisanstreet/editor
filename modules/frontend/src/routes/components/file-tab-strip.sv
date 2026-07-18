<script lang="ts" effect>
	import { Effect, Option } from "effect";
	import {
		IconGitCompare as GitCompare,
		IconPin as Pin,
		IconPinned as Pinned,
		IconRobot as Robot,
		IconX as X,
	} from "@tabler/icons-svelte";

	import type {
		CloseTabOutcome,
		DirtyCloseConfirmation,
		WorkspaceTab,
	} from "$lib/workspace/workspace-tab-model";
	import { Button } from "$lib/components/ui/button";
	import {
		AlertDialog,
		AlertDialogAction,
		AlertDialogCancel,
		AlertDialogContent,
		AlertDialogDescription,
		AlertDialogFooter,
		AlertDialogHeader,
		AlertDialogTitle,
	} from "$lib/components/ui/alert-dialog";

	let {
		visible_tabs,
		overflow_tabs,
		active_tab_id,
		on_activate,
		on_pin,
		on_promote,
		on_close,
		on_confirm_close,
	}: {
		visible_tabs: ReadonlyArray<WorkspaceTab>;
		overflow_tabs: ReadonlyArray<WorkspaceTab>;
		active_tab_id: string | undefined;
		on_activate: (tab_id: string) => Effect.Effect<void>;
		on_pin: (tab_id: string) => Effect.Effect<void>;
		on_promote: (tab_id: string) => Effect.Effect<void>;
		on_close: (tab_id: string) => Effect.Effect<CloseTabOutcome>;
		on_confirm_close: (
			confirmation: DirtyCloseConfirmation,
		) => Effect.Effect<CloseTabOutcome>;
	} = $props();

	let pending_confirmation = $state.raw<DirtyCloseConfirmation>();
	let pending_file_name = $state("");

	const HandleCloseOutcome = (outcome: CloseTabOutcome) =>
		Effect.gen(function* () {
			yield* Effect.void;

			if (outcome._tag === "ConfirmationRequired") {
				pending_confirmation = outcome.confirmation;
				pending_file_name = outcome.tab.file.name;
			} else if (outcome._tag === "Closed" || outcome._tag === "TabNotFound") {
				pending_confirmation = undefined;
				pending_file_name = "";
			}
		});

	const RequestClose = (tab_id: string) =>
		Effect.gen(function* () {
			const outcome = yield* on_close(tab_id);
			yield* HandleCloseOutcome(outcome);
		});

	const ConfirmClose = Effect.gen(function* () {
		if (pending_confirmation === undefined) {
			return;
		}

		const exact_confirmation = pending_confirmation;
		const outcome = yield* on_confirm_close(exact_confirmation);
		if (outcome._tag === "ConfirmationStale") {
			const refreshed = yield* on_close(exact_confirmation.tab_id);
			yield* HandleCloseOutcome(refreshed);
			return;
		}

		yield* HandleCloseOutcome(outcome);
	});

	const DismissClose = () => {
		pending_confirmation = undefined;
		pending_file_name = "";
	};

	const CancelClose = Effect.gen(function* () {
		DismissClose();
	});

	const HandleDialogOpenChange = (open: boolean) => {
		if (!open) {
			DismissClose();
		}
	};
</script>

<div class="file-tab-strip" role="group" aria-label="Open files">
	<span class="ownership-label">Your files</span>
	<div class="file-tabs-scroll" role="group" aria-label="Editor file tabs">
		{#each visible_tabs as tab}
			{const agent_change = Option.getOrUndefined(tab.agent_change)}
			<div id={`workspace-tab-${tab.generation}`} class:active={active_tab_id === tab.id} class="file-tab" data-ownership={tab.ownership._tag} data-content={tab.content._tag} data-edit={tab.edit_state._tag}>
				<Button variant="ghost" class="tab-activate" aria-pressed={active_tab_id === tab.id} ondblclick={yield* on_promote(tab.id)} onclick={yield* on_activate(tab.id)}>
					<span class="tab-name">{tab.file.name}</span>
					<span class="tab-states" aria-label={`States for ${tab.file.name}`}>
						{#if tab.content._tag === "DiffPreview"}<span class="state-chip diff"><GitCompare size={11} stroke={1.8} aria-hidden="true" />Diff preview</span>{/if}
						{#if tab.ownership._tag === "Preview"}<span class="state-chip">Preview</span>{/if}
						{#if tab.ownership._tag === "Pinned"}<span class="state-chip"><Pinned size={10} stroke={1.8} aria-hidden="true" />Pinned</span>{/if}
						{#if tab.edit_state._tag === "Dirty"}<span class="state-chip dirty">Dirty</span>{/if}
						{#if agent_change !== undefined}<span class="state-chip agent"><Robot size={10} stroke={1.8} aria-hidden="true" />Agent change +{agent_change.added} −{agent_change.removed}</span>{/if}
					</span>
				</Button>
				<Button variant="ghost" size="icon-xs" class="tab-action" disabled={tab.ownership._tag === "Pinned"} aria-label={tab.ownership._tag === "Pinned" ? `${tab.file.name} is pinned` : `Pin ${tab.file.name}`} onclick={yield* on_pin(tab.id)}>
					<Pin size={12} stroke={1.8} aria-hidden="true" />
				</Button>
				<Button variant="ghost" size="icon-xs" class="tab-action close" aria-label={`Close ${tab.file.name}`} onclick={yield* RequestClose(tab.id)}>
					<X size={13} stroke={1.8} aria-hidden="true" />
				</Button>
			</div>
		{/each}
	</div>
	{#if overflow_tabs.length > 0}<span class="overflow-count" aria-label={`${overflow_tabs.length} tabs in overflow`}>+{overflow_tabs.length}</span>{/if}
</div>

<AlertDialog open={pending_confirmation !== undefined} onOpenChange={HandleDialogOpenChange}>
	<AlertDialogContent>
		<AlertDialogHeader>
			<AlertDialogTitle>Discard unsaved edits?</AlertDialogTitle>
			<AlertDialogDescription>
				<strong>{pending_file_name}</strong> has unsaved edits. This local change will be discarded.
			</AlertDialogDescription>
		</AlertDialogHeader>
		<AlertDialogFooter>
			<AlertDialogCancel onclick={yield* CancelClose}>Keep open</AlertDialogCancel>
			<AlertDialogAction onclick={yield* ConfirmClose}>Discard and close</AlertDialogAction>
		</AlertDialogFooter>
	</AlertDialogContent>
</AlertDialog>

<style>
	.file-tab-strip {
		display: flex;
		align-items: stretch;
		min-width: 0;
		height: 45px;
		border-bottom: 1px solid var(--line);
		background: var(--pane-inset);
	}

	.ownership-label,
	.overflow-count {
		display: grid;
		flex: 0 0 auto;
		place-items: center;
		padding: 0 9px;
		border-right: 1px solid var(--line);
		color: var(--text-muted);
		font-size: 8px;
		font-weight: 650;
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.overflow-count {
		border-right: 0;
		border-left: 1px solid var(--line);
	}

	.file-tabs-scroll {
		display: flex;
		min-width: 0;
		flex: 1;
		overflow-x: auto;
		overflow-y: hidden;
		scrollbar-width: none;
	}

	.file-tab {
		display: grid;
		min-width: 176px;
		max-width: 240px;
		grid-template-columns: minmax(0, 1fr) 24px 24px;
		border-right: 1px solid var(--line);
		background: transparent;
		color: var(--text-muted);
	}

	.file-tab:hover,
	.file-tab.active {
		background: var(--pane);
		color: var(--text-primary);
	}

	.file-tab.active {
		box-shadow: inset 0 -2px var(--focus);
	}

	:global(.tab-activate),
	:global(.tab-action) {
		min-width: 0;
		padding: 0;
		border: 0;
		background: transparent;
		color: inherit;
		cursor: pointer;
	}

	:global(.tab-activate) {
		display: grid;
		align-content: center;
		gap: 3px;
		padding-left: 9px;
		text-align: left;
	}

	.tab-name {
		overflow: hidden;
		font-size: 10px;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.tab-states {
		display: flex;
		min-width: 0;
		align-items: center;
		gap: 3px;
		overflow: hidden;
	}

	.state-chip {
		display: inline-flex;
		flex: 0 0 auto;
		align-items: center;
		gap: 2px;
		color: var(--text-muted);
		font-size: 7px;
		font-weight: 650;
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.state-chip + .state-chip::before {
		content: "·";
		margin-right: 1px;
		color: var(--line-strong);
	}

	.state-chip.dirty::after {
		content: "";
		width: 5px;
		height: 5px;
		border: 1px solid currentColor;
		border-radius: 50%;
	}

	.state-chip.diff {
		color: var(--run-active);
	}

	.state-chip.agent {
		color: var(--permission);
	}

	:global(.tab-action) {
		display: grid;
		place-items: center;
		align-self: stretch;
		opacity: 0.42;
	}

	:global(.tab-action:hover),
	:global(.tab-action:focus-visible) {
		background: var(--raised-hover);
		opacity: 1;
	}

	:global(.tab-action:disabled) {
		cursor: default;
		opacity: 0.18;
	}

	:global(.tab-action.close:hover) {
		color: var(--run-failed);
	}

	:global(.tab-activate:focus-visible),
	:global(.tab-action:focus-visible) {
		outline: 2px solid var(--focus);
		outline-offset: -2px;
	}
</style>
