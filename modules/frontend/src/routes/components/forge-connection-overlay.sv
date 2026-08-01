<script lang="ts" effect>
	import { ForgeStartLaunchUrl } from "@artisan/protocol";
	import PlayerPlay from "@tabler/icons-svelte/icons/player-play";
	import Refresh from "@tabler/icons-svelte/icons/refresh";
	import X from "@tabler/icons-svelte/icons/x";
	import { Effect, Option } from "effect";

	import { WriteClipboardText } from "$lib/browser/clipboard";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { Button } from "$lib/components/ui/button";
	import {
		PresentForgeGate,
		type ForgeGateModel,
	} from "$lib/forge/gate";
	import { DiscoverForge, type ReachableForge } from "$lib/forge/discovery";

	let {
		model,
		ondismiss,
		retry_connection,
		retry_hydration,
	}: {
		model: ForgeGateModel;
		ondismiss: Effect.Effect<void>;
		retry_connection: Effect.Effect<void>;
		retry_hydration: Effect.Effect<void>;
	} = $props();

	const banner = `
   █████████              █████     ███
  ███▒▒▒▒▒███            ▒▒███     ▒▒▒
 ▒███    ▒███  ████████  ███████   ████   █████   ██████   ████████
 ▒███████████ ▒▒███▒▒███▒▒▒███▒   ▒▒███  ███▒▒   ▒▒▒▒▒███ ▒▒███▒▒███
 ▒███▒▒▒▒▒███  ▒███ ▒▒▒   ▒███     ▒███ ▒▒█████   ███████  ▒███ ▒███
 ▒███    ▒███  ▒███       ▒███ ███ ▒███  ▒▒▒▒███ ███▒▒███  ▒███ ▒███
 █████   █████ █████      ▒▒█████  █████ ██████ ▒▒████████ ████ █████
▒▒▒▒▒   ▒▒▒▒▒ ▒▒▒▒▒        ▒▒▒▒▒  ▒▒▒▒▒ ▒▒▒▒▒▒   ▒▒▒▒▒▒▒▒ ▒▒▒▒ ▒▒▒▒▒`;

	const presentation = $derived(PresentForgeGate(model));
	const is_visible = $derived(model.state.phase !== "ready" && !model.dismissed);

	const DismissOnEscape = (event: KeyboardEvent) =>
		Effect.gen(function* () {
		if (!is_visible || !presentation.dismissible || event.key !== "Escape") return;
		yield* RunBrowserDom(() => event.preventDefault());
		yield* ondismiss;
		});

	/**
	 * `artisan://forge/start` is an OS-global protocol handler, so it can only
	 * ever boot the installed Forge. And a reachable /health while the
	 * transport fails means the server is fine but this browser holds no
	 * session — offering "Start" there would misdiagnose a pairing problem as
	 * an outage, so the gate switches to pairing guidance instead. A
	 * development origin is never bootable through the handler either, so a
	 * `development: true` health latches the start affordance off.
	 */
	let origin_development = $state(false);
	let origin_reachable = $state(false);
	let other_instances = $state<ReadonlyArray<ReachableForge>>([]);
	let pair_command_copied = $state(false);
	const unpaired = $derived(presentation.tone === "error" && origin_reachable);
	const show_start = $derived(
		presentation.show_start && !origin_reachable && !origin_development,
	);
	const pair_command = "ae open";
	let pair_command_copy_failed = $state(false);

	const CopyPairCommand = Effect.gen(function* () {
		pair_command_copy_failed = false;
		yield* WriteClipboardText(pair_command);
		pair_command_copied = true;
		yield* Effect.sleep("1500 millis");
		pair_command_copied = false;
	}).pipe(
		Effect.catchTag("ClipboardWriteError", () =>
			Effect.gen(function* () {
				pair_command_copied = false;
				pair_command_copy_failed = true;
			}),
		),
	);

	const ClearDiscovery = Effect.gen(function* () {
		origin_reachable = false;
		origin_development = false;
		other_instances = [];
	});
	const LoadDiscovery = Effect.gen(function* () {
		const { health, others } = yield* DiscoverForge;
		const reachable_health = Option.getOrUndefined(health);
		origin_reachable = reachable_health !== undefined;
		origin_development = reachable_health?.development === true;
		other_instances = others;
	}).pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
				yield* ClearDiscovery;
			}),
		),
	);

	// Top-level SER work re-runs when the gate's reactive visibility changes.
	if (is_visible && presentation.tone === "error") {
		yield* LoadDiscovery;
	} else {
		yield* ClearDiscovery;
	}
	let previous_focus: HTMLElement | null = null;
	let recovery_actions = $state<HTMLDivElement | null>(null);
	let status_element = $state<HTMLElement | null>(null);
	let was_visible = false;

	/**
	 * Focus returns to wherever it came from whether the gate closed because
	 * Forge arrived or because the user dismissed it, so this tracks the
	 * overlay's own visibility rather than the connection phase.
	 */
	const ManageOverlayFocus = Effect.gen(function* () {
		if (!is_visible) {
			const can_restore_focus = yield* RunBrowserDom(
				() => previous_focus?.isConnected === true,
			);
			if (was_visible && can_restore_focus) {
				yield* RunBrowserDom(() => previous_focus?.focus({ preventScroll: true }));
			}
			previous_focus = null;
			was_visible = false;
			return;
		}

		if (!was_visible) {
			previous_focus = yield* RunBrowserDom(() => {
				const active_element = document.activeElement;
				return active_element instanceof HTMLElement && active_element !== document.body
					? active_element
					: null;
			});
			was_visible = true;
		}

		const focus_target = yield* RunBrowserDom(() =>
			presentation.tone === "error"
				? recovery_actions?.querySelector<HTMLElement>("a, button")
				: status_element,
		);
		yield* RunBrowserDom(() =>
			(focus_target ?? status_element)?.focus({ preventScroll: true }),
		);
	});
	yield* ManageOverlayFocus;

	/** The quiet line that fades in when a progress phase overstays 5 seconds. */
	const progress_detail = $derived(
		model.state.phase === "reconnecting"
			? "Connection dropped — getting you back…"
			: model.state.phase === "hydrating"
				? "Loading your workspace…"
				: "Connecting to Artisan Forge…",
	);
	const failure_happened = $derived.by(() => {
		if (model.state.phase === "hydration-failed") {
			return model.state.error.length > 0
				? model.state.error
				: "Artisan Forge connected, but your workspace could not be loaded.";
		}
		if (unpaired) {
			return "Artisan Forge is running on this machine, but this browser holds no session for it.";
		}
		return "We couldn't reach Artisan Forge. The connection closed or never came up, and repeated attempts to get through didn't land.";
	});
	const failure_remedy = $derived.by(() => {
		if (model.state.phase === "hydration-failed") {
			return "This is usually temporary — retry loading. If it keeps happening, restart Artisan Forge and reconnect.";
		}
		if (unpaired) {
			return "Pair this browser from a terminal, then retry the connection:";
		}
		return "Start Artisan Forge and this screen gets out of your way — launch the installed app, or run “ae open” from a terminal. If it's already running, retry the connection.";
	});
</script>

<svelte:window onkeydown={yield* DismissOnEscape(event)} />

{#if is_visible}
	<div
		class="absolute inset-0 z-50 flex flex-col items-center justify-center gap-10 overflow-y-auto bg-background px-6"
	>
		{#if presentation.dismissible}
			<button
				type="button"
				class="absolute top-4 right-4 grid size-9 place-items-center rounded-full text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out hover:bg-surface-100 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none motion-reduce:transition-none dark:hover:bg-surface-900"
				aria-label="Dismiss and browse the disconnected client"
				onclick={yield* ondismiss}
			>
				<X class="size-4" aria-hidden="true" />
			</button>
		{/if}
		<!-- Focused programmatically for screen readers; the ring would outline the whole scene. -->
		<section
			bind:this={status_element}
			class="flex max-w-full flex-col items-center gap-10 outline-none"
			role={presentation.tone === "error" ? "alert" : "status"}
			aria-busy={presentation.tone === "progress"}
			aria-live={presentation.tone === "error" ? "assertive" : "polite"}
			tabindex="-1"
		>
			<pre
				class={presentation.tone === "progress"
					? "banner-art banner-shimmer max-w-full overflow-hidden select-none"
					: "banner-art max-w-full overflow-hidden text-foreground select-none"}
				data-text={banner}
				aria-label="Artisan">{banner}</pre>

			{#if presentation.tone === "error"}
				<div class="w-full max-w-xl">
					<p class="text-base font-medium text-destructive">
						Artisan Editor ran into a problem and could not continue.
					</p>
					<div class="mt-8 flex flex-col gap-8">
						<div>
							<p class="font-semibold text-foreground">What happened?</p>
							<p class="mt-1.5 text-sm leading-6 text-muted-foreground">
								{failure_happened}
							</p>
						</div>
						<div>
							<p class="font-semibold text-foreground">What to do now</p>
							<p class="mt-1.5 text-sm leading-6 text-muted-foreground">
								{failure_remedy}
							</p>
							{#if unpaired}
								<button
									type="button"
									class="mt-3 inline-flex items-center gap-2 rounded-lg bg-surface-100 px-3 py-1.5 font-mono text-xs text-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none dark:bg-surface-900"
					onclick={yield* CopyPairCommand}
								>
									<span>{pair_command}</span>
									<span class="text-muted-foreground">
										{pair_command_copied ? "copied" : "copy"}
									</span>
								</button>
								{#if pair_command_copy_failed}
									<p class="mt-2 text-sm text-destructive" role="status">
										Couldn't copy the command. Select it and copy manually.
									</p>
								{/if}
							{/if}
						</div>
						<div bind:this={recovery_actions} class="flex flex-wrap gap-2">
							{#if show_start}
								<Button href={ForgeStartLaunchUrl}>
									<PlayerPlay aria-hidden="true" />
									Start Artisan Forge
								</Button>
							{/if}
							{#if presentation.retry === "connection"}
								<Button
									variant={show_start ? "outline" : "default"}
									onclick={yield* retry_connection}
								>
									<Refresh aria-hidden="true" />
									Retry connection
								</Button>
							{:else if presentation.retry === "hydration"}
								<Button onclick={yield* retry_hydration}>
									<Refresh aria-hidden="true" />
									Retry loading
								</Button>
							{/if}
						</div>
						{#if other_instances.length > 0}
							<div class="w-full">
								<p
									class="text-xs font-medium tracking-wide text-muted-foreground uppercase"
								>
									Other Forge instances on this machine
								</p>
								<ul class="mt-2 flex flex-col gap-1.5">
									{#each other_instances as instance (instance.endpoint)}
										<li>
											<a
												href={instance.endpoint}
												class="flex items-baseline justify-between gap-3 rounded-xl bg-surface-100 px-3 py-2 text-sm text-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none dark:bg-surface-900"
											>
												<span class="truncate font-medium">
													{instance.endpoint}
												</span>
											</a>
										</li>
									{/each}
								</ul>
							</div>
						{/if}
					</div>
				</div>
			{:else}
				<!-- Keyed so re-entering a progress phase restarts the 5s intent delay. -->
				{#key model.state.phase}
					<p class="late-reassurance text-sm text-muted-foreground">
						<span class="font-medium">This is taking more time than expected…</span>
						{progress_detail}
					</p>
				{/key}
			{/if}
		</section>
	</div>
{/if}

<style>
	.banner-art {
		font-size: clamp(0.2rem, 0.5vw, 0.45rem);
		line-height: 1.2;
	}

	/*
	 * Background activity: the transitions.dev shimmer-text construction. The
	 * base glyphs render dimmed and constant; a ::before duplicate (via
	 * data-text) clips a sweeping highlight band onto the same glyphs, so
	 * contrast never drops. Pure CSS because transition directives deadlock
	 * the async renderer.
	 */
	.banner-shimmer {
		position: relative;
		color: var(--muted-foreground);
	}

	.banner-shimmer::before {
		content: attr(data-text);
		position: absolute;
		inset: 0;
		pointer-events: none;
		background-image: linear-gradient(
			90deg,
			transparent 0%,
			transparent 40%,
			var(--foreground) 50%,
			transparent 60%,
			transparent 100%
		);
		background-size: 400% 100%;
		background-repeat: no-repeat;
		-webkit-background-clip: text;
		background-clip: text;
		color: transparent;
		-webkit-text-fill-color: transparent;
		animation: t-shimmer 2000ms linear infinite;
	}

	@keyframes t-shimmer {
		0% {
			background-position: 100% 0;
		}

		100% {
			background-position: 0% 0;
		}
	}

	/*
	 * The reassurance line holds off for 5s of intent delay, then rises in
	 * with the texts-reveal treatment (fade + rise + blur settle).
	 */
	.late-reassurance {
		opacity: 0;
		animation: reassurance-in var(--duration-very-slow, 500ms) var(--ease-in-out, ease-in-out)
			5s forwards;
	}

	@keyframes reassurance-in {
		from {
			opacity: 0;
			transform: translateY(0.25rem);
			filter: blur(2px);
		}

		to {
			opacity: 1;
			transform: translateY(0);
			filter: blur(0);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.banner-shimmer::before {
			animation: none !important;
		}

		.banner-shimmer {
			color: var(--foreground);
		}

		/* The intent delay stays; only the motion goes. */
		.late-reassurance {
			animation: reassurance-in 0s linear 5s forwards;
		}
	}
</style>
