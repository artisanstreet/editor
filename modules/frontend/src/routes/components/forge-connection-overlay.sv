<script lang="ts" effect>
	import { ForgeStartLaunchUrl } from "@artisan/protocol";
	import AlertCircle from "@tabler/icons-svelte/icons/alert-circle";
	import Loader2 from "@tabler/icons-svelte/icons/loader-2";
	import PlayerPlay from "@tabler/icons-svelte/icons/player-play";
	import Refresh from "@tabler/icons-svelte/icons/refresh";
	import X from "@tabler/icons-svelte/icons/x";
	import { Effect, Option, Queue } from "effect";

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
		ondismiss: () => void;
		retry_connection: Effect.Effect<void>;
		retry_hydration: Effect.Effect<void>;
	} = $props();

	const presentation = $derived(PresentForgeGate(model));
	const is_visible = $derived(model.state.phase !== "ready" && !model.dismissed);

	const DismissOnEscape = (event: KeyboardEvent) => {
		if (!is_visible || !presentation.dismissible || event.key !== "Escape") return;
		event.preventDefault();
		ondismiss();
	};

	/**
	 * `artisan://forge/start` is an OS-global protocol handler, so it can only
	 * ever boot the installed Forge. And a reachable /health while the
	 * transport fails means the server is fine but this browser holds no
	 * session — offering "Start Forge" there would misdiagnose a pairing
	 * problem as an outage, so the gate switches to pairing guidance instead.
	 * A development origin is never bootable through the handler either, so a
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

	const copy_pair_command = Effect.tryPromise(() =>
		navigator.clipboard.writeText(pair_command),
	).pipe(
		Effect.andThen(
			Effect.sync(() => {
				pair_command_copied = true;
			}),
		),
		Effect.andThen(Effect.sleep("1500 millis")),
		Effect.andThen(
			Effect.sync(() => {
				pair_command_copied = false;
			}),
		),
		Effect.ignore,
	);

	const discovery_requests = yield* Queue.dropping<void>(1);
	$effect(() => {
		if (!is_visible || presentation.tone !== "error") {
			origin_reachable = false;
			return;
		}
		Queue.offerUnsafe(discovery_requests, undefined);
	});
	yield* Queue.take(discovery_requests).pipe(
		Effect.flatMap(() => DiscoverForge),
		Effect.flatMap(({ health, others }) =>
			Effect.sync(() => {
				const reachable_health = Option.getOrUndefined(health);
				origin_reachable = reachable_health !== undefined;
				origin_development = reachable_health?.development === true;
				other_instances = others;
			}),
		),
		Effect.forever,
		Effect.forkScoped,
	);
	let previous_focus: HTMLElement | null = null;
	let recovery_actions = $state<HTMLDivElement | null>(null);
	let status_element = $state<HTMLElement | null>(null);
	let was_visible = false;

	/**
	 * Focus returns to wherever it came from whether the gate closed because
	 * Forge arrived or because the user dismissed it, so this tracks the
	 * overlay's own visibility rather than the connection phase.
	 */
	$effect(() => {
		if (!is_visible) {
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

<svelte:window onkeydown={DismissOnEscape} />

{#if is_visible}
	<div
		class="absolute inset-0 z-50 grid place-items-center bg-background/45 px-6 backdrop-blur-md supports-[backdrop-filter]:bg-background/35"
	>
		{#if presentation.dismissible}
			<button
				type="button"
				class="absolute top-4 right-4 grid size-9 place-items-center rounded-full text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out hover:bg-surface-100 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none motion-reduce:transition-none dark:hover:bg-surface-900"
				aria-label="Dismiss and browse the disconnected client"
				onclick={ondismiss}
			>
				<X class="size-4" aria-hidden="true" />
			</button>
		{/if}
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
				{unpaired ? "This browser isn't paired" : presentation.title}
			</h2>
			<p class="mt-1.5 max-w-sm text-sm leading-6 text-muted-foreground">
				{unpaired
					? "Forge is running, but this browser holds no session for it. Pair from a terminal, then retry."
					: presentation.description}
			</p>

			{#if unpaired}
				<button
					type="button"
					class="mt-3 inline-flex items-center gap-2 rounded-lg bg-surface-100 px-3 py-1.5 font-mono text-xs text-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none dark:bg-surface-900"
					onclick={yield* copy_pair_command}
				>
					<span>{pair_command}</span>
					<span class="text-muted-foreground">{pair_command_copied ? "copied" : "copy"}</span>
				</button>
			{/if}

			{#if show_start || presentation.retry !== undefined}
				<div
					bind:this={recovery_actions}
					class="mt-5 flex flex-wrap justify-center gap-2"
				>
					{#if show_start}
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

			{#if presentation.tone === "error" && other_instances.length > 0}
				<div class="mt-6 w-full max-w-sm text-left">
					<p class="text-xs font-medium tracking-wide text-muted-foreground uppercase">
						Other Forge instances on this machine
					</p>
					<ul class="mt-2 flex flex-col gap-1.5">
						{#each other_instances as instance (instance.endpoint)}
							<li>
								<a
									href={instance.endpoint}
									class="flex items-baseline justify-between gap-3 rounded-xl bg-surface-100 px-3 py-2 text-sm text-foreground hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:outline-none dark:bg-surface-900"
								>
									<span class="truncate font-medium">{instance.endpoint}</span>
								</a>
							</li>
						{/each}
					</ul>
				</div>
			{/if}
		</section>
	</div>
{/if}
