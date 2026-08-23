<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import type { TelemetryPreferences } from "@artisan/protocol";
	import { Switch } from "$lib/components/ui/switch";
	import { TelemetryController } from "$lib/settings/telemetry-controller";
	import Card from "./card.svelte";
	import Header from "./header.svelte";
	import Row from "./row.svelte";
	import Section from "./section.svelte";

	const telemetry = yield* TelemetryController;
	let preferences = $state.raw<TelemetryPreferences>(yield* telemetry.Current);
	let crash_restart_required = $state(false);
	const ApplyPreferences = (next: TelemetryPreferences) =>
		Effect.gen(function* () {
			preferences = next;
		});
	yield* telemetry.Changes.pipe(Stream.runForEach(ApplyPreferences), Effect.forkScoped);
	yield* telemetry.Refresh.pipe(Effect.forkScoped);

	const SetUsageAnalytics = (enabled: boolean) =>
		Effect.gen(function* () {
			preferences = yield* telemetry.SetUsageAnalytics(enabled ? "enabled" : "disabled");
		});
	const SetCrashReports = (enabled: boolean) =>
		Effect.gen(function* () {
			preferences = yield* telemetry.SetCrashReports(enabled ? "enabled" : "disabled");
			crash_restart_required = true;
		});
</script>

<Header
	title="Privacy"
	description="Two independent choices for anonymous product analytics and sanitized crash reports."
/>

<Section id="telemetry" title="Observability">
	<Card class="mt-3">
		<Row
			title="Usage analytics"
			description="Sends allowlisted product events to PostHog using a random installation ID. Artisan never sends prompts, responses, source code, diffs, terminal activity, names, or paths."
		>
			{#snippet control()}
				<div class="flex items-center gap-2">
					<span class="text-xs text-muted-foreground">
						{preferences.usage_analytics === "unset"
							? "Not decided"
							: preferences.usage_analytics === "enabled"
								? "On"
								: "Off"}
					</span>
					<Switch
						checked={preferences.usage_analytics === "enabled"}
						aria-label="Usage analytics"
						onclick={yield* SetUsageAnalytics(preferences.usage_analytics !== "enabled")}
					/>
				</div>
			{/snippet}
		</Row>

		<div class="border-t border-border/60"></div>

		<Row
			title="Crash reports"
			description="Sends sanitized exceptions, crash reasons, release information, and coarse performance diagnostics to Sentry. Attachments, replay, console logs, request data, environment variables, and process arguments stay off."
		>
			{#snippet control()}
				<div class="flex items-center gap-2">
					<span class="text-xs text-muted-foreground">
						{preferences.crash_reports === "unset"
							? "Not decided"
							: preferences.crash_reports === "enabled"
								? "On"
								: "Off"}
					</span>
					<Switch
						checked={preferences.crash_reports === "enabled"}
						aria-label="Crash reports"
						onclick={yield* SetCrashReports(preferences.crash_reports !== "enabled")}
					/>
				</div>
			{/snippet}
		</Row>

		{#if crash_restart_required}
			<div class="border-t border-border/60 px-4 py-3 text-xs leading-relaxed text-muted-foreground">
				Restart Artisan to apply crash-reporting changes to Electron's native crash handler.
			</div>
		{/if}
	</Card>
</Section>

<Section id="never-collected" title="Never collected">
	<Card class="mt-3">
		<Row
			title="Your work stays local"
			description="Prompts, model responses, source code, file contents, diffs, terminal commands and output, repository and project names, paths, credentials, headers, request bodies, process arguments, and environment variables are prohibited from both systems."
		/>
	</Card>
</Section>
