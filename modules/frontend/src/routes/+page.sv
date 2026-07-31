<script lang="ts" effect>
	import MessageCircle from "@tabler/icons-svelte/icons/message-circle";
	import Plus from "@tabler/icons-svelte/icons/plus";
	import { Clock, Effect, Stream } from "effect";
	import type { SurfaceUsageDailyBucket, ThreadListItem } from "@artisan/protocol";
	import { ArtisanClient, type ThreadListUpdate } from "@artisan/transport/client";
	import VerticalCalendarActivityGrid, {
		type CalendarActivity,
	} from "$lib/components/activity/vertical-calendar-activity-grid.sv";
	import { BannerService } from "$lib/banner/service";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import {
		ApplyRootThreadListUpdate,
		FormatRecentThreadTime,
		ThreadRoutePathFor,
	} from "$lib/root/thread-navigation";

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	const now_ms = yield* Clock.currentTimeMillis;
	/** One year of UTC days, matching the grid's densest readable layout. */
	const usage_day_count = 365;
	const day_in_ms = 86_400_000;
	let threads = $state.raw<ReadonlyArray<ThreadListItem>>([]);
	let usage = $state.raw<ReadonlyArray<SurfaceUsageDailyBucket>>([]);
	/** Before any usage exists, the grid keeps its full layout with zero-token days. */
	const empty_usage_days = (): ReadonlyArray<CalendarActivity> =>
		Array.from({ length: usage_day_count }, (_, index) => ({
			date: new Date(now_ms - (usage_day_count - 1 - index) * day_in_ms)
				.toISOString()
				.slice(0, 10),
			engines: [],
			tokens: 0,
		}));
	const activities = $derived(
		usage.length === 0
			? empty_usage_days()
			: usage.map(
					(bucket): CalendarActivity => ({
						date: bucket.date,
						engines: bucket.engines.map((slice) => ({
							...(slice.engine_id === undefined ? {} : { engine_id: slice.engine_id }),
							...(slice.model_id === undefined ? {} : { model_id: slice.model_id }),
							tokens: slice.input_tokens + slice.output_tokens,
						})),
						tokens: bucket.input_tokens + bucket.output_tokens,
					}),
				),
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
			<!--
				Every thread is reachable here, but the list holds its four-row
				footprint: one row is 3rem of link plus a 1px divider, so four rows
				with three inner dividers cap the viewport. The scroll-driven fade
				masks each edge only while rows are actually hidden past it, and the
				thin native scrollbar takes its own gutter — the model picker's
				technique — so it never overlays the timestamps.
			-->
			<div class="thread-scroll docs-scroll-fade max-h-[calc(12rem+3px)] min-w-0 overflow-y-auto">
				<div class="mr-1">
				<table class="w-full table-fixed border-collapse text-left" aria-label="Recent threads">
						<thead class="sr-only">
							<tr><th>Thread</th><th>Last used</th></tr>
						</thead>
						<tbody>
						{#each threads as thread (thread.thread_id)}
								<tr class="group border-b border-border last:border-b-0">
									<td class="min-w-0 p-0">
										<a
											href={ThreadRoutePathFor(thread)}
											class="flex min-w-0 items-center gap-2 py-3 font-medium text-foreground outline-none transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none"
										>
											<MessageCircle
												class="size-4 shrink-0 text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none"
											/>
											<span class="min-w-0 truncate">{thread.title}</span>
										</a>
									</td>
									<td
										class="w-28 p-0 pl-4 text-right text-xs text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none"
									>
										<span class="whitespace-nowrap">{FormatRecentThreadTime(thread.last_activity_at, now_ms)}</span>
									</td>
								</tr>
							{/each}
							{#if threads.length === 0}
								<tr class="group border-b border-border last:border-b-0">
									<td colspan="2" class="p-0">
										<a
											href="/threads"
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
		</div>
	</div>
</main>

<style>
	/** The model picker's scrollbar: thin, muted, and holding its own gutter. */
	.thread-scroll {
		scrollbar-width: thin;
		scrollbar-color: var(--surface-500) transparent;
	}
</style>
