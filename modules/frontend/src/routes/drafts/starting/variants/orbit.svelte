<script lang="ts">
	/**
	 * Recency, made spatial.
	 *
	 * The premise: a list sorted by last-used encodes the ordering but throws
	 * away the *distance* — third and seventh look the same, one row apart. An
	 * arc keeps it: what you touched an hour ago sits above the field, and what
	 * you have not opened in a month is small and out at the edge.
	 *
	 * Choosing re-centres the arc rather than replacing it. Every project keeps
	 * its element and only its transform changes, so a fast run of clicks
	 * retargets mid-flight instead of restarting.
	 */
	import { DraftProjects, ThreadsFor } from "../mock";
	import Composer from "../pieces/composer.svelte";
	import Monogram from "../pieces/monogram.svelte";

	const total = DraftProjects.length;
	let active_index = $state(0);
	const active = $derived(DraftProjects[active_index] ?? DraftProjects[0]);

	/**
	 * Rank 0 sits at the top of the arc; the rest alternate outward, so the arc
	 * stays balanced no matter which project is centred.
	 */
	const slot_of = (rank: number) =>
		rank === 0 ? 0 : rank % 2 === 1 ? Math.ceil(rank / 2) : -Math.ceil(rank / 2);

	const max_slot = Math.ceil((total - 1) / 2);

	const placement = (index: number) => {
		const rank = (index - active_index + total) % total;
		const slot = slot_of(rank);
		const fraction = max_slot === 0 ? 0 : slot / max_slot;
		const angle = fraction * 1.15;
		const radius = 190 + Math.abs(fraction) * 40;
		return {
			distance: Math.abs(fraction),
			rank,
			x: Math.sin(angle) * radius * 1.85,
			y: -Math.cos(angle) * radius,
		};
	};
</script>

<div class="dz-vignette relative grid h-full place-items-center overflow-hidden p-8">
	<!-- The arc reaches ~230px above its anchor; the padding is the room it needs. -->
	<div class="relative w-full max-w-[40rem] pt-[15rem]">
		<div
			aria-label="Projects by recency"
			class="pointer-events-none absolute inset-x-0 top-[15rem]"
			role="group"
		>
			{#each DraftProjects as project, index (project.project_id)}
				{@const spot = placement(index)}
				<button
					class="dz-press dz-focus pointer-events-auto absolute top-0 left-1/2 flex flex-col items-center gap-2 rounded-2xl p-2 outline-none transition-[transform,opacity] duration-(--panel-open-dur) ease-(--ease-smooth-out) motion-reduce:transition-none"
					onclick={() => (active_index = index)}
					style:opacity={1 - spot.distance * 0.55}
					style:transform={`translate(calc(-50% + ${spot.x}px), ${spot.y}px) scale(${1 - spot.distance * 0.28})`}
					style:z-index={total - spot.rank}
					type="button"
				>
					<Monogram
						class={`size-12 rounded-2xl text-xs ${spot.rank === 0 ? "" : "grayscale-[35%]"}`}
						{project}
					/>
					<span class="max-w-28 truncate text-[11px] whitespace-nowrap text-muted-foreground">
						{project.name}
					</span>
					{#if spot.rank === 0}
						<span class="text-[10px] text-surface-500">{project.last_used}</span>
					{/if}
				</button>
			{/each}
		</div>

		<div class="relative">
			<p class="dz-enter mb-3 text-center text-sm text-muted-foreground" style:--dz-delay="200ms">
				{ThreadsFor(active.project_id).length} open in
				<span class="text-foreground-extra">{active.name}</span>
				<span class="text-surface-600">· {active.branch}</span>
			</p>
			<div class="dz-enter" style:--dz-delay="240ms">
				<Composer placeholder={`What are we doing in ${active.name}?`} tall />
			</div>
		</div>
	</div>
</div>
