<script lang="ts" effect>
	import { Effect } from "effect";
	import { IconCode as CodeIcon, IconGitBranch as GitBranch, IconMessageCircle as MessageCircle } from "@tabler/icons-svelte";
	import { Button } from "$lib/components/ui/button";

	import type { EditorMode } from "./editor-fixtures";

	let { mode, on_select }: { mode: EditorMode; on_select: (mode: EditorMode) => Effect.Effect<void> } = $props();

	const SelectMode = (next_mode: EditorMode) =>
		Effect.gen(function* () {
			yield* on_select(next_mode);
		});
</script>

<div class="workspace-mode-switcher t-tabs" data-mode={mode} role="group" aria-label="Workspace mode">
	<span class="t-tabs-pill" aria-hidden="true"></span>
	<Button variant="ghost" size="icon-sm" class="t-tab" aria-label="Editor" aria-pressed={mode === "editor"} title="Editor" onclick={yield* SelectMode("editor")}>
		<CodeIcon size={16} stroke={1.7} aria-hidden="true" />
	</Button>
	<Button variant="ghost" size="icon-sm" class="t-tab" aria-label="Chat" aria-pressed={mode === "chat"} title="Chat" onclick={yield* SelectMode("chat")}>
		<MessageCircle size={16} stroke={1.7} aria-hidden="true" />
	</Button>
	<Button variant="ghost" size="icon-sm" class="t-tab" aria-label="Orchestrator" aria-pressed={mode === "orchestrator"} title="Orchestrator" onclick={yield* SelectMode("orchestrator")}>
		<GitBranch size={16} stroke={1.7} aria-hidden="true" />
	</Button>
</div>

<style>
	.t-tabs {
		position: relative;
		display: inline-grid;
		grid-template-columns: repeat(3, 32px);
		align-items: center;
		gap: 3px;
		padding: 3px;
		border: 1px solid var(--line);
		border-radius: var(--radius-md);
		background: var(--tabs-bar-bg);
	}

	:global(.t-tab) {
		position: relative;
		z-index: 1;
		display: grid;
		width: 32px;
		height: 28px;
		place-items: center;
		padding: 0;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--tabs-text-muted);
		cursor: pointer;
		transition: color var(--tabs-dur) var(--tabs-ease);
	}

	:global(.t-tab:hover),
	:global(.t-tab[aria-pressed="true"]) {
		color: var(--tabs-text-active);
	}

	:global(.t-tab:focus-visible) {
		outline: 2px solid var(--focus);
		outline-offset: -2px;
	}

	.t-tabs-pill {
		position: absolute;
		top: 3px;
		left: 3px;
		z-index: 0;
		width: 32px;
		height: 28px;
		border: 1px solid var(--line-strong);
		border-radius: var(--radius-sm);
		background: var(--tabs-pill-bg);
		box-shadow: var(--shadow-xs);
		transform: translateX(0);
		transition: transform var(--tabs-dur) var(--tabs-ease);
		will-change: transform;
		pointer-events: none;
	}

	.t-tabs[data-mode="chat"] .t-tabs-pill {
		transform: translateX(35px);
	}

	.t-tabs[data-mode="orchestrator"] .t-tabs-pill {
		transform: translateX(70px);
	}

	@media (prefers-reduced-motion: reduce) {
		.t-tabs-pill,
		:global(.t-tab) {
			transition: none !important;
		}
	}
</style>
