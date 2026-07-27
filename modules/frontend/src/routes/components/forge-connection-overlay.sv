<script lang="ts" effect>
	import { ForgeStartLaunchUrl } from "@artisan/protocol";
	import AlertCircle from "@tabler/icons-svelte/icons/alert-circle";
	import Loader2 from "@tabler/icons-svelte/icons/loader-2";
	import PlayerPlay from "@tabler/icons-svelte/icons/player-play";
	import Refresh from "@tabler/icons-svelte/icons/refresh";
	import type { Effect } from "effect";

	import { Button } from "$lib/components/ui/button";
	import {
		PresentForgeGate,
		type ForgeGateModel,
	} from "$lib/forge/gate";

	let {
		model,
		retry_connection,
		retry_hydration,
	}: {
		model: ForgeGateModel;
		retry_connection: Effect.Effect<void>;
		retry_hydration: Effect.Effect<void>;
	} = $props();

	const presentation = $derived(PresentForgeGate(model));
	const is_visible = $derived(model.state.phase !== "ready");
	let previous_focus: HTMLElement | null = null;
	let recovery_actions = $state<HTMLDivElement | null>(null);
	let status_element = $state<HTMLElement | null>(null);
	let was_visible = false;

	$effect(() => {
		const phase = model.state.phase;
		if (phase === "ready") {
			if (was_visible && previous_focus?.isConnected === true) {
				previous_focus.focus({ preventScroll: true });
			}
			previous_focus = null;
			was_visible = false;
			return;
		}

		if (!was_visible) {
			previous_focus =
				document.activeElement instanceof HTMLElement &&
				document.activeElement !== document.body
					? document.activeElement
					: null;
			was_visible = true;
		}

		const focus_target =
			presentation.tone === "error"
				? recovery_actions?.querySelector<HTMLElement>("a, button")
				: status_element;
		(focus_target ?? status_element)?.focus({ preventScroll: true });
	});
</script>

{#if is_visible}
	<div
		class="absolute inset-0 z-50 grid place-items-center bg-background/45 px-6 backdrop-blur-md supports-[backdrop-filter]:bg-background/35"
	>
		<section
			bind:this={status_element}
			class="flex max-w-md flex-col items-center text-center"
			role={presentation.tone === "error" ? "alert" : "status"}
			aria-busy={presentation.tone === "progress"}
			aria-live={presentation.tone === "error" ? "assertive" : "polite"}
			tabindex="-1"
		>
			<div
				class="grid size-11 place-items-center rounded-full bg-surface-100 card dark:bg-surface-900"
			>
				{#if presentation.tone === "error"}
					<AlertCircle class="size-5 text-destructive" aria-hidden="true" />
				{:else}
					<Loader2
						class="size-5 animate-spin text-muted-foreground motion-reduce:animate-none"
						aria-hidden="true"
					/>
				{/if}
			</div>

			<h2
				class={`mt-4 text-lg font-medium tracking-tight ${
					presentation.tone === "error" ? "text-destructive" : "text-foreground"
				}`}
			>
				{presentation.title}
			</h2>
			<p class="mt-1.5 max-w-sm text-sm leading-6 text-muted-foreground">
				{presentation.description}
			</p>

			{#if presentation.show_start || presentation.retry !== undefined}
				<div
					bind:this={recovery_actions}
					class="mt-5 flex flex-wrap justify-center gap-2"
				>
					{#if presentation.show_start}
						<Button href={ForgeStartLaunchUrl} variant="destructive">
							<PlayerPlay aria-hidden="true" />
							Start Forge
						</Button>
					{/if}
					{#if presentation.retry === "connection"}
						<Button variant="outline" onclick={yield* retry_connection}>
							<Refresh aria-hidden="true" />
							Retry connection
						</Button>
					{:else if presentation.retry === "hydration"}
						<Button variant="outline" onclick={yield* retry_hydration}>
							<Refresh aria-hidden="true" />
							Retry loading
						</Button>
					{/if}
				</div>
			{/if}
		</section>
	</div>
{/if}
