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

	interface ReachableForge {
		readonly endpoint: string;
		readonly profile: string;
		readonly self: boolean;
	}

	/**
	 * `artisan://forge/start` is an OS-global protocol handler, so it can only
	 * ever boot the installed default Forge. Offering it while this page is
	 * served by a differently named instance would boot the wrong process, so
	 * the button is gated on this origin's own profile and the gate instead
	 * lists the other announced instances to switch to.
	 */
	let origin_profile = $state<string | undefined>(undefined);
	let other_instances = $state<ReadonlyArray<ReachableForge>>([]);
	const show_start = $derived(
		presentation.show_start && (origin_profile === undefined || origin_profile === "default"),
	);

	$effect(() => {
		if (!is_visible || presentation.tone !== "error") return;
		let cancelled = false;

		const Discover = async () => {
			try {
				const health = await fetch("/health", { cache: "no-store" });
				if (health.ok) {
					const body: unknown = await health.json();
					const named =
						typeof body === "object" && body !== null && "profile" in body
							? (body as { readonly profile?: unknown }).profile
							: undefined;
					if (!cancelled && typeof named === "string") origin_profile = named;
				}
				const listing = await fetch("/api/instances", { cache: "no-store" });
				if (!listing.ok) return;
				const decoded: unknown = await listing.json();
				const instances =
					typeof decoded === "object" && decoded !== null && "instances" in decoded
						? (decoded as { readonly instances?: unknown }).instances
						: undefined;
				if (cancelled || !Array.isArray(instances)) return;
				other_instances = instances.filter(
					(candidate): candidate is ReachableForge =>
						typeof candidate === "object" &&
						candidate !== null &&
						typeof (candidate as ReachableForge).endpoint === "string" &&
						typeof (candidate as ReachableForge).profile === "string" &&
						(candidate as ReachableForge).self === false,
				);
			} catch {
				/** An unreachable origin cannot enumerate siblings; the gate keeps its plain remedies. */
			}
		};

		void Discover();
		return () => {
			cancelled = true;
		};
	});
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
									<span class="font-medium">{instance.profile}</span>
									<span class="truncate text-xs text-muted-foreground">{instance.endpoint}</span>
								</a>
							</li>
						{/each}
					</ul>
				</div>
			{/if}
		</section>
	</div>
{/if}
