<script lang="ts" effect>
	import { Effect } from "effect";
	import { IconLayoutSidebar as PanelLeft, IconLayoutSidebarRight as PanelRight } from "@tabler/icons-svelte";
	import {
		DefaultShellPresentationState,
		ShellPresentationPreferences,
	} from "$lib/runtime/shell-presentation-preferences";

	import LeftPane from "./left-pane.sv";
	import MainPane from "./main-pane.sv";
	import RightPane from "./right-pane.sv";

	type EdgePane = "left" | "right";

	const shell_presentation_preferences = yield* ShellPresentationPreferences;
	const initial_presentation = yield* shell_presentation_preferences.Load;

	let left_open = $state(false);
	let right_open = $state(false);
	let left_collapsed = $state(initial_presentation.left_collapsed);
	let right_collapsed = $state(initial_presentation.right_collapsed);
	let selected_thread = $state("thread-editor");
	let draft_threads = $state(0);

	const SavePresentation = Effect.gen(function* () {
		yield* shell_presentation_preferences.Save({
			...DefaultShellPresentationState,
			left_collapsed,
			right_collapsed,
		});
	});

	const OpenPane = (pane: EdgePane) =>
		Effect.gen(function* () {
			left_open = pane === "left";
			right_open = pane === "right";
		});

	const ExpandLeft = Effect.gen(function* () {
		left_collapsed = false;
		yield* SavePresentation;
	});

	const ExpandRight = Effect.gen(function* () {
		right_collapsed = false;
		yield* SavePresentation;
	});

	const CollapseLeft = Effect.gen(function* () {
		left_collapsed = true;
		left_open = false;
		yield* SavePresentation;
	});

	const CollapseRight = Effect.gen(function* () {
		right_collapsed = true;
		right_open = false;
		yield* SavePresentation;
	});

	const ClosePanes = Effect.gen(function* () {
		left_open = false;
		right_open = false;
	});

	const SelectThread = (thread_id: string) =>
		Effect.gen(function* () {
			selected_thread = thread_id;
		});

	const NewChat = Effect.gen(function* () {
		draft_threads += 1;
	});

	const HandleKeydown = (pressed_key: string) =>
		Effect.gen(function* () {
			if (pressed_key === "Escape") {
				yield* ClosePanes;
			}
		});
</script>

<svelte:window onkeydown={yield* HandleKeydown(event.key)} />

<div
	class="editor-shell"
	data-left-collapsed={left_collapsed}
	data-left-open={left_open}
	data-right-collapsed={right_collapsed}
	data-right-open={right_open}
>
	<div class="pane-slot desktop-left-slot">
		<LeftPane compact={false} instance_id="desktop-left" {selected_thread} {draft_threads} on_select_thread={SelectThread} on_new_chat={NewChat} on_collapse={CollapseLeft} />
	</div>

	<div class="left-rail-slot">
		<LeftPane compact={true} instance_id="rail-left" {selected_thread} {draft_threads} on_select_thread={SelectThread} on_new_chat={NewChat} />
	</div>

	<main class="main-slot">
		<div class="compact-pane-actions" aria-label="Open workspace panes">
			<button class="pane-toggle desktop-left-toggle" type="button" aria-label="Expand thread navigation" onclick={yield* ExpandLeft}>
				<PanelLeft size={18} stroke={1.7} aria-hidden="true" />
			</button>
			<button class="pane-toggle desktop-right-toggle" type="button" aria-label="Expand session pane" onclick={yield* ExpandRight}>
				<PanelRight size={18} stroke={1.7} aria-hidden="true" />
			</button>
			<button class="pane-toggle responsive-left-toggle" type="button" aria-label="Open thread navigation" onclick={yield* OpenPane("left")}>
				<PanelLeft size={18} stroke={1.7} aria-hidden="true" />
			</button>
			<button class="pane-toggle responsive-right-toggle" type="button" aria-label="Open session pane" onclick={yield* OpenPane("right")}>
				<PanelRight size={18} stroke={1.7} aria-hidden="true" />
			</button>
		</div>
		<MainPane />
	</main>

	<div class="pane-slot desktop-right-slot">
		<RightPane instance_id="desktop-right" on_collapse={CollapseRight} />
	</div>

	<div class="pane-slot left-overlay t-panel-slide" data-open={left_open} aria-hidden={!left_open} inert={!left_open}>
		<LeftPane compact={false} instance_id="overlay-left" {selected_thread} {draft_threads} on_select_thread={SelectThread} on_new_chat={NewChat} />
	</div>

	<div class="pane-slot right-overlay t-panel-slide" data-open={right_open} aria-hidden={!right_open} inert={!right_open}>
		<RightPane instance_id="overlay-right" />
	</div>

	{#if left_open || right_open}
		<button class="pane-backdrop" type="button" aria-label="Close open pane" onclick={yield* ClosePanes}></button>
	{/if}
</div>

<style>
	.editor-shell {
		--pane-action-space: 10px;

		display: grid;
		grid-template-columns: 272px minmax(720px, 1fr) 340px;
		gap: 12px;
		height: 100dvh;
		min-height: 0;
		padding: 8px;
		background: var(--canvas);
		color: var(--text-primary);
		overflow: hidden;
	}

	.pane-slot,
	.main-slot,
	.left-rail-slot {
		min-width: 0;
		min-height: 0;
	}

	.pane-slot,
	.main-slot {
		position: relative;
		z-index: 1;
	}

	.main-slot {
		overflow: hidden;
	}

	.left-rail-slot,
	.compact-pane-actions,
	.desktop-left-toggle,
	.desktop-right-toggle,
	.responsive-left-toggle,
	.responsive-right-toggle,
	.pane-backdrop {
		display: none;
	}

	.pane-toggle {
		display: grid;
		width: 32px;
		height: 32px;
		place-items: center;
		border: 1px solid var(--line);
		border-radius: var(--radius-sm);
		background: var(--raised);
		color: var(--text-secondary);
		cursor: pointer;
	}

	.editor-shell[data-left-collapsed="true"] {
		grid-template-columns: 56px minmax(720px, 1fr) 340px;
		--pane-action-space: 48px;
	}

	.editor-shell[data-right-collapsed="true"] {
		grid-template-columns: 272px minmax(720px, 1fr);
		--pane-action-space: 48px;
	}

	.editor-shell[data-left-collapsed="true"][data-right-collapsed="true"] {
		grid-template-columns: 56px minmax(720px, 1fr);
		--pane-action-space: 82px;
	}

	.editor-shell[data-left-collapsed="true"] .desktop-left-slot,
	.editor-shell[data-right-collapsed="true"] .desktop-right-slot {
		display: none;
	}

	.editor-shell[data-left-collapsed="true"] .left-rail-slot,
	.editor-shell[data-left-collapsed="true"] .compact-pane-actions,
	.editor-shell[data-left-collapsed="true"] .desktop-left-toggle,
	.editor-shell[data-right-collapsed="true"] .compact-pane-actions,
	.editor-shell[data-right-collapsed="true"] .desktop-right-toggle {
		display: grid;
	}

	.editor-shell[data-left-collapsed="true"] .compact-pane-actions,
	.editor-shell[data-right-collapsed="true"] .compact-pane-actions {
		position: absolute;
		top: 10px;
		right: 10px;
		z-index: 10;
		display: flex;
		gap: 6px;
	}

	.pane-toggle:hover {
		color: var(--text-primary);
		border-color: var(--line-strong);
	}

	.pane-toggle:focus-visible,
	.pane-backdrop:focus-visible {
		outline: 2px solid var(--focus);
		outline-offset: 2px;
	}

	@media (min-width: 1280px) and (max-width: 1367px) {
		.editor-shell {
			grid-template-columns: 240px minmax(720px, 1fr) 280px;
		}

		.editor-shell[data-left-collapsed="true"] {
			grid-template-columns: 56px minmax(720px, 1fr) 280px;
		}

		.editor-shell[data-right-collapsed="true"] {
			grid-template-columns: 240px minmax(720px, 1fr);
		}

		.editor-shell[data-left-collapsed="true"][data-right-collapsed="true"] {
			grid-template-columns: 56px minmax(720px, 1fr);
		}
	}

	@media (max-width: 1279px) {
		.editor-shell {
			--pane-action-space: 48px;

			grid-template-columns: 240px minmax(0, 1fr);
		}

		.editor-shell[data-left-collapsed="true"],
		.editor-shell[data-right-collapsed="true"],
		.editor-shell[data-left-collapsed="true"][data-right-collapsed="true"] {
			grid-template-columns: 240px minmax(0, 1fr);
		}

		.editor-shell .desktop-right-slot {
			display: none;
		}

		.editor-shell[data-left-collapsed="true"] .desktop-left-slot {
			display: block;
		}

		.editor-shell[data-left-collapsed="true"] .left-rail-slot,
		.editor-shell[data-left-collapsed="true"] .desktop-left-toggle,
		.editor-shell[data-right-collapsed="true"] .desktop-right-toggle {
			display: none;
		}

		.right-overlay {
			position: fixed;
			top: 8px;
			right: 8px;
			bottom: 8px;
			z-index: 30;
			width: min(340px, calc(100vw - 24px));
			transform: translateX(calc(100% + 12px));
		}

		.right-overlay.t-panel-slide[data-open="true"] {
			transform: translateX(0);
		}

		.responsive-right-toggle,
		.pane-backdrop {
			display: grid;
		}

		.compact-pane-actions {
			position: absolute;
			top: 10px;
			right: 10px;
			z-index: 10;
			display: flex;
		}

		.pane-backdrop {
			position: fixed;
			inset: 0;
			z-index: 20;
			border: 0;
			background: color-mix(in srgb, var(--canvas) 68%, transparent);
			cursor: default;
		}
	}

	@media (min-width: 800px) and (max-width: 999px) {
		.editor-shell,
		.editor-shell[data-left-collapsed="true"],
		.editor-shell[data-right-collapsed="true"],
		.editor-shell[data-left-collapsed="true"][data-right-collapsed="true"] {
			grid-template-columns: 56px minmax(0, 1fr);
		}

		.editor-shell .desktop-left-slot,
		.editor-shell[data-left-collapsed="true"] .desktop-left-slot {
			display: none;
		}

		.editor-shell .left-rail-slot,
		.editor-shell[data-left-collapsed="true"] .left-rail-slot {
			display: block;
		}
	}

	@media (max-width: 799px) {
		.editor-shell,
		.editor-shell[data-left-collapsed="true"],
		.editor-shell[data-right-collapsed="true"],
		.editor-shell[data-left-collapsed="true"][data-right-collapsed="true"] {
			--pane-action-space: 82px;

			grid-template-columns: minmax(0, 1fr);
			gap: 0;
			padding: 6px;
		}

		.editor-shell .desktop-left-slot,
		.editor-shell[data-left-collapsed="true"] .desktop-left-slot,
		.editor-shell .left-rail-slot,
		.editor-shell[data-left-collapsed="true"] .left-rail-slot {
			display: none;
		}

		.left-overlay {
			position: fixed;
			top: 6px;
			bottom: 6px;
			left: 6px;
			z-index: 30;
			width: min(288px, calc(100vw - 24px));
			transform: translateX(calc(-100% - 12px));
		}

		.left-overlay.t-panel-slide[data-open="true"] {
			transform: translateX(0);
		}

		.responsive-left-toggle,
		.responsive-right-toggle,
		.pane-backdrop {
			display: grid;
		}

		.compact-pane-actions {
			position: absolute;
			top: 8px;
			right: 8px;
			z-index: 10;
			display: flex;
			gap: 6px;
		}
	}

	.t-panel-slide {
		display: none;
		opacity: 1;
		filter: blur(0);
		transition:
			transform var(--panel-close-dur) var(--panel-ease),
			opacity var(--panel-close-dur) var(--panel-ease),
			filter var(--panel-close-dur) var(--panel-ease);
		will-change: transform, opacity, filter;
	}

	@media (max-width: 1279px) {
		.right-overlay {
			display: block;
		}

		.t-panel-slide {
			opacity: 0;
			filter: blur(var(--panel-blur));
			pointer-events: none;
		}

		.t-panel-slide[data-open="true"] {
			opacity: 1;
			filter: blur(0);
			pointer-events: auto;
			transition:
				transform var(--panel-open-dur) var(--panel-ease),
				opacity var(--panel-open-dur) var(--panel-ease),
				filter var(--panel-open-dur) var(--panel-ease);
		}
	}

	@media (max-width: 799px) {
		.left-overlay {
			display: block;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.t-panel-slide {
			transition: none !important;
		}
	}
</style>
