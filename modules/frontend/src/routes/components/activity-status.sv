<script lang="ts" effect>
	import { Effect } from "effect";

	import type { LiveWorkspaceSnapshot } from "$lib/live-workspace/store";
	import { MakeActivityStatusView } from "$lib/live-workspace/activity-status";
	import { Badge } from "$lib/components/ui/badge";
	import { Card, CardContent } from "$lib/components/ui/card";

	let { snapshot }: { snapshot: LiveWorkspaceSnapshot } = $props();
	let phrase_index = $state(0);
	let reduced_motion = $state(false);

	if (typeof window !== "undefined" && typeof window.matchMedia === "function") {
		const motion_query = window.matchMedia("(prefers-reduced-motion: reduce)");
		const UpdateMotionPreference = (event: MediaQueryListEvent) => {
			reduced_motion = event.matches;
		};
		reduced_motion = motion_query.matches;
		motion_query.addEventListener("change", UpdateMotionPreference);
		yield* Effect.addFinalizer(
			Effect.sync(() => motion_query.removeEventListener("change", UpdateMotionPreference)),
		);
	}

	yield* Effect.forever(
		Effect.sleep("2400 millis").pipe(
			Effect.andThen(
				Effect.sync(() => {
					if (!reduced_motion) phrase_index += 1;
				}),
			),
		),
	).pipe(Effect.forkScoped);

	const activity = $derived(MakeActivityStatusView(snapshot, phrase_index, reduced_motion));
</script>

{#if activity.active}
	<Card class="mx-auto w-full max-w-3xl border-dashed shadow-none" aria-live="polite">
		<CardContent class="flex items-center gap-2 px-3 py-2">
			<span class="artisan-working-sprite" role="img" aria-label="Artisan is working"></span>
			<span class="text-sm font-medium">{activity.label}</span>
			<Badge class="ml-auto" variant="secondary">{activity.mode}</Badge>
		</CardContent>
	</Card>
{/if}

<style>
	.artisan-working-sprite {
		width: 2rem;
		height: 2rem;
		flex: none;
		background-image: url("/activity/artisan-working-sprite.png");
		background-position: 0 0;
		background-size: 200% 200%;
		image-rendering: pixelated;
		animation: artisan-working-sprite 1.6s steps(1, end) infinite;
	}

	@keyframes artisan-working-sprite {
		0%, 100% { background-position: 0 0; }
		25% { background-position: 100% 0; }
		50% { background-position: 0 100%; }
		75% { background-position: 100% 100%; }
	}

	@media (prefers-reduced-motion: reduce) {
		.artisan-working-sprite {
			animation: none;
			background-position: 0 0;
		}
	}
</style>
