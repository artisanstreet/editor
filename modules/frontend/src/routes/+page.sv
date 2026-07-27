<script lang="ts" effect>
	import MessageCircle from "@tabler/icons-svelte/icons/message-circle";
	import { Clock, Effect, Stream } from "effect";
	import type { ThreadListItem } from "@artisan/protocol";
	import { ArtisanClient, type ThreadListUpdate } from "@artisan/transport/client";
	import { BannerService } from "$lib/banner/service";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import {
		ApplyRootThreadListUpdate,
		FormatRecentThreadTime,
	} from "$lib/root/thread-navigation";

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	const now_ms = yield* Clock.currentTimeMillis;
	let threads = $state.raw<ReadonlyArray<ThreadListItem>>([]);

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

</script>

<svelte:head><title>Artisan Editor</title></svelte:head>

<main class="flex h-full min-h-0 items-center justify-center overflow-hidden p-6 lg:p-10">
	<div class="w-full max-w-[800px]">
		<div class="min-w-0">
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
										No threads yet. Create one from the sidebar.
									</td>
								</tr>
							{/if}
						</tbody>
					</table>
		</div>
	</div>
</main>
