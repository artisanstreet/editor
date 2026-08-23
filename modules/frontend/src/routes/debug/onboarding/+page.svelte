<script lang="ts">
	import { dev } from "$app/environment";
	import OnboardingView from "../../components/onboarding/view.svelte";
</script>

<svelte:head><title>Set up Artisan</title></svelte:head>

{#if dev}
	<OnboardingView />
{:else}
	<div class="flex h-full items-center justify-center p-10">
		<p class="text-sm text-muted-foreground">
			This surface belongs to development tooling and is not part of this build.
		</p>
	</div>
{/if}
