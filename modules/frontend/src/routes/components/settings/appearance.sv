<script lang="ts" effect>
	import { Effect } from "effect";
	import { shader_enabled } from "$lib/appearance-config";
	import { Switch } from "$lib/components/ui/switch";
	import { AppearancePreferences } from "$lib/runtime/appearance-preferences";
	import Row from "./row.sv";

	const preferences = yield* AppearancePreferences;
	const stored = yield* preferences.Load;

	shader_enabled.set(stored.shader_enabled);
	let enabled = $state(stored.shader_enabled);

	/**
	 * The store moves first so every glass surface answers the switch in the same
	 * frame; the durable write follows and is never what the reader waits on.
	 */
	const ToggleShader = (next: boolean) =>
		Effect.gen(function* () {
			enabled = next;
			shader_enabled.set(next);
			yield* preferences.Save({ ...stored, shader_enabled: next });
		});
</script>

<h1 class="text-lg font-semibold text-foreground">Appearance</h1>
<p class="mt-1 text-sm text-muted-foreground">How Artisan's surfaces are drawn.</p>

<section class="mt-10" aria-labelledby="glass">
	<h2 id="glass" class="scroll-mt-6 text-sm font-medium text-foreground">Glass</h2>
	<div
		class="card mt-3 rounded-xl bg-linear-to-b from-surface-225 to-surface-200 dark:from-surface-800 dark:to-surface-925"
	>
		<div class="flex flex-col divide-y divide-border/40">
			<Row
				title="Shader under glass"
				description="Lights glass surfaces with the animated shader they were designed around. Turning it off leaves the glass itself intact — the material, highlight, and depth stay — and only the moving light stops."
			>
				{#snippet control()}
					<Switch
						checked={enabled}
						aria-label="Shader under glass"
						onclick={yield* ToggleShader(!enabled)}
					/>
				{/snippet}
			</Row>
		</div>
	</div>
</section>
