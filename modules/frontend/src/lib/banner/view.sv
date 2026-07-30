<script lang="ts">
	import PlayerPlay from "@tabler/icons-svelte/icons/player-play";
	import Refresh from "@tabler/icons-svelte/icons/refresh";

	import {
		is_banner_executable_action,
		type BannerEvent,
		type BannerExecutableAction,
	} from "./service";

	let {
		event,
		onaction,
	}: {
		event: BannerEvent;
		onaction: (action: BannerExecutableAction) => void;
	} = $props();

	const tone_class = {
		error: "banner-error card-error",
		info: "banner-info card-info",
		success: "banner-success card-success",
		warning: "banner-warning card-warning",
	} as const;
</script>

<section
	class={`banner ${tone_class[event.severity]} flex min-w-[250px] flex-col -space-x-1`}
>
	<span>{event.title}</span>
	{#if event.description !== undefined}
		<p>{event.description}</p>
	{/if}
	{#if event.actions !== undefined && event.actions.length > 0}
		<div class="banner-actions">
			{#each event.actions as action (action.id)}
				{#if "href" in action}
					<a
						class="banner-action"
						href={action.href}
						onclick={() => {
							if (is_banner_executable_action(action)) onaction(action);
						}}
					>
						{#if action.icon === "player-play"}<PlayerPlay aria-hidden="true" />{/if}
						{#if action.icon === "refresh"}<Refresh aria-hidden="true" />{/if}
						<span>{action.label}</span>
					</a>
				{:else}
					<button type="button" class="banner-action" onclick={() => onaction(action)}>
						{#if action.icon === "player-play"}<PlayerPlay aria-hidden="true" />{/if}
						{#if action.icon === "refresh"}<Refresh aria-hidden="true" />{/if}
						<span>{action.label}</span>
					</button>
				{/if}
			{/each}
		</div>
	{/if}
</section>
