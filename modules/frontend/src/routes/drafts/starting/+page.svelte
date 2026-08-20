<script lang="ts">
	/**
	 * Draft gallery for the starting surface — the screen you land on with no
	 * thread open, which today has no way to choose a project at all.
	 *
	 * Two buttons, because the point of the two buttons is comparison: a layout
	 * decision is never made by looking at one layout, it is made by flicking
	 * between two and noticing which one you keep wanting to go back to. `←` and
	 * `→` do the same thing, `[` and `]` skip to the next argument rather than
	 * the next draft, and `h` hides this chrome so a variant can be looked at, or
	 * screenshotted, without a pill sitting on top of it.
	 *
	 * Nothing here touches Forge. These are drawings with mock fixtures behind
	 * them, and they are deliberately above the connection gate so they can be
	 * reviewed with nothing running.
	 */
	import { dev } from "$app/environment";
	import ChevronLeft from "@tabler/icons-svelte/icons/chevron-left";
	import ChevronRight from "@tabler/icons-svelte/icons/chevron-right";
	import { StartingVariants } from "./registry";

	import "./drafts.css";

	let index = $state(0);
	let chrome_visible = $state(true);
	const total = StartingVariants.length;
	const variant = $derived(StartingVariants.at(index));
	const Variant = $derived(variant?.component);

	const Step = (delta: number) => {
		if (total === 0) return;
		index = (index + delta + total) % total;
	};

	/**
	 * The register is grouped, and stepping one draft at a time is the wrong
	 * granularity once a group is three deep: `[` and `]` land on the first draft
	 * of the previous or next argument, so the families can be compared without
	 * walking through every refinement inside them.
	 */
	const StepGroup = (delta: number) => {
		if (variant === undefined) return;
		const groups = [...new Set(StartingVariants.map((entry) => entry.group))];
		if (groups.length === 0) return;
		const at = groups.indexOf(variant.group);
		const next = groups[(at + delta + groups.length) % groups.length];
		index = StartingVariants.findIndex((entry) => entry.group === next);
	};

	/**
	 * A variant may own a text field — the launcher focuses one on arrival — and
	 * a gallery that stole its arrow keys would make that variant impossible to
	 * try. Typing always belongs to whatever is being typed into.
	 */
	const IsTyping = (target: EventTarget | null) =>
		target instanceof HTMLElement &&
		(target.isContentEditable ||
			target instanceof HTMLInputElement ||
			target instanceof HTMLTextAreaElement);

	const OnKeydown = (event: KeyboardEvent) => {
		if (event.metaKey || event.ctrlKey || event.altKey || IsTyping(event.target)) return;
		if (event.key === "ArrowRight") Step(1);
		else if (event.key === "ArrowLeft") Step(-1);
		else if (event.key === "]") StepGroup(1);
		else if (event.key === "[") StepGroup(-1);
		else if (event.key === "h") chrome_visible = !chrome_visible;
		else return;
		event.preventDefault();
	};
</script>

<svelte:head><title>Starting surface · drafts</title></svelte:head>

<!-- Top level because `<svelte:window>` has to be; the handler is inert without a gallery. -->
<svelte:window onkeydown={OnKeydown} />

{#if !dev}
	<div class="flex h-full items-center justify-center p-10">
		<p class="text-sm text-muted-foreground">
			This surface belongs to development tooling and is not part of this build.
		</p>
	</div>
{:else if variant !== undefined && Variant !== undefined}
	<!-- Above the Forge connection gate (z-50): a drawing needs no backend. -->
	<div class="fixed inset-0 z-[60] overflow-hidden bg-background text-foreground">
		{#key index}
			<div class="h-full w-full">
				<Variant />
			</div>
		{/key}
	</div>

	{#if chrome_visible}
		<div
			class="dz-swap fixed top-4 left-1/2 z-[70] flex w-max max-w-[min(46rem,calc(100vw-2rem))] -translate-x-1/2 flex-col items-center"
		>
			<div
				class="card-lg flex items-center gap-1 rounded-full border border-border bg-surface-900/95 p-1 backdrop-blur-md"
			>
				<button
					aria-label="Previous variant"
					class="dz-press dz-focus grid size-8 place-items-center rounded-full text-muted-foreground outline-none hover:bg-accent hover:text-foreground"
					onclick={() => Step(-1)}
					type="button"
				>
					<ChevronLeft class="size-4" />
				</button>

				<div class="flex min-w-[12rem] flex-col items-center px-2 py-0.5">
					<span class="text-[10px] leading-none text-surface-600">{variant.group}</span>
					<span class="mt-1 flex items-baseline gap-2 leading-none">
						<span class="text-sm text-foreground-extra">{variant.name}</span>
						<span class="font-mono text-[11px] tabular-nums text-muted-foreground">
							{index + 1}/{total}
						</span>
					</span>
				</div>

				<button
					aria-label="Next variant"
					class="dz-press dz-focus grid size-8 place-items-center rounded-full text-muted-foreground outline-none hover:bg-accent hover:text-foreground"
					onclick={() => Step(1)}
					type="button"
				>
					<ChevronRight class="size-4" />
				</button>
			</div>

			{#key index}
				<p
					class="dz-swap mt-2 max-w-[34rem] px-3 text-center text-[11px] leading-relaxed text-balance text-muted-foreground"
				>
					{variant.thesis}
				</p>
			{/key}

			<p class="mt-1.5 font-mono text-[10px] text-surface-600">
				← → switch · [ ] by group · h hides this
			</p>
		</div>
	{/if}
{:else}
	<div
		class="fixed inset-0 z-[60] grid place-items-center bg-background p-10 text-muted-foreground"
	>
		No starting-surface variants are registered.
	</div>
{/if}
