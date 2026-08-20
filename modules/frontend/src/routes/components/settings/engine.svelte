<script lang="ts" effect>
	import Refresh from "@tabler/icons-svelte/icons/refresh";
	import { Effect, Stream } from "effect";
	import type { EngineUsageReport } from "@artisan/protocol";
	import { Button } from "$lib/components/ui/button";
	import { EngineMarkClass, EngineMarkFor, ProviderMarkFor } from "$lib/engine/presentation";
	import { ModelsFromCatalog } from "$lib/engine/model-selection";
	import {
		EngineInstallationsController,
		type EngineInstallationsState,
	} from "$lib/settings/engine-installations-controller";
	import {
		SessionDefaultsController,
		type SessionDefaultsState,
	} from "$lib/settings/session-defaults-controller";
	import { EngineUsageController, type EngineUsageState } from "$lib/identity/engine-usage-controller";
	import { Badge } from "$lib/components/ui/badge";
	import { Skeleton } from "$lib/components/ui/skeleton";
	import { Switch } from "$lib/components/ui/switch";
	import Card from "./card.svelte";
	import Header from "./header.svelte";
	import Row from "./row.svelte";
	import Section from "./section.svelte";

	let { engine_id }: { engine_id: string } = $props();

	const usage_controller = yield* EngineUsageController;
	const defaults_controller = yield* SessionDefaultsController;
	const installations_controller = yield* EngineInstallationsController;
	/** Shell hydration owns defaults; Settings paints from its retained snapshot. */
	let defaults_state = $state.raw<SessionDefaultsState>(yield* defaults_controller.Current);
	const ApplyDefaults = (next: SessionDefaultsState) =>
		Effect.gen(function* () {
			defaults_state = next;
		});
	yield* defaults_controller.Changes.pipe(
		Stream.runForEach(ApplyDefaults),
		Effect.forkScoped,
	);
	const runtime_catalog = $derived(defaults_state.catalog);
	const forge_available = $derived(defaults_state.available);
	const catalog_models = $derived(ModelsFromCatalog(runtime_catalog));

	const harness = $derived(
		runtime_catalog.manifest.harnesses.find((candidate) => candidate.id === engine_id),
	);
	/**
	 * The blanket switch. Off means this engine is not represented as available
	 * anywhere — no selector section, no usage read, no models — until it is
	 * switched back on. Everything below the switch exists only while it is on.
	 */
	const engine_enabled = $derived(
		!(defaults_state.defaults.disabled_engines ?? []).includes(engine_id),
	);
	let availability_saving = $state(false);

	const ToggleAvailability = (enabled: boolean) =>
		Effect.gen(function* () {
			if (availability_saving) return;
			availability_saving = true;
			yield* defaults_controller.SetEngineEnabled(engine_id, enabled).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
					}),
				),
				Effect.ensuring(
					Effect.gen(function* () {
						availability_saving = false;
					}),
				),
			);
		});
	const engine_mark = $derived(EngineMarkFor(engine_id));
	const engine_models = $derived(catalog_models.filter((model) => model.engine === engine_id));
	const compaction_default = $derived(
		catalog_models.find((model) => model.id === harness?.compaction_default_model_id),
	);

	/** Installation status is similarly retained so route navigation never waits on Forge. */
	let installations_state = $state.raw<EngineInstallationsState>(
		yield* installations_controller.Current,
	);
	const ApplyInstallations = (next: EngineInstallationsState) =>
		Effect.gen(function* () {
			installations_state = next;
		});
	yield* installations_controller.Changes.pipe(
		Stream.runForEach(ApplyInstallations),
		Effect.forkScoped,
	);
	const LoadInstallations = (page_engine_id: string) =>
		installations_controller.Refresh({ engine_id: page_engine_id }).pipe(Effect.ignore);
	yield* LoadInstallations(engine_id).pipe(Effect.forkScoped);
	const current_installation = $derived(installations_state.reports[engine_id]);
	const installation_pending = $derived(installations_state.pending_engine_ids.has(engine_id));
	const installation_error = $derived(installations_state.errors[engine_id]);
	const installation_phase_labels: Readonly<Record<string, string>> = {
		authenticating: "Complete sign-in in your browser…",
		checking: "Checking release…",
		downloading: "Downloading…",
		provisioning: "Preparing managed home…",
		resolving: "Resolving release…",
		staging: "Staging…",
		verifying: "Verifying…",
	};
	const installation_status = $derived(
		installation_pending ||
			current_installation?.activity === "installing" ||
			current_installation?.activity === "authenticating"
			? (installation_phase_labels[current_installation?.activity_phase ?? ""] ??
				"Installing…")
			: current_installation?.activity === "failed"
				? (installation_error ??
					current_installation.failure ??
					"The managed installation did not complete.")
				: current_installation?.activity === "idle" && current_installation.managed
					? "Managed installation ready."
					: installation_error,
	);
	const InstallationAction = (version?: string) =>
		Effect.gen(function* () {
			return yield* installations_controller.Install(engine_id, version).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
						return yield* installations_controller.Current;
					}),
				),
			);
		});
	const RollbackInstallation = Effect.gen(function* () {
		return yield* installations_controller.Rollback(engine_id).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					return yield* installations_controller.Current;
				}),
			),
		);
	});
	const CheckInstallationUpdates = Effect.gen(function* () {
		return yield* installations_controller.Refresh({
			check_updates: true,
			engine_id,
		}).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					return yield* installations_controller.Current;
				}),
			),
		);
	});

	let usage = $state<EngineUsageReport | undefined>(undefined);
	let usage_loaded = $state(false);
	let usage_refreshing = $state(false);
	const ApplyUsageState = (next: EngineUsageState) =>
		Effect.gen(function* () {
			const entry = next.entries.get(engine_id);
			if (entry !== undefined) {
				usage = entry.report;
				usage_loaded = true;
			}
			usage_refreshing = next.refreshing_engine_ids.has(engine_id);
		});
	yield* usage_controller.Changes.pipe(
		Stream.runForEach(ApplyUsageState),
		Effect.forkScoped,
	);

	const StoreUsage = (page_engine_id: string, report: EngineUsageReport | undefined) =>
		Effect.gen(function* () {
			if (engine_id !== page_engine_id) return;
			usage = report;
			usage_loaded = true;
			usage_refreshing = false;
		});

	const LoadUsage = (page_engine_id: string, force: boolean) =>
		Effect.gen(function* () {
			const entry = yield* usage_controller.Load(page_engine_id, { force });
			yield* StoreUsage(
				page_engine_id,
				entry.report,
			);
		}).pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					yield* StoreUsage(page_engine_id, undefined);
				}),
			),
		);

	const RefreshUsage = Effect.gen(function* () {
		if (!forge_available || usage_refreshing) return;
		usage_refreshing = true;
		yield* LoadUsage(engine_id, true);
	});
	const AuthenticateInstallation = (current_engine_id: string) =>
		Effect.gen(function* () {
			return yield* installations_controller.Authenticate(current_engine_id).pipe(
				/** Authentication admission is enough to paint pending state; usage is advisory. */
				Effect.tap(() =>
					Effect.gen(function* () {
						yield* RefreshUsage.pipe(Effect.forkScoped);
					}),
				),
				Effect.catch(() => installations_controller.Current),
			);
		});

	/**
	 * `engine_id` and the availability switch are reactive inputs: navigating
	 * between engine pages refetches, and flipping the switch on fetches the
	 * first reading for the sections that just appeared — which paint their
	 * skeletons immediately while it loads. A switched-off engine is never
	 * asked for anything.
	 */
	const LoadInitialUsage = (page_engine_id: string, enabled: boolean) =>
		Effect.gen(function* () {
			if (!enabled) {
				if (engine_id !== page_engine_id) return;
				usage = undefined;
				usage_loaded = false;
				return;
			}
			yield* LoadUsage(page_engine_id, false);
		});
	yield* LoadInitialUsage(engine_id, engine_enabled).pipe(Effect.forkScoped);

	/** Only paint a report that belongs to the page being viewed. */
	const current_usage = $derived(usage?.engine_id === engine_id ? usage : undefined);
	const window_kind_labels: Readonly<Record<string, string>> = {
		monthly: "Monthly",
		session: "Session",
		unknown: "Usage",
		weekly: "Weekly",
	};
	const window_reset_label = (resets_at: string | undefined) =>
		resets_at === undefined
			? undefined
			: `Resets ${new Date(resets_at).toLocaleString(undefined, {
					day: "numeric",
					hour: "numeric",
					minute: "2-digit",
					month: "short",
				})}`;
</script>

{#if harness === undefined}
	<Header
		title="Unknown engine"
		description={`No engine with id "${engine_id}" exists in the catalog.`}
	/>
{:else}
	{@const EngineIcon = engine_mark.icon}
	<Header
		title={harness.label}
		description={`Choose where ${harness.label} appears, manage its installation, and inspect its account and models.`}
	>
		{#snippet mark()}
			<EngineIcon
				class={engine_mark.monochrome ? "size-5 shrink-0 dark:invert" : "size-5 shrink-0"}
			/>
		{/snippet}
	</Header>

	<Section id="availability" title="Availability">
		<Card class="mt-3">
			<Row
				title={`Enable ${harness.label}`}
				description="Whether this engine is represented as available at all. Off, its models leave the model picker and its account is never asked for usage."
			>
				{#snippet control()}
					<Switch
						checked={engine_enabled}
						disabled={availability_saving}
						aria-label={`Enable ${harness.label}`}
						onclick={yield* ToggleAvailability(!engine_enabled)}
					/>
				{/snippet}
			</Row>
		</Card>
	</Section>

	<Section id="installation" title="Installation">
		{#if installation_status !== undefined}
			<span class="sr-only" role="status" aria-live="polite" aria-atomic="true">
				{installation_status}
			</span>
		{/if}
		{#snippet action()}
			<Button
				variant="ghost"
				size="xs"
				disabled={installation_pending}
				onclick={yield* CheckInstallationUpdates}
			>
				<Refresh class="size-3.5 shrink-0" aria-hidden="true" />
				{installations_state.load_error === undefined ? "Check for updates" : "Retry"}
			</Button>
		{/snippet}
		<Card class="mt-3">
			{#if !installations_state.available}
				<div class="flex flex-col gap-3 px-5 py-5">
					{#if installations_state.load_error === undefined}
						<Skeleton class="h-4 w-44" />
						<Skeleton class="h-3 w-72" />
					{:else}
						<p class="text-sm text-destructive">{installations_state.load_error}</p>
						<Button size="sm" class="w-fit" onclick={yield* CheckInstallationUpdates}>
							Retry
						</Button>
					{/if}
				</div>
			{:else if current_installation === undefined}
				<div class="flex flex-col gap-3 px-5 py-5">
					<p class="text-sm text-muted-foreground">Installation status is unavailable.</p>
					<Button size="sm" class="w-fit" disabled>Install</Button>
				</div>
			{:else if installation_pending ||
				current_installation.activity === "installing" ||
				current_installation.activity === "authenticating"}
				<div class="flex flex-col gap-2 px-5 py-5">
					<span class="text-sm text-foreground">
						{installation_phase_labels[current_installation.activity_phase ?? ""] ??
							"Installing…"}
					</span>
					<p class="text-xs text-muted-foreground">
						Artisan is preparing its managed copy and isolated provider home.
					</p>
				</div>
			{:else if current_installation.activity === "failed"}
				<div class="flex flex-col gap-3 px-5 py-5">
					<div class="flex flex-col gap-1">
						<span class="text-sm text-destructive">Installation failed</span>
						<p class="text-xs text-muted-foreground">
							{installation_status ?? "The managed installation did not complete."}
						</p>
					</div>
					<Button
						size="sm"
						class="w-fit"
						disabled={installation_pending}
						onclick={yield* InstallationAction(current_installation.active_version)}
					>
						{current_installation.active_version === undefined ? "Retry install" : "Repair"}
					</Button>
				</div>
			{:else if !current_installation.managed}
				<div class="flex flex-col gap-3 px-5 py-5">
					<div class="flex flex-col gap-1">
						<span class="text-sm text-foreground">Not installed</span>
						<p class="text-xs text-muted-foreground">
							Artisan downloads and runs its own verified copy, with a separate provider home.
						</p>
					</div>
					{#if installation_error !== undefined || current_installation.failure !== undefined}
						<p class="text-xs text-destructive">
							{installation_error ?? current_installation.failure}
						</p>
					{/if}
					<Button size="sm" class="w-fit" disabled={installation_pending} onclick={yield* InstallationAction()}>
						{installation_error === undefined && current_installation.failure === undefined
							? "Install"
							: "Retry"}
					</Button>
				</div>
			{:else}
				<div class="flex flex-col gap-3 px-5 py-5">
					<div class="flex flex-col gap-1">
						<span class="text-sm text-foreground">
							Installed{current_installation.active_version === undefined
								? ""
								: ` · ${current_installation.active_version}`}
						</span>
						<p class="text-xs text-muted-foreground">
							Managed by Artisan with an isolated provider home.
						</p>
					</div>
					<div class="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
						{#if current_installation.recommended_version !== undefined}
							<span>Recommended {current_installation.recommended_version}</span>
						{/if}
						{#if current_installation.latest_version !== undefined}
							<span>Latest {current_installation.latest_version}</span>
						{/if}
					</div>
					{#if installation_error !== undefined || current_installation.failure !== undefined}
						<p class="text-xs text-destructive">
							{installation_error ?? current_installation.failure}
						</p>
					{/if}
					<div class="flex flex-wrap gap-2">
						{#if current_installation.update_available === true}
							<Button size="sm" disabled={installation_pending} onclick={yield* InstallationAction()}>
								Update
							</Button>
						{/if}
						{#if current_installation.failure !== undefined &&
							current_installation.update_available !== true}
							<Button
								size="sm"
								disabled={installation_pending}
								onclick={yield* InstallationAction(current_installation.active_version)}
							>
								Repair
							</Button>
						{/if}
						{#if current_installation.previous_version !== undefined}
							<Button
								size="sm"
								variant="outline"
								disabled={installation_pending}
								onclick={yield* RollbackInstallation}
							>
								Rollback to {current_installation.previous_version}
							</Button>
						{/if}
					</div>
				</div>
			{/if}
		</Card>
	</Section>

	{#if engine_enabled}
		<Section id="account" title="Account">
			{#snippet action()}
				<Button
					variant="ghost"
					size="xs"
					disabled={!forge_available || usage_refreshing}
					onclick={yield* RefreshUsage}
				>
					<Refresh
						class={usage_refreshing
							? "size-3.5 shrink-0 animate-spin motion-reduce:animate-none"
							: "size-3.5 shrink-0"}
						aria-hidden="true"
					/>
					{usage_refreshing ? "Refreshing…" : "Refresh"}
				</Button>
			{/snippet}
			<Card class="mt-3">
				{#if !usage_loaded}
					<div class="flex flex-col gap-5 px-5 py-5">
						<Skeleton class="h-4 w-44" />
						<Skeleton class="h-3 w-32" />
						<Skeleton class="h-1.5 w-full" />
						<Skeleton class="h-3 w-24" />
						<Skeleton class="h-1.5 w-full" />
					</div>
				{:else}
					<div class="flex animate-[settings-usage-enter_var(--duration-quick)_var(--ease-smooth-out)] flex-col gap-5 px-5 py-5">
					{#if current_usage === undefined}
						<span class="text-sm text-muted-foreground">Account unavailable</span>
					{:else if current_usage.authentication === "authenticated"}
						<span class="text-sm text-foreground">
							{#if current_usage.account_email}
								Signed in as
								<span class="font-medium text-foreground">
									{current_usage.account_email}
								</span>
							{:else}
								Signed in
							{/if}
						</span>
					{:else if current_usage.authentication === "unauthenticated"}
						<div class="flex flex-wrap items-center gap-3">
							<span class="text-sm text-destructive">Not signed in</span>
							{#if current_installation?.managed === true}
								<Button
									size="sm"
									disabled={installation_pending}
									onclick={yield* AuthenticateInstallation(engine_id)}
								>
									{current_installation.activity === "authenticating"
										? "Complete sign-in in browser…"
										: "Sign in"}
								</Button>
							{/if}
						</div>
					{:else}
						<span class="text-sm text-muted-foreground">Sign-in status unknown</span>
					{/if}
					{#if current_usage?.failure !== undefined}
						<p class="text-pretty text-xs text-muted-foreground">
							{current_usage.failure}
						</p>
					{/if}
					{#if current_usage !== undefined && current_usage.windows.length > 0}
						<div class="flex flex-col gap-4">
							{#each current_usage.windows as usage_window (usage_window.id)}
								<div class="flex flex-col gap-1">
									<div class="flex items-center justify-between gap-4 text-xs">
										<span class="truncate text-muted-foreground">
											{usage_window.label ??
												window_kind_labels[usage_window.kind] ??
												"Usage"}
										</span>
										<span class="shrink-0 text-foreground">
											{Math.round(usage_window.percent_used)}%
										</span>
									</div>
									<div
										class="h-1.5 w-full overflow-hidden rounded-full bg-surface-500/30"
									>
										<div
											class="h-full rounded-full bg-foreground/70 transition-[width] duration-(--duration-quick) ease-(--ease-smooth-out) motion-reduce:transition-none"
											style={`width: ${Math.min(usage_window.percent_used, 100)}%`}
										></div>
									</div>
									{#if window_reset_label(usage_window.resets_at) !== undefined}
										<span class="text-[0.7rem] text-muted-foreground">
											{window_reset_label(usage_window.resets_at)}
										</span>
									{/if}
								</div>
							{/each}
						</div>
					{:else if current_usage !== undefined && current_usage.authentication === "authenticated"}
						<p class="text-xs text-muted-foreground">
							This provider does not expose a quota surface.
						</p>
					{/if}
					</div>
				{/if}
			</Card>
		</Section>

		<Section id="models" title="Models">
			<Card class="mt-3">
				{#each engine_models as model (model.id)}
					{@const lab_mark = ProviderMarkFor(model.definition.provider)}
					{@const LabIcon = lab_mark.icon}
					<div class="flex items-center justify-between gap-4 px-4 py-2.5">
						<span class="flex min-w-0 items-center gap-2.5">
							<LabIcon class={EngineMarkClass(lab_mark, "size-4 shrink-0")} />
							<span class="flex min-w-0 flex-col">
								<span class="truncate text-sm text-foreground">{model.name}</span>
								{#if model.definition.description !== undefined}
									<span class="truncate text-xs text-muted-foreground">
										{model.definition.description}
									</span>
								{/if}
							</span>
						</span>
						<span class="flex shrink-0 items-center gap-1.5">
							{#if model.id === compaction_default?.id}
								<Badge variant="outline">Compaction default</Badge>
							{/if}
							{#if model.definition.disabled !== undefined}
								<Badge variant="outline">Disabled</Badge>
							{/if}
						</span>
					</div>
				{/each}
			</Card>
		</Section>

	{:else}
		<!--
			Nothing below the switch while it is off: painting a dimmed account and
			model list would say "present but broken", and the switch's whole claim
			is that this engine is absent.
		-->
		<p class="mt-8 text-sm text-muted-foreground">
			{harness.label} is switched off. Its models are hidden everywhere until it is
			enabled again.
		</p>
	{/if}
{/if}
