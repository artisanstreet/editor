<script lang="ts" effect>
	/**
	 * A new thread, which is what the application opens on.
	 *
	 * There is one question here and it is the message. Where the work happens is
	 * a word inside the sentence above the composer rather than a screen you get
	 * past first — the catalog is local, the answer is nearly always the project
	 * you had last, and making that a destination of its own charged a navigation
	 * for a decision that is usually already made.
	 *
	 * The same surface serves `/` and `/t/<workspace>`. The difference is only
	 * where the project comes from: the URL names it on the second, and changing
	 * the word there is a navigation because the URL would otherwise be lying.
	 * Nothing durable exists until the first send, which is what creates the
	 * thread and hands over to its own route.
	 */
	import { Effect, Stream } from "effect";
	import type { Project, ThreadSessionPolicy } from "@artisan/protocol";
	import { RouteNavigation } from "$lib/browser/route-navigation";
	import type { ComposerActionFailure } from "$lib/composer/action-failure";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import {
		NewThreadSentenceWords,
		ProjectMarker,
	} from "$lib/root/new-thread-sentence";
	import { PreferredProject, RecentProjects } from "$lib/root/project-catalog";
	import { SeededDraftPolicy } from "$lib/root/draft-policy";
	import { DraftThreadController, type DraftThreadState } from "$lib/root/draft-thread";
	import {
		WorkspaceCatalogController,
		type WorkspaceCatalogState,
	} from "$lib/root/workspace-catalog-controller";
	import {
		RetryNewThreadDraft,
		SubmitNewThreadDraft,
		new_thread_draft_key,
	} from "$lib/root/new-thread-draft";
	import { ThreadRoutePath, WorkspaceRoutePath } from "$lib/root/thread-navigation";
	import { SessionDefaultsController } from "$lib/settings/session-defaults-controller";
	import ProjectFolderPicker from "./project-folder-picker.svelte";
	import ProjectSelector from "./project-selector.svelte";
	import ComposerActionFailureView from "./composer/action-failure.svelte";
	import ThreadComposer from "./thread-composer.svelte";

	let {
		workspace_id,
	}: {
		/**
		 * The project named by the route, when one is. Absent on `/`, where the
		 * sentence itself is what names it.
		 */
		readonly workspace_id?: string;
	} = $props();

	const navigation = yield* RouteNavigation;
	const draft_thread = yield* DraftThreadController;
	const session_defaults = yield* SessionDefaultsController;
	const workspace_catalog = yield* WorkspaceCatalogController;
	const draft_key = $derived(new_thread_draft_key(workspace_id));

	/** The shell owns hydration; a new route reads its retained snapshot immediately. */
	let catalog_state = $state.raw<WorkspaceCatalogState>(yield* workspace_catalog.Current);
	const ApplyCatalogState = (next: WorkspaceCatalogState) =>
		Effect.gen(function* () {
			catalog_state = next;
		});
	yield* workspace_catalog.Changes.pipe(
		Stream.runForEach(ApplyCatalogState),
		Effect.forkScoped,
	);
	const projects = $derived(catalog_state.projects);
	const threads = $derived(catalog_state.threads);
	const catalog_read = $derived(catalog_state.projects_loaded);

	const recents = $derived(RecentProjects(projects, threads));

	/**
	 * Where the reader last was, read once rather than followed: it seeds the
	 * word on arrival, and a draft edited behind this surface has no business
	 * changing it afterwards.
	 */
	const opening_state = yield* draft_thread.Current;
	const opening_project_id =
		opening_state._tag === "Uninitialized" ? undefined : opening_state.project?.project_id;
	/**
	 * A retained created draft needs its recovery action on arrival. A draft
	 * created by this mount does not: it is still following the ordinary route
	 * handoff until navigation actually fails.
	 */
	let show_draft_recovery = $state(opening_state._tag === "Created");
	/** What the last draft action refused, reported over the composer it refused. */
	let draft_failure = $state.raw<ComposerActionFailure | undefined>(undefined);
	const DismissDraftFailure = Effect.gen(function* () {
		draft_failure = undefined;
	});
	/** The word, once the reader has changed it. */
	let chosen_project_id = $state<string | undefined>(undefined);
	/**
	 * A URL that names a workspace is resolved against the catalog rather than
	 * trusted — a segment naming a project Forge does not have is not a
	 * workspace, and this surface must not compose into a fiction.
	 */
	const routed_project = $derived(
		workspace_id === undefined
			? undefined
			: recents.find((recent) => recent.project.project_id === workspace_id)?.project,
	);
	const project = $derived(
		workspace_id === undefined
			? PreferredProject(recents, chosen_project_id ?? opening_project_id)
			: routed_project,
	);
	const detached = $derived(
		workspace_id !== undefined && catalog_read && routed_project === undefined,
	);

	let draft_state = $state.raw<DraftThreadState>(opening_state);
	let draft_revision = $state(yield* draft_thread.CurrentRevision);
	const ApplyDraftState = (next: DraftThreadState) =>
		Effect.gen(function* () {
			draft_state = next;
		});
	const ApplyDraftRevision = (next: number) =>
		Effect.gen(function* () {
			draft_revision = next;
		});
	yield* draft_thread.Changes.pipe(Stream.runForEach(ApplyDraftState), Effect.forkScoped);
	yield* draft_thread.RevisionChanges.pipe(
		Stream.runForEach(ApplyDraftRevision),
		Effect.forkScoped,
	);
	/** A first message already in flight owns the project it was created with. */
	const locked = $derived(draft_state._tag === "Created");

	/**
	 * Defaults are hydrated by the persistent shell and retained in the app-scoped
	 * controller, so a new route can open with the same model and permission
	 * without paying another Forge round trip. Before that hydration completes,
	 * the controller's offline snapshot is the honest fallback.
	 */
	const LoadSeedPolicy = Effect.gen(function* () {
		const snapshot = yield* session_defaults.Current;
		return SeededDraftPolicy(snapshot.catalog, snapshot.defaults);
	});

	/**
	 * Pointing the draft at the word the sentence currently says.
	 *
	 * Read from the controller rather than from the reactive mirror above: the
	 * mirror is written by this very block, and a dependency on it would make
	 * aligning the draft re-align it forever. The seed policy is only fetched
	 * when the draft has none, so changing the word costs nothing.
	 */
	/**
	 * Whether the controller is actually holding the project this surface offers
	 * to send into.
	 *
	 * The sentence naming a project is not the same fact as the draft carrying
	 * one: the first is read from the catalog here, the second is written into
	 * the controller by the alignment below, and only the second is what a send
	 * is checked against. Reading the refusal is what keeps them from disagreeing
	 * — a discarded `false` armed the composer over a draft that would reject it,
	 * and the rejection had nowhere to surface.
	 */
	let draft_aligned = $state(false);

	const AlignDraft = (target: Project | undefined, _mirrored_revision: number) =>
		Effect.gen(function* () {
			if (target === undefined) {
				draft_aligned = false;
				return;
			}
			const current = yield* draft_thread.Current;
			/** A retained submission already owns its project; there is nothing to align. */
			if (current._tag === "Created") {
				draft_aligned = true;
				return;
			}
			/**
			 * The mirrored revision says *when* to align, not what to align against.
			 * It arrives through a forked stream, so it trails the controller by a
			 * scheduler turn, and passing it through refused every alignment in the
			 * gap between a reset and the mirror catching up — leaving the draft
			 * with no project while the sentence above still named one.
			 *
			 * Read here, before the seed policy below, the revision is this
			 * attempt's own starting point. A reset landing while that policy loads
			 * still moves it, and the controller still rejects the write, which is
			 * the stale-alignment race the guard is there for.
			 */
			const expected_revision = yield* draft_thread.CurrentRevision;
			const policy =
				current._tag === "Ready" && current.policy !== undefined
					? current.policy
					: yield* LoadSeedPolicy;
			/** The controller checks and writes under the same lock used by an explicit reset. */
			draft_aligned = yield* draft_thread.AlignAtRevision(expected_revision, target, policy);
		});
	/** The mirrored revision is read purely so a reset re-runs the alignment. */
	yield* AlignDraft(project, draft_revision);
	/** The one fact a send is actually gated on: a project, held by the draft. */
	const draft_ready = $derived(project !== undefined && draft_aligned);

	/**
	 * A navigation that cannot be performed is reported rather than dropped.
	 * The empty branch this replaces is why moving between projects could do
	 * nothing at all and say nothing about it.
	 */
	const Navigate = (path: string) =>
		Effect.gen(function* () {
			yield* navigation.Navigate(path).pipe(
				Effect.catch((error) =>
					Effect.gen(function* () {
						draft_failure = {
							description: error.message,
							title: "Could not open that project",
						};
					}),
				),
			);
		});

	/**
	 * The created draft is already retained before this navigation begins. Keep
	 * its recovery action hidden during the normal handoff and reveal it only if
	 * the route cannot be opened.
	 */
	const NavigateCreatedDraft = (path: string) =>
		Effect.gen(function* () {
			show_draft_recovery = false;
			yield* navigation.Navigate(path).pipe(
				Effect.catch(() =>
					Effect.gen(function* () {
						show_draft_recovery = true;
					}),
				),
			);
		});

	/**
	 * Changing the word. On `/` that is all it is; on a routed workspace the URL
	 * names the project, so the same gesture has to move the URL with it.
	 */
	const SelectProject = (next: Project) =>
		Effect.gen(function* () {
			if (workspace_id === undefined) {
				chosen_project_id = next.project_id;
				return;
			}
			yield* Navigate(WorkspaceRoutePath(next.project_id));
		});

	let attaching_project = $state(false);
	const OpenFolderPicker = Effect.gen(function* () {
		attaching_project = true;
	});
	/** A folder that has just become a project is the one you meant to work in. */
	const AdoptAttachedProject = (attached: Project) =>
		Effect.gen(function* () {
			yield* workspace_catalog.ApplyProjectCatalogUpdate({
				snapshot: {
					projects: [
						...catalog_state.projects.filter(
							(project) => project.project_id !== attached.project_id,
						),
						attached,
					],
				},
				type: "replacement",
			});
			yield* SelectProject(attached);
		});

	/**
	 * The durable thread materializes only here, at the first send: it is created
	 * with the sentence's project, receives the draft's session policy, and the
	 * submission is handed to the routed thread, which owns the durable message
	 * pipeline.
	 */
	const SubmitFirstMessage = (submission: ComposerSubmission) =>
		Effect.gen(function* () {
			const created = yield* SubmitNewThreadDraft(draft_key, submission);
			yield* NavigateCreatedDraft(
				ThreadRoutePath(created.project.project_id, created.thread_id),
			);
		});

	/**
	 * The picker persists a policy and waits to be told what was actually applied,
	 * then remembers that as the session default. A draft has no server to answer
	 * with authority, so what it stored is the answer.
	 */
	const UpdateDraftPolicy = (policy: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			yield* draft_thread.UpdatePolicy(policy);
			return policy;
		});

	const RetryDraftNavigation = Effect.gen(function* () {
		const created = yield* RetryNewThreadDraft(draft_key);
		yield* NavigateCreatedDraft(
			ThreadRoutePath(created.project.project_id, created.thread_id),
		);
	});

	/**
	 * One sentence per visit, chosen on arrival and then left alone: re-picking
	 * it when the project changes would replay the reveal for a decision that
	 * only touched one word of it.
	 */
	const sentence_words = NewThreadSentenceWords(`Create your first ${ProjectMarker} today.`);
</script>

<svelte:head>
	<title>{project === undefined ? "Artisan Editor" : `${project.display_name} › Artisan Editor`}</title>
</svelte:head>

<main class="relative h-full min-h-0 overflow-hidden" aria-label="New thread">
	<!--
		The opening sentence is an orientation point, not transcript prose. It
		centres in the actual conversation viewport even when the rail reserves a
		margin for navigation; inheriting the transcript column would shift this
		line away from the middle of the screen.
	-->
	<div class="prose-column-frame absolute inset-0 flex flex-col items-center justify-center">
		<div class="flex w-full max-w-(--prose-width) flex-col gap-3 text-center">
			{#if detached}
				<p class="text-lg text-foreground">This project is not attached.</p>
				<p class="max-w-sm text-sm text-muted-foreground">
					Forge owns the project catalog and has no project by this identity.
				</p>
				<a
					class="text-sm text-(--banner-info) underline-offset-2 hover:underline"
					href="/"
				>
					Start a thread somewhere else
				</a>
			{:else if !catalog_read}
				<p class="text-sm text-muted-foreground" role="status">Loading projects…</p>
			{:else}
				<!--
					The line arrives out of focus and sharpens word by word, which walks
					the eye along it to the one word that is underlined.
				-->
				<p class="text-2xl leading-snug text-pretty text-muted-foreground">
					{#each sentence_words as word, index (index)}
						{#if word.leading_space}{" "}{/if}
						<!--
							Whatever is glued to the project stays inside its span, so a full
							stop can neither drift a space away from the name nor wrap onto a
							line by itself.
						-->
						<span class="sentence-word" style={`--sentence-delay: ${word.delay_ms}ms`}>
							{#if word.project}{word.prefix}<ProjectSelector
									disabled={locked}
									onnewproject={OpenFolderPicker}
									onselect={SelectProject}
									{project}
									projects={recents}
								/>{word.suffix}{:else}{word.text}{/if}
						</span>
					{/each}
				</p>
			{/if}
		</div>
	</div>

	{#if locked && show_draft_recovery}
		<div
			class="fixed inset-x-0 bottom-6 z-30 mx-auto flex w-fit items-center gap-3 rounded-lg border border-border bg-background px-4 py-3 shadow-lg"
			role="status"
		>
			<span>Your new thread is ready. Its first message is retained until delivery succeeds.</span>
			<button type="button" onclick={yield* RetryDraftNavigation}>Open thread and retry</button>
		</div>
	{/if}

	<ComposerActionFailureView failure={draft_failure} ondismiss={DismissDraftFailure} />

	<!-- The composer pins to the page bottom; its first send creates the thread and routes into it. -->
	<!--
		The draft is keyed to the surface, not to the word.

		A routed workspace is one project for as long as it is mounted, so its
		draft is that project's. The root is not: changing the word there is a
		decision about a message already being written, and swapping the text out
		from under it — which a key that followed the word would do — would throw
		away the thing the reader is in the middle of.
	-->
	{#key draft_revision}
		<ThreadComposer
			disabled={!draft_ready || locked}
			{draft_key}
			engine_locked={locked}
			onpolicychange={UpdateDraftPolicy}
			onsubmit={SubmitFirstMessage}
			policy={draft_state._tag === "Uninitialized" ? undefined : draft_state.policy}
		/>
	{/key}
</main>

<ProjectFolderPicker bind:open={attaching_project} onattached={AdoptAttachedProject} />
