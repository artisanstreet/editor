<script lang="ts" effect>
	import type { UsageInterruption } from "@artisan/protocol";
	import AlertTriangle from "@tabler/icons-svelte/icons/alert-triangle";
	import Refresh from "@tabler/icons-svelte/icons/refresh";
	import { Clock, Effect } from "effect";
	import { Switch } from "$lib/components/ui/switch";

	type ResolveAction =
		| { readonly type: "set_auto_continue"; readonly enabled: boolean }
		| {
				readonly type: "continue";
				readonly target_engine_id: string;
				readonly target_model_id?: string;
		  }
		| { readonly type: "cancel" };

	let {
		interruption,
		onresolve,
	}: {
		interruption: UsageInterruption;
		onresolve?: (
			interruption_id: string,
			expected_revision: number,
			action: ResolveAction,
		) => Effect.Effect<void, { readonly message: string }>;
	} = $props();

	let now_ms = $state(yield* Clock.currentTimeMillis);
	let action_message = $state("");
	let submitting = $state(false);
	const reset_ms = $derived(
		interruption.resets_at === undefined
			? undefined
			: Date.parse(interruption.resets_at),
	);
	const remaining_ms = $derived(reset_ms === undefined ? undefined : Math.max(0, reset_ms - now_ms));
	const reset_countdown = $derived(
		remaining_ms === undefined
			? undefined
			: FormatCountdown(remaining_ms),
	);
	const DisplayModel = (model_id: string): string =>
		model_id
			.replace(/[-_]+/g, " ")
			.split(" ")
			.filter((part) => part.length > 0)
			.map((part) => {
				if (part.toLowerCase() === "gpt") return "GPT";
				return `${part[0]?.toUpperCase() ?? ""}${part.slice(1)}`;
			})
			.join(" ");
	const source_name = $derived(
		interruption.source_model_id === undefined
			? "the same model"
			: DisplayModel(interruption.source_model_id),
	);
	const engine_name = $derived(
		interruption.source_engine_id.length === 0
			? "provider"
			: `${interruption.source_engine_id[0]?.toUpperCase() ?? ""}${interruption.source_engine_id.slice(1)}`,
	);
	const is_open = $derived(
		interruption.state === "scheduled" || interruption.state === "awaiting_decision",
	);
	const can_continue = $derived(
		is_open && interruption.source_engine_id.length > 0 && !submitting,
	);

	const FormatCountdown = (milliseconds: number): string => {
		if (milliseconds <= 0) return "now";
		const total_seconds = Math.ceil(milliseconds / 1000);
		const hours = Math.floor(total_seconds / 3600);
		const minutes = Math.floor((total_seconds % 3600) / 60);
		if (hours > 0) return `${hours}h ${minutes}m`;
		return minutes === 0 ? "less than a minute" : `${minutes}m`;
	};

	const TitleFor = (value: UsageInterruption): string => {
		if (value.limit_label !== undefined) return `Your ${value.limit_label} limit was depleted`;
		if (value.limit_scope === "shared") return `Your ${engine_name} shared limit was depleted`;
		if (value.limit_scope === "model") {
			const model = value.affected_model_id ?? value.source_model_id;
			if (model !== undefined) return `Your ${DisplayModel(model)} limit was depleted`;
		}
		return `Your ${engine_name} usage limit was depleted`;
	};

	const HistoricalLabelFor = (value: UsageInterruption): string => {
		if (value.state === "continued") {
			return value.target_model_id === undefined
				? "Continued with the same model"
				: `Continued with ${DisplayModel(value.target_model_id)}`;
		}
		if (value.state === "cancelled") return "Continuation was cancelled";
		if (value.state === "failed") return "Continuation could not be started";
		if (value.state === "launching") return "Starting continuation…";
		return "Waiting for usage to become available";
	};

	const title = $derived(TitleFor(interruption));
	const historical_label = $derived(HistoricalLabelFor(interruption));

	const Resolve = (action: ResolveAction) =>
		Effect.gen(function* () {
			if (onresolve === undefined || submitting || !is_open) return;
			action_message = "";
			submitting = true;
			yield* onresolve(interruption.interruption_id, interruption.revision, action).pipe(
				Effect.catch((error) =>
					Effect.gen(function* () {
						action_message = error.message;
					}),
				),
				Effect.ensuring(
					Effect.gen(function* () {
						submitting = false;
					}),
				),
			);
		});

	const ResumeDefault = () => Resolve({ type: "set_auto_continue", enabled: true });

	const KeepCountdownCurrent = (
		state: UsageInterruption["state"],
		deadline_ms: number | undefined,
	) =>
		Effect.gen(function* () {
		while (
			(state === "scheduled" || state === "awaiting_decision") &&
			deadline_ms !== undefined &&
			deadline_ms > now_ms
		) {
			yield* Effect.sleep("1 second");
			now_ms = yield* Clock.currentTimeMillis;
		}
		});

	/** `state` and `reset_ms` stay direct inputs so an item-upsert restarts this scoped ticker. */
	yield* KeepCountdownCurrent(interruption.state, reset_ms).pipe(Effect.forkScoped);
</script>

<section
	class="card flex w-full flex-col gap-3 overflow-hidden rounded-2xl bg-linear-to-t from-warning/12 to-surface-200 p-4 dark:to-surface-900"
	aria-labelledby={`usage-interruption-${interruption.interruption_id}`}
>
	<div class="flex min-w-0 items-start gap-3">
		<div class="mt-0.5 rounded-lg bg-warning/15 p-1.5 text-warning">
			<AlertTriangle class="size-4" aria-hidden="true" />
		</div>
		<div class="min-w-0 flex-1">
			<h3 id={`usage-interruption-${interruption.interruption_id}`} class="font-semibold text-foreground">
				{title}
			</h3>
			{#if is_open}
				<p class="mt-1 text-sm text-muted-foreground">
					{source_name} stopped before it could finish. Artisan saved this thread and can continue it safely.
				</p>
			{:else}
				<p class="mt-1 text-sm text-muted-foreground">{historical_label}</p>
			{/if}
		</div>
	</div>

	{#if is_open}
		<div class="flex flex-wrap gap-2">
			{#each interruption.alternatives as alternative (`${alternative.engine_id}:${alternative.model_id}`)}
				<button
					type="button"
					class="inline-flex h-8 items-center rounded-lg bg-foreground px-3 text-sm font-medium text-background transition-colors hover:bg-foreground/90 focus-visible:ring-2 focus-visible:ring-ring/60 disabled:cursor-not-allowed disabled:opacity-50"
					disabled={!can_continue}
					onclick={yield* Resolve({
						type: "continue",
						target_engine_id: alternative.engine_id,
						target_model_id: alternative.model_id,
					})}
				>
					Switch to {alternative.display_name}
				</button>
			{/each}
			<button
				type="button"
				class="inline-flex h-8 items-center gap-1.5 rounded-lg border border-border/70 bg-background/40 px-3 text-sm font-medium text-foreground transition-colors hover:bg-background/70 focus-visible:ring-2 focus-visible:ring-ring/60 disabled:cursor-not-allowed disabled:opacity-50"
				disabled={!can_continue || interruption.auto_continue}
				onclick={yield* ResumeDefault()}
			>
				<Refresh class="size-3.5" aria-hidden="true" />
				{reset_countdown === undefined
					? `Continue with ${source_name} when available`
					: `Continue with ${source_name} in ${reset_countdown}`}
			</button>
		</div>

		<div class="flex items-center justify-between gap-3 rounded-xl bg-background/35 px-3 py-2">
			<div class="min-w-0">
				<p class="text-sm font-medium text-foreground">Continue this turn automatically</p>
				<p class="text-xs text-muted-foreground">
					{reset_countdown === undefined
						? "Artisan will check provider availability without starting a billable turn."
						: `Scheduled from the provider’s reset time: ${reset_countdown}.`}
				</p>
			</div>
			<Switch
				checked={interruption.auto_continue}
				disabled={submitting || onresolve === undefined}
				aria-label="Continue this turn automatically"
				onclick={yield* Resolve({ type: "set_auto_continue", enabled: !interruption.auto_continue })}
			/>
		</div>
	{/if}

	{#if action_message.length > 0}
		<p class="text-sm text-destructive" role="status">{action_message}</p>
	{/if}
</section>
