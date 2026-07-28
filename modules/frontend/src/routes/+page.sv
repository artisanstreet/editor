<script lang="ts" effect>
	import MessageCircle from "@tabler/icons-svelte/icons/message-circle";
	import Plus from "@tabler/icons-svelte/icons/plus";
	import { Clock, Effect, Stream } from "effect";
	import type { SurfaceUsageDailyBucket, ThreadListItem } from "@artisan/protocol";
	import { ArtisanClient, type ThreadListUpdate } from "@artisan/transport/client";
	import VerticalCalendarActivityGrid from "$lib/components/activity/vertical-calendar-activity-grid.sv";
	import { BannerService } from "$lib/banner/service";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import {
		ApplyRootThreadListUpdate,
		FormatRecentThreadTime,
		ThreadRoutePath,
	} from "$lib/root/thread-navigation";

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	const now_ms = yield* Clock.currentTimeMillis;
	/** The landing table is a convenience shortcut, not the primary thread navigator. */
	const recent_thread_limit = 4;
	/** One year of UTC days, matching the grid's densest readable layout. */
	const usage_day_count = 365;
	const day_in_ms = 86_400_000;
	let threads = $state.raw<ReadonlyArray<ThreadListItem>>([]);
	let usage = $state.raw<ReadonlyArray<SurfaceUsageDailyBucket>>([]);
	const recent_threads = $derived(threads.slice(0, recent_thread_limit));
	/** Before any usage exists, the grid keeps its full layout with zero-token days. */
	const empty_usage_days = (): ReadonlyArray<{ date: string; tokens: number }> =>
		Array.from({ length: usage_day_count }, (_, index) => ({
			date: new Date(now_ms - (usage_day_count - 1 - index) * day_in_ms)
				.toISOString()
				.slice(0, 10),
			tokens: 0,
		}));
	const activities = $derived(
		usage.length === 0
			? empty_usage_days()
			: usage.map((bucket) => ({
					date: bucket.date,
					tokens: bucket.input_tokens + bucket.output_tokens,
				})),
	);

	const ApplyUpdate = (update: ThreadListUpdate) =>
		Effect.sync(() => {
			threads = ApplyRootThreadListUpdate(threads, update);
		});

	const ApplyFailure = (error: { readonly message: string }) =>
		Effect.gen(function* () {
			threads = [];
			yield* banner.error("Could not load threads", { description: error.message });
		});

	yield* client.ListThreads.pipe(
		Effect.map((next_threads) => ({ journal_sequence: 0, threads: next_threads, type: "snapshot" as const })),
		Effect.flatMap(ApplyUpdate),
		Effect.catch(ApplyFailure),
	);

	yield* RunAuthoritativeSubscription(
		client.SubscribeThreadList,
		ApplyUpdate,
		client.ListThreads.pipe(
			Effect.map((next_threads) => ({
				journal_sequence: 0,
				threads: next_threads,
				type: "snapshot" as const,
			})),
			Effect.flatMap(ApplyUpdate),
		),
	).pipe(
		Effect.catch(ApplyFailure),
		Effect.forkScoped,
	);

	yield* client.GetSurfaceUsageDaily({ day_count: usage_day_count }).pipe(
		Effect.tap((snapshot) =>
			Effect.sync(() => {
				usage = snapshot.buckets;
			}),
		),
		Effect.catch((error) =>
			banner.error("Could not load token usage", { description: error.message }),
		),
	);

</script>

<svelte:head><title>Artisan Editor</title></svelte:head>

<main class="flex h-full min-h-0 items-center justify-center overflow-hidden p-6 lg:p-10">
	<div class="w-full max-w-[800px]">
		<section class="mb-8 min-w-0" aria-label="Token usage">
			<div class="flex h-24 w-full">
				<VerticalCalendarActivityGrid {activities} />
			</div>
		</section>
		<div class="min-w-0">
				<table class="w-full border-collapse text-left" aria-label="Recent threads">
						<thead class="sr-only">
							<tr><th>Thread</th><th>Last used</th></tr>
						</thead>
						<tbody>
						{#each recent_threads as thread (thread.thread_id)}
								<tr class="group border-b border-border last:border-b-0">
									<td class="p-0">
										<a
											href={ThreadRoutePath(thread.thread_id)}
											class="flex items-center gap-2 py-3 font-medium text-foreground outline-none transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none"
										>
											<MessageCircle
												class="size-4 shrink-0 text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none"
											/>
											<span class="truncate">{thread.title}</span>
										</a>
									</td>
									<td
										class="w-28 p-0 text-right text-xs text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none"
									>
										<span class="whitespace-nowrap">{FormatRecentThreadTime(thread.last_activity_at, now_ms)}</span>
									</td>
								</tr>
							{/each}
							{#if threads.length === 0}
								<tr class="group border-b border-border last:border-b-0">
									<td colspan="2" class="p-0">
										<a
											href="/threads/new"
											class="flex w-full items-center gap-2 py-3 font-medium text-foreground outline-none transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none"
										>
											<Plus
												class="size-4 shrink-0 text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none"
											/>
											<span class="truncate">New thread</span>
										</a>
									</td>
								</tr>
							{/if}
						</tbody>
					</table>
		</div>
	</div>
</main>
