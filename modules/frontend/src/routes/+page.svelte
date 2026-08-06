<script lang="ts" effect>
	import { Clock, Effect, Stream } from "effect";
	import type {
		SurfaceUsageDailyBucket,
		ThreadListItem,
		ThreadSessionPolicy,
	} from "@artisan/protocol";
	import { ArtisanClient, type ThreadListUpdate } from "@artisan/transport/client";
	import VerticalCalendarActivityGrid, {
		type CalendarActivity,
	} from "$lib/components/activity/vertical-calendar-activity-grid.svelte";
	import { BannerService } from "$lib/banner/service";
	import { EngineMarkClass, UsageSlicePresentationFor } from "$lib/engine/presentation";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import { RunAuthoritativeSubscription } from "$lib/conversation/subscription";
	import {
		permission_for_harness,
		policy_fields_for_permission,
	} from "$lib/engine/model-selection";
	import {
		DraftThreadController,
		type DraftThreadState,
	} from "$lib/root/draft-thread";
	import {
		ApplyRootThreadListUpdate,
		FormatRecentThreadTime,
		ThreadRoutePath,
		ThreadRoutePathFor,
	} from "$lib/root/thread-navigation";
	import { SessionDefaultsController } from "$lib/settings/session-defaults-controller";
	import ThreadComposer from "./components/thread-composer.svelte";

	const client = yield* ArtisanClient;
	const banner = yield* BannerService;
	const navigation = yield* RouteNavigation;
	const draft_thread = yield* DraftThreadController;
	const session_defaults = yield* SessionDefaultsController;
	const now_ms = yield* Clock.currentTimeMillis;
	/** One year of UTC days, matching the grid's densest readable layout. */
	const usage_day_count = 365;
	const day_in_ms = 86_400_000;
	let threads = $state.raw<ReadonlyArray<ThreadListItem>>([]);
	let usage = $state.raw<ReadonlyArray<SurfaceUsageDailyBucket>>([]);
	let draft_state = $state.raw<DraftThreadState>(yield* draft_thread.Current);
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
		Effect.gen(function* () {
			threads = ApplyRootThreadListUpdate(threads, update);
		});

	const ApplyFailure = (error: { readonly message: string }) =>
		Effect.gen(function* () {
			threads = [];
			yield* banner.error("Could not load threads", { description: error.message });
		});

	const RefreshThreads = Effect.gen(function* () {
		const next_threads = yield* client.ListThreads;
		yield* ApplyUpdate({
			journal_sequence: 0,
			threads: next_threads,
			type: "snapshot",
		});
	}).pipe(Effect.catch(ApplyFailure));

	yield* RefreshThreads;

	yield* RunAuthoritativeSubscription(
		client.SubscribeThreadList,
		ApplyUpdate,
		RefreshThreads,
	).pipe(
		Effect.catch(ApplyFailure),
		Effect.forkScoped,
	);

	const LoadUsage = Effect.gen(function* () {
		const snapshot = yield* client.GetSurfaceUsageDaily({ day_count: usage_day_count });
		usage = snapshot.buckets;
	}).pipe(
		Effect.catch((error) =>
			Effect.gen(function* () {
				yield* banner.error("Could not load token usage", { description: error.message });
			}),
		),
	);
	yield* LoadUsage;

	const ApplyDraftState = (next: DraftThreadState) =>
		Effect.gen(function* () {
			draft_state = next;
		});
	yield* draft_thread.Changes.pipe(
		Stream.runForEach(ApplyDraftState),
		Effect.forkScoped,
	);

	/**
	 * Every draft starts from the most recently used project; the panel can
	 * change it. A disconnected client simply has no projects to offer, which
	 * is the same state as a fresh install and already blocks the first send.
	 */
	const LoadInitialProject = Effect.gen(function* () {
		const catalog = yield* client.ListProjects;
		return catalog.projects[0];
	}).pipe(
		Effect.catch(() =>
			Effect.gen(function* () {
				return undefined;
			}),
		),
	);

	/**
	 * The draft is the only place the engine can be chosen: it locks once the
	 * first message creates the session. A new draft inherits the model the
	 * user chose last time (with that model's own default effort and speed, as
	 * if re-picked); with no usable history it mirrors the backend's default
	 * session policy with the runtime catalog's default model engine.
	 *
	 * This page remounts on every navigation back to the root (e.g. the user
	 * picks a model, opens a thread, then returns). Seeding unconditionally
	 * would wipe whatever the user already shaped in the draft. Seed the
	 * default only when no draft policy exists yet; a draft the user has
	 * touched survives remounts until the first message creates the durable
	 * thread.
	 */
	const LoadInitialPolicy = Effect.gen(function* () {
		/**
		 * Defaults live on Forge, so a second client or a cleared browser store
		 * still opens on the same model and permission. An unreachable Forge only
		 * means falling back to the catalog default.
		 */
		const snapshot = yield* session_defaults.Refresh.pipe(
			Effect.catch(() =>
				Effect.gen(function* () {
					return yield* session_defaults.Current;
				}),
			),
		);
		const runtime_catalog = snapshot.catalog;
		const defaults = snapshot.defaults;
		const last_model_id = defaults.last_model_id;
		const last_model = runtime_catalog.manifest.models.find(
			(model) =>
				model.native_model_id === last_model_id && model.disabled === undefined,
		);
		const default_model = runtime_catalog.manifest.models.find(
			(model) => model.id === runtime_catalog.default_model_id,
		);
		const seed_model = last_model ?? default_model;
		const seed_thinking = seed_model?.capabilities.thinking;
		/** Effort and context are per model: only the chosen model's row applies. */
		const model_defaults = defaults.models.find(
			(entry) => entry.model_id === seed_model?.id,
		);
		/** A saved option from another harness falls back to this harness's default. */
		const seed_permission = permission_for_harness(
			runtime_catalog,
			seed_model?.harness ?? "codex",
			defaults.permission,
		);
		return {
			engine_id: seed_model?.harness ?? "codex",
			...(model_defaults?.context_window === undefined
				? {}
				: { context_window: model_defaults.context_window }),
			model: seed_model?.native_model_id,
			...policy_fields_for_permission(seed_permission),
			reasoning_effort:
				model_defaults?.reasoning_effort ??
				(seed_thinking?.availability === "supported"
					? seed_thinking.default === "light"
						? "low"
						: seed_thinking.default
					: "medium"),
			service_tier:
				seed_model?.capabilities.speed_options.find(
					(option) => option.default && option.disabled === undefined,
				)?.native_value ?? "standard",
			strict_clarification: false,
			web_search_enabled: false,
		} satisfies ThreadSessionPolicy;
	});

	if (draft_state._tag !== "Created") {
		const initial_project =
			draft_state._tag === "Ready" && draft_state.project !== undefined
				? draft_state.project
				: yield* LoadInitialProject;
		const initial_policy =
			draft_state._tag === "Ready" && draft_state.policy !== undefined
				? draft_state.policy
				: yield* LoadInitialPolicy;
		yield* draft_thread.Initialize(initial_project, initial_policy);
	}

	const Navigate = (path: string) =>
		Effect.gen(function* () {
			yield* navigation.Navigate(path);
		});

	/**
	 * The durable thread materializes only here, at the first send: it is
	 * created with the draft's project, receives the draft's session policy,
	 * and the submission is handed to the routed thread, which owns the
	 * durable message pipeline.
	 */
	const SubmitFirstMessage = (submission: ComposerSubmission) =>
		Effect.gen(function* () {
			const created = yield* draft_thread.Submit(submission);
			yield* Navigate(ThreadRoutePath(created.project.project_id, created.thread_id));
		});

	/**
	 * The picker persists a policy and waits to be told what was actually applied,
	 * then remembers that as the session default. A draft has no server to answer
	 * with authority, so what it stored is the answer — returning nothing put an
	 * `undefined` into the confirmed set and the defaults write read straight
	 * through it.
	 */
	const UpdateDraftPolicy = (policy: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			yield* draft_thread.UpdatePolicy(policy);
			return policy;
		});

	const RetryDraftNavigation = Effect.gen(function* () {
		const created = yield* draft_thread.Retry;
		yield* Navigate(ThreadRoutePath(created.project.project_id, created.thread_id));
	});
</script>

<svelte:head><title>Artisan Editor</title></svelte:head>

<main class="relative h-full min-h-0 overflow-hidden">
	<div class="flex h-full min-h-0 items-center justify-center overflow-hidden p-6 pb-44 lg:p-10 lg:pb-48">
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
					<!--
						The inset sits on the whole list, not on the rows: the divider is
						the row's own bottom border, and a margin on a table row is
						ignored, so insetting rows alone would slide the timestamps left
						and leave the rules under the scrollbar.
					-->
					<div class="mr-3">
					<table class="w-full table-fixed border-collapse text-left" aria-label="Recent threads">
							<thead class="sr-only">
								<tr><th>Thread</th><th>Last used</th></tr>
							</thead>
							<tbody>
							{#each threads as thread (thread.thread_id)}
								{@const thread_mark = UsageSlicePresentationFor(thread.engine_id, thread.model_id).mark}
								{@const ThreadMark = thread_mark.icon}
									<tr class="group border-b border-border last:border-b-0">
										<td class="min-w-0 p-0">
											<a
												href={ThreadRoutePathFor(thread)}
												class="flex min-w-0 items-center gap-2 py-3 font-medium text-foreground outline-none transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none"
											>
												<!--
													A thread wears what it runs on. Until it has an engine its mark is the
													neutral glyph, which is honest: nothing has answered in it yet.
												-->
												<ThreadMark
													class={EngineMarkClass(thread_mark, "size-4 shrink-0 text-muted-foreground transition-colors duration-(--duration-fast) ease-in-out group-hover:text-foreground-extra group-focus-within:text-foreground-extra motion-reduce:transition-none")}
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
							</tbody>
						</table>
					</div>
				</div>
			</div>
		</div>
	</div>

	{#if draft_state._tag === "Created"}
		<div class="fixed inset-x-0 bottom-6 z-30 mx-auto flex w-fit items-center gap-3 rounded-lg border border-border bg-background px-4 py-3 shadow-lg" role="status">
			<span>Your new thread is ready. Its first message is retained until delivery succeeds.</span>
			<button type="button" onclick={yield* RetryDraftNavigation}>Open thread and retry</button>
		</div>
	{/if}

	<!-- The composer pins to the page bottom; its first send creates the thread and routes into it. -->
	<ThreadComposer
		disabled={draft_state._tag === "Created"}
		draft_key="draft:new-thread"
		engine_locked={draft_state._tag === "Created"}
		onpolicychange={UpdateDraftPolicy}
		onsubmit={SubmitFirstMessage}
		policy={draft_state._tag === "Uninitialized" ? undefined : draft_state.policy}
	/>
</main>

<style>
	/** The model picker's scrollbar: thin, muted, and holding its own gutter. */
	.thread-scroll {
		scrollbar-width: thin;
		scrollbar-color: var(--surface-500) transparent;
	}
</style>
