<script lang="ts" effect>
	import Terminal2 from "@tabler/icons-svelte/icons/terminal-2";
	import type { TerminalSession } from "@artisan/protocol";
	import type { Effect } from "effect";
	import { terminal_command_line, terminal_display_name } from "$lib/terminal/presentation";
	import HoverPill, { type PillHover } from "./hover-pill.svelte";

	let {
		entries,
		hover,
		oninspect,
	}: {
		readonly entries: ReadonlyArray<TerminalSession>;
		readonly hover: PillHover;
		readonly oninspect: (entry: TerminalSession) => Effect.Effect<void>;
	} = $props();
</script>

<section class="min-h-0" aria-labelledby="thread-terminals-heading">
	<h2 id="thread-terminals-heading" class="px-2 pt-2 pb-1 text-sm font-medium text-foreground">
		Terminals
	</h2>
	<div class="relative">
		<HoverPill {hover} />
		<ul class="@container flex min-w-0 flex-col" aria-label="Open terminals">
			{#each entries as entry (entry.terminal_id)}
			<li>
				<button
					type="button"
					class="relative flex w-full min-w-0 items-center justify-between gap-4 rounded-lg px-2 py-2 text-left text-sm outline-none"
					onclick={yield* oninspect(entry)}
					onpointerenter={hover.move}
					onpointermove={hover.move}
					onfocusin={hover.move}
				>
					<span class="flex min-w-0 flex-1 items-center gap-2">
						<Terminal2 class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
						<!-- Ellipsis, not a fade: a fading last letter reads as a rendering fault. -->
						<span class="min-w-0 truncate text-foreground">
							{terminal_display_name(entry)}
						</span>
					</span>
					<!--
						The command is the identity: "vp dev" says which terminal this is when
						three shells share one executable. It rides the same line as the name
						now instead of sitting under it, so it gives up its own width first and
						leaves outright once the row cannot carry both legibly — the name alone
						still points at a row you can open to see the rest.
					-->
					<span
						class="hidden max-w-[55%] shrink truncate font-mono text-xs text-muted-foreground @min-[14rem]:block"
					>
						{terminal_command_line(entry)}
					</span>
				</button>
			</li>
			{/each}
		</ul>
	</div>
</section>
