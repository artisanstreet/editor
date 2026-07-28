<script lang="ts" effect>
	import { goto } from "$app/navigation";
	import { Effect } from "effect";
	import { get } from "svelte/store";
	import { ArtisanClient } from "@artisan/transport/client";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import {
		draft_thread_project,
		pending_first_submission,
	} from "$lib/root/draft-thread";
	import { ThreadRoutePath } from "$lib/root/thread-navigation";
	import ThreadComposer from "../../components/thread-composer.sv";

	const client = yield* ArtisanClient;

	/** Every draft starts from the most recently used project; the panel can change it. */
	const catalog = yield* client.ListProjects;
	draft_thread_project.set(catalog.projects[0]);

	/**
	 * The durable thread materializes only here, at the first send: it is
	 * created with the draft's project and the submission is handed to the
	 * routed thread, which owns the durable message pipeline.
	 */
	const SubmitFirstMessage = (submission: ComposerSubmission) =>
		Effect.gen(function* () {
			const project = get(draft_thread_project);
			if (project === undefined) {
				return yield* Effect.fail({
					message: "Select a project at the top of the panel before sending.",
				});
			}
			const thread = yield* client.CreateThread({
				project_id: project.project_id,
				title: "New thread",
			});
			pending_first_submission.set({ submission, thread_id: thread.thread_id });
			yield* Effect.promise(() => goto(ThreadRoutePath(thread.thread_id)));
		});
</script>

<svelte:head><title>New thread · Artisan Editor</title></svelte:head>

<main class="relative h-full min-h-0 overflow-hidden" aria-label="New thread draft">
	<ThreadComposer onsubmit={SubmitFirstMessage} />
</main>
