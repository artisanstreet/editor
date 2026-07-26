<script lang="ts" effect>
	import { dev } from "$app/environment";
	import Settings from "@tabler/icons-svelte/icons/settings";
	import { Effect, Fiber } from "effect";
	import { Button } from "$lib/components/ui/button";
	import ShaderDevPanel from "./shader-dev-panel.sv";

	type PanelState = "closed" | "open" | "closing";

	let panel_state: PanelState = $state("closed");
	let close_fiber: Fiber.Fiber<void> | undefined;

	const OpenPanel = Effect.gen(function* () {
		if (close_fiber !== undefined) yield* Fiber.interrupt(close_fiber);
		panel_state = "open";
	});

	const ClosePanel = Effect.gen(function* () {
		panel_state = "closing";
		close_fiber = yield* Effect.forkScoped(
			Effect.sleep("150 millis").pipe(
				Effect.andThen(Effect.sync(() => {
					panel_state = "closed";
				})),
			),
		);
	});

	const TogglePanel = Effect.gen(function* () {
		if (panel_state === "open") yield* ClosePanel;
		else yield* OpenPanel;
	});

	const HandleKeydown = (event: KeyboardEvent) =>
		event.key === "Escape" && panel_state === "open" ? ClosePanel : Effect.void;
</script>

<svelte:window onkeydown={yield* HandleKeydown(event)} />

<div class="relative flex h-full min-h-0 flex-col p-4">
	{#if dev}
		<Button
			variant="outline"
			size="icon-sm"
			class="absolute right-0 bottom-0 z-30 bg-background/80 backdrop-blur-xl"
			onclick={yield* TogglePanel}
			aria-label="Shader settings"
			aria-controls="shader-development-panel"
			aria-expanded={panel_state === "open"}
		>
			<Settings class="size-4 text-muted-foreground" />
		</Button>

		<div
			id="shader-development-panel"
			class:is-open={panel_state === "open"}
			class:is-closing={panel_state === "closing"}
			class="t-dropdown absolute inset-x-2 top-2 bottom-10 z-20 flex min-h-0 flex-col overflow-hidden bg-background/95 p-3 backdrop-blur-xl card"
			data-origin="bottom-right"
			aria-hidden={panel_state === "closed"}
		>
			<ShaderDevPanel />
		</div>
	{/if}
</div>

<style>
	:global(:root) {
		--dropdown-open-dur: 250ms;
		--dropdown-close-dur: 150ms;
		--dropdown-pre-scale: 0.97;
		--dropdown-closing-scale: 0.99;
		--dropdown-ease: cubic-bezier(0.22, 1, 0.36, 1);
	}

	.t-dropdown {
		transform-origin: top left;
		transform: scale(var(--dropdown-pre-scale));
		opacity: 0;
		pointer-events: none;
		transition:
			transform var(--dropdown-open-dur) var(--dropdown-ease),
			opacity var(--dropdown-open-dur) var(--dropdown-ease);
		will-change: transform, opacity;
	}

	.t-dropdown[data-origin="bottom-right"] {
		transform-origin: bottom right;
	}

	.t-dropdown.is-open {
		transform: scale(1);
		opacity: 1;
		pointer-events: auto;
	}

	.t-dropdown.is-closing {
		transform: scale(var(--dropdown-closing-scale));
		opacity: 0;
		pointer-events: none;
		transition:
			transform var(--dropdown-close-dur) var(--dropdown-ease),
			opacity var(--dropdown-close-dur) var(--dropdown-ease);
	}

	@media (prefers-reduced-motion: reduce) {
		.t-dropdown {
			transition: none !important;
		}
	}
</style>
