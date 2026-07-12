<script lang="ts" effect>
	import { Effect } from "effect";
	import { IconFileCode as FileCode } from "@tabler/icons-svelte";

	import type { FileFixture } from "./editor-fixtures";

	let {
		files,
		active_file_id,
		on_select,
	}: {
		files: ReadonlyArray<FileFixture>;
		active_file_id: string;
		on_select: (file_index: number) => Effect.Effect<void>;
	} = $props();

	const SelectFile = (file_index: number) =>
		Effect.gen(function* () {
			yield* on_select(file_index);
		});
</script>

<div class="file-tab-strip" role="group" aria-label="Open files">
	<span class="ownership-label">Your files</span>
	<div class="file-tabs-scroll">
		{#each files as file, file_index}
			<button
				class:active={active_file_id === file.id}
				class="file-tab"
				type="button"
				aria-pressed={active_file_id === file.id}
				onclick={yield* SelectFile(file_index)}
			>
				<FileCode size={14} stroke={1.7} aria-hidden="true" />
				<span>{file.name}</span>
			</button>
		{/each}
	</div>
</div>

<style>
	.file-tab-strip {
		display: flex;
		align-items: stretch;
		min-width: 0;
		height: 38px;
		border-bottom: 1px solid var(--line);
		background: var(--pane-inset);
	}

	.ownership-label {
		display: grid;
		flex: 0 0 auto;
		place-items: center;
		padding: 0 10px;
		border-right: 1px solid var(--line);
		color: var(--text-muted);
		font-size: 9px;
		font-weight: 650;
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.file-tabs-scroll {
		display: flex;
		min-width: 0;
		overflow-x: auto;
		overflow-y: hidden;
		scrollbar-width: none;
	}

	.file-tab {
		display: flex;
		min-width: 142px;
		max-width: 210px;
		align-items: center;
		gap: 7px;
		padding: 0 10px;
		border: 0;
		border-right: 1px solid var(--line);
		background: transparent;
		color: var(--text-muted);
		font: inherit;
		font-size: 11px;
		cursor: pointer;
	}

	.file-tab span {
		overflow: hidden;
		flex: 1;
		text-align: left;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.file-tab:hover,
	.file-tab.active {
		background: var(--pane);
		color: var(--text-primary);
	}

	.file-tab.active {
		box-shadow: inset 0 -2px var(--focus);
	}

	.file-tab:focus-visible {
		outline: 2px solid var(--focus);
		outline-offset: -2px;
	}
</style>
