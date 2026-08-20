<script lang="ts" effect>
	import { Effect, Stream } from "effect";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { RuntimeSurfaceFor } from "$lib/browser/runtime-surface";
	import { Button } from "$lib/components/ui/button";
	import { Switch } from "$lib/components/ui/switch";
	import { Tooltip, TooltipContent, TooltipTrigger } from "$lib/components/ui/tooltip";
	import {
		SystemNotificationGapFor,
		SystemNotifications,
		type SystemNotificationSettings,
	} from "$lib/notifications/service";
	import Card from "./card.svelte";
	import Header from "./header.svelte";
	import Row from "./row.svelte";
	import Section from "./section.svelte";

	const notifications = yield* SystemNotifications;
	/**
	 * Permission is host state, and the host is where it changes: a reader who
	 * has just allowed Artisan in browser site settings or macOS notification
	 * settings arrives here expecting the screen to already know. Re-reading on
	 * arrival is what lets the notice below resolve itself rather than needing
	 * a reload to clear.
	 */
	let settings = $state.raw<SystemNotificationSettings>(yield* notifications.Current);
	const ApplySettings = (next: SystemNotificationSettings) =>
		Effect.gen(function* () {
			settings = next;
		});
	yield* notifications.Changes.pipe(Stream.runForEach(ApplySettings), Effect.forkScoped);
	yield* notifications.Refresh.pipe(Effect.forkScoped);

	/**
	 * Which shell is asking. The bundled desktop window is granted notifications
	 * by its own installation, so a refusal there points at operating-system
	 * settings; a browser tab has to ask, and a refusal there points at site
	 * permissions. Same feature, two different places to go and undo it.
	 */
	const desktop = yield* RunBrowserDom(
		() => RuntimeSurfaceFor(globalThis.navigator?.userAgent ?? "") === "desktop",
	);
	const gap = $derived(SystemNotificationGapFor(settings));

	const SetEnabled = (enabled: boolean) =>
		Effect.gen(function* () {
			settings = enabled ? yield* notifications.Enable : yield* notifications.Disable;
		});

	/**
	 * Asks again and re-reads the answer. After a dismissed prompt this really
	 * does ask; after a refusal the host answers instantly with the same
	 * refusal and shows nothing, which is why the notice beside it is what
	 * says where the refusal has to be undone.
	 */
	const RequestAgain = Effect.gen(function* () {
		settings = yield* notifications.Enable;
	});

	const toggle_description = desktop
		? "Posts a system notification when a thread finishes, fails, or needs an answer from you. Artisan appears in your operating system's notification settings, where you can silence or restyle it."
		: "Posts a notification when a thread finishes, fails, or needs an answer from you. Your browser asks for permission the first time you turn this on.";

	const gap_notice = $derived(
		gap === "blocked"
			? {
					description: desktop
						? "Artisan asked and your system refused. Allow notifications for Artisan in your operating system's notification settings, then check again."
						: "Artisan asked and your browser refused. A page cannot prompt its way past a block, so allow notifications for this site in your browser's site settings, then check again.",
					title: `Blocked by your ${desktop ? "system" : "browser"}`,
				}
			: gap === "unprompted"
				? {
						description:
							"The permission prompt was closed without an answer, so nothing can be posted yet. Asking again is safe.",
						title: "Not allowed yet",
					}
				: undefined,
	);
</script>

<Header title="Notifications" description="When Artisan is allowed to interrupt you." />

<Section id="system" title="System">
	<Card class="mt-3">
		<Row title="Notify me" description={toggle_description}>
			{#snippet control()}
				{#if gap === "unsupported"}
					<Tooltip>
						<TooltipTrigger>
							{#snippet child({ props: tooltip_props })}
								<span {...tooltip_props} class="flex">
									<Switch checked={false} disabled aria-label="Notify me" />
								</span>
							{/snippet}
						</TooltipTrigger>
						<TooltipContent side="left" class="max-w-60">
							This browser exposes no notification API, so there is nothing for
							Artisan to post to.
						</TooltipContent>
					</Tooltip>
				{:else}
					<Switch
						checked={settings.enabled}
						aria-label="Notify me"
						onclick={yield* SetEnabled(!settings.enabled)}
					/>
				{/if}
			{/snippet}
		</Row>

		{#if gap_notice !== undefined}
			<!--
				Shown in place rather than as a toast, and the switch stays on:
				the reader did ask for this, and the only thing between that and
				a notification is a host setting that is not ours to change.
			-->
			<div class="flex items-start justify-between gap-6 px-4 py-3.5">
				<div class="flex min-w-0 flex-col gap-0.5">
					<span class="text-sm text-destructive">{gap_notice.title}</span>
					<span class="text-pretty text-xs leading-relaxed text-muted-foreground">
						{gap_notice.description}
					</span>
				</div>
				<Button variant="outline" size="sm" onclick={yield* RequestAgain}>
					Check again
				</Button>
			</div>
		{/if}

		<Row
			title="Clears itself"
			description="A notification you don't answer disappears after a few seconds. Nothing is lost by letting it go — an approval or a question keeps waiting in its thread — so a notification can never pile up into something you have to go and dismiss."
		/>
	</Card>
</Section>
