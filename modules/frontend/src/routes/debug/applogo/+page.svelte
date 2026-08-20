<script lang="ts">
	import { dev } from "$app/environment";
	import artisan_star from "$lib/assets/barekey/artisan-star.svg";
	import logo_gradient from "$lib/assets/barekey/logo-gradient.svg";

	const treatments = [
		{ class_name: "card", label: "Card" },
		{ class_name: "card-plastic", label: "Plastic card" },
	] as const;

</script>

<svelte:head><title>App logo lab</title></svelte:head>

{#if !dev}
	<div class="flex h-full items-center justify-center p-10">
		<p class="text-sm text-muted-foreground">
			This surface belongs to development tooling and is not part of this build.
		</p>
	</div>
{:else}
	<!-- Above the Forge connection gate: the logo comparison has no Forge dependency. -->
	<main
		class="fixed inset-0 z-[60] flex justify-center overflow-auto bg-[#171717] px-8 py-16"
	>
		<div class="my-auto grid w-full max-w-2xl grid-cols-1 gap-14 sm:grid-cols-2 sm:gap-20">
			{#each treatments as treatment (treatment.class_name)}
				<figure class="flex flex-col items-center gap-5">
					<div
						class={`app-icon ${treatment.class_name}`}
						style:background-image={`url(${logo_gradient})`}
					>
						<img aria-hidden="true" alt="" class="app-icon-mark" src={artisan_star} />
					</div>
					<figcaption class="font-mono text-xs tracking-wide text-white/45">
						{treatment.label}
					</figcaption>
				</figure>
			{/each}
		</div>
	</main>
{/if}

<style>
	.app-icon {
		display: grid;
		width: min(14rem, 58vw);
		aspect-ratio: 1;
		place-items: center;
		border-radius: 22.5%;
		background-position: center;
		background-size: cover;
		corner-shape: squircle;
	}

	.app-icon-mark {
		width: 75%;
		aspect-ratio: 1;
		filter: drop-shadow(0 1px 1px rgb(0 0 0 / 12%));
	}

</style>
