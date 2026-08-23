<script lang="ts">
	import { dev } from "$app/environment";
	import artisan_street from "$lib/assets/barekey/artisan-street.png";
	import artisan_street_jaw_shaded from "$lib/assets/barekey/artisan-street-jaw-shaded.png";
	import logo_gradient from "$lib/assets/barekey/logo-gradient.svg";

	const treatments = [
		{
			id: "card",
			class_name: "card",
			label: "Card",
			image: artisan_street,
			keyed_gradient: false,
			keyed_shadow_tint: false,
		},
		{
			id: "card-plastic",
			class_name: "card-plastic",
			label: "Plastic card",
			image: artisan_street,
			keyed_gradient: false,
			keyed_shadow_tint: false,
		},
		{
			id: "card-plastic-jaw-shaded",
			class_name: "card-plastic",
			label: "Plastic + jaw shading",
			image: artisan_street_jaw_shaded,
			keyed_gradient: false,
			keyed_shadow_tint: false,
		},
		{
			id: "card-jaw-shaded",
			class_name: "card",
			label: "Jaw shading",
			image: artisan_street_jaw_shaded,
			keyed_gradient: false,
			keyed_shadow_tint: false,
		},
		{
			id: "foreground-plastic-gradient-symbol",
			class_name: "card-plastic app-icon-foreground",
			label: "Foreground plastic + gradient symbol",
			image: artisan_street_jaw_shaded,
			keyed_gradient: true,
			keyed_shadow_tint: true,
		},
		{
			id: "foreground-muted-plastic-gradient-symbol",
			class_name:
				"card-plastic app-icon-foreground bg-linear-to-t from-foreground to-muted-foreground",
			label: "Foreground–muted plastic + gradient symbol",
			image: artisan_street_jaw_shaded,
			keyed_gradient: true,
			keyed_shadow_tint: true,
		},
		{
			id: "foreground-plastic-gradient-symbol-original-jaw",
			class_name: "card-plastic app-icon-foreground",
			label: "Foreground plastic + original jaw",
			image: artisan_street,
			keyed_gradient: true,
			keyed_shadow_tint: false,
		},
	] as const;
	const icon_background = "#505050";
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
		<div class="my-auto grid w-full max-w-6xl grid-cols-1 gap-14 sm:grid-cols-3 sm:gap-20">
			{#each treatments as treatment (treatment.id)}
				<figure class="flex flex-col items-center gap-5">
					<div
						class={`app-icon ${treatment.class_name}`}
						style:background-color={treatment.keyed_gradient
							? "var(--foreground)"
							: icon_background}
						style:background-image={treatment.keyed_gradient
							? undefined
							: `url(${logo_gradient})`}
					>
						{#if treatment.keyed_gradient}
							<div
								aria-hidden="true"
								class="app-icon-art app-icon-art-keyed"
								style:--app-icon-mask={`url(${treatment.image})`}
								style:background-image={`url(${logo_gradient})`}
							></div>
							{#if treatment.keyed_shadow_tint}
								<div
									aria-hidden="true"
									class="app-icon-art app-icon-art-keyed app-icon-art-keyed-shadow-tint"
									style:--app-icon-mask={`url(${treatment.image})`}
									style:background-image={`url(${logo_gradient})`}
								></div>
							{/if}
						{:else}
							<img
								aria-hidden="true"
								alt=""
								class="app-icon-art"
								src={treatment.image}
							/>
						{/if}
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
		position: relative;
		border-radius: 22.5%;
		overflow: hidden;
		isolation: isolate;
		background-position: center;
		background-size: cover;
		corner-shape: squircle;
	}

	.app-icon-art {
		display: block;
		width: 100%;
		height: 100%;
		object-fit: cover;
		border-radius: inherit;
		filter: drop-shadow(0 1px 1px rgb(0 0 0 / 12%));
		position: relative;
		z-index: 0;
	}

	.app-icon-art-keyed {
		position: absolute;
		inset: 0;
		background-position: center;
		background-repeat: no-repeat;
		background-size: cover;
		mask-image: var(--app-icon-mask);
		mask-position: center;
		mask-repeat: no-repeat;
		mask-size: cover;
		-webkit-mask-image: var(--app-icon-mask);
		-webkit-mask-position: center;
		-webkit-mask-repeat: no-repeat;
		-webkit-mask-size: cover;
	}

	.app-icon-art-keyed-shadow-tint {
		opacity: 0.45;
	}

</style>
