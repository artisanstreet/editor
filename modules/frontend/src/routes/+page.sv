<script lang="ts" effect>
	import MessageCircle from "@tabler/icons-svelte/icons/message-circle";
	import { Clock, Effect, Stream } from "effect";
	import type { ThreadListItem } from "@artisan/protocol";
	import { ArtisanClient, type ThreadListUpdate } from "@artisan/transport/client";
	import VerticalCalendarActivityGrid from "$lib/components/activity/vertical-calendar-activity-grid.sv";
	import {
		ApplyRootThreadListUpdate,
		FormatRecentThreadTime,
	} from "$lib/root/thread-navigation";

	const client = yield* ArtisanClient;
	const now_ms = yield* Clock.currentTimeMillis;
	let threads = $state.raw<ReadonlyArray<ThreadListItem>>([]);
	let thread_list_error = $state<string | undefined>();

	const date_key = (date: Date) => {
		const year = date.getFullYear();
		const month = String(date.getMonth() + 1).padStart(2, "0");
		const day = String(date.getDate()).padStart(2, "0");
		return `${year}-${month}-${day}`;
	};

	const activity = Array.from({ length: 365 }, (_, index) => {
		const date = new Date(now_ms);
		date.setHours(12, 0, 0, 0);
		date.setDate(date.getDate() - (364 - index));
		const active = index > 238 && (index * 17 + date.getDay() * 11) % 9 > 1;
		const wave = Math.sin(index / 8) * 0.35 + 0.65;
		return {
			date: date_key(date),
			tokens: active ? Math.round((18_000 + ((index * 7_919) % 170_000)) * wave) : 0,
		};
	});

	const ApplyUpdate = (update: ThreadListUpdate) =>
		Effect.sync(() => {
			threads = ApplyRootThreadListUpdate(threads, update);
			thread_list_error = undefined;
		});

	const ApplyFailure = (error: { readonly message: string }) =>
		Effect.sync(() => {
			thread_list_error = error.message;
			threads = [];
		});

	yield* client.ListThreads.pipe(
		Effect.map((next_threads) => ({ journal_sequence: 0, threads: next_threads, type: "snapshot" as const })),
		Effect.flatMap(ApplyUpdate),
		Effect.catch(ApplyFailure),
	);

	yield* client.SubscribeThreadList.pipe(
		Effect.flatMap((updates) => Stream.runForEach(updates, ApplyUpdate)),
		Effect.catch(ApplyFailure),
		Effect.forkScoped,
	);

</script>

<svelte:head><title>Artisan Editor</title></svelte:head>

<main class="flex h-full min-h-0 items-center justify-center overflow-hidden p-6 lg:p-10">
	<div class="w-full max-w-[800px]">
		<div class="flex flex-row items-stretch gap-12">
			<div class="relative w-32 shrink-0">
				<section class="absolute inset-x-0 inset-y-2 flex min-h-0 flex-col" aria-label="Tokens used">
					<VerticalCalendarActivityGrid activities={activity} />
				</section>
			</div>

			<div class="min-w-0 grow">
				<table class="w-full border-collapse text-left" aria-label="Recent threads">
						<thead class="sr-only">
							<tr><th>Thread</th><th>Last used</th></tr>
						</thead>
						<tbody>
						{#each threads as thread (thread.thread_id)}
								<tr class="group border-b border-border last:border-b-0">
									<td class="p-0">
										<a
											href={`/threads/${thread.thread_id}`}
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
								<tr>
									<td colspan="2" class="py-3 text-sm text-muted-foreground">
										{thread_list_error ?? "No threads yet. Create one from the sidebar."}
									</td>
								</tr>
							{/if}
						</tbody>
					</table>
			</div>
		</div>
	</div>
</main>
