<script lang="ts" effect>
	import { goto } from "$app/navigation";
	import { Effect } from "effect";
	import { get } from "svelte/store";
	import type { ThreadSessionPolicy } from "@artisan/protocol";
	import { ArtisanClient } from "@artisan/transport/client";
	import type { ComposerSubmission } from "$lib/composer/image-attachments";
	import {
		draft_thread_policy,
		draft_thread_project,
		pending_first_submission,
	} from "$lib/root/draft-thread";
	import { recall_last_model } from "$lib/root/last-model";
	import { ThreadRoutePath } from "$lib/root/thread-navigation";
	import ThreadComposer from "../../components/thread-composer.sv";

	const client = yield* ArtisanClient;

	/** Every draft starts from the most recently used project; the panel can change it. */
	const catalog = yield* client.ListProjects;
	draft_thread_project.set(catalog.projects[0]);

	/**
	 * The draft is the only place the engine can be chosen: it locks once the
	 * first message creates the session. A new draft inherits the model the
	 * user chose last time (with that model's own default effort and speed, as
	 * if re-picked); with no usable history it mirrors the backend's default
	 * session policy with the runtime catalog's default model engine.
	 *
	 * This route remounts on every navigation to/from the draft (e.g. the user
	 * picks a model, navigates away, then back). Seeding unconditionally would
	 * wipe whatever the user already shaped in the draft. Seed the default only
	 * when no draft policy exists yet; a draft the user has touched survives
	 * remounts until the first message creates the durable thread.
	 */
	const runtime_catalog = yield* client.GetRuntimeCatalog;
	if (get(draft_thread_policy) === undefined) {
		const last_model_id = recall_last_model();
		const last_model = runtime_catalog.manifest.models.find(
			(model) =>
				model.native_model_id === last_model_id && model.disabled === undefined,
		);
		const default_model = runtime_catalog.manifest.models.find(
			(model) => model.id === runtime_catalog.default_model_id,
		);
		const seed_model = last_model ?? default_model;
		const seed_thinking = last_model?.capabilities.thinking;
		draft_thread_policy.set({
			engine_id: seed_model?.harness ?? "codex",
			model: last_model?.native_model_id,
			permission_mode: "on_request",
			reasoning_effort:
				seed_thinking?.availability === "supported"
					? seed_thinking.default === "light"
						? "low"
						: seed_thinking.default
					: "medium",
			sandbox_mode: "workspace_write",
			service_tier:
				last_model?.capabilities.speed_options.find(
					(option) => option.default && option.disabled === undefined,
				)?.native_value ?? "standard",
			strict_clarification: false,
			web_search_enabled: false,
		});
	}

	/** A retried submit must reuse the already created thread instead of minting another. */
	let created_thread_id: string | undefined;

	/**
	 * The durable thread materializes only here, at the first send: it is
	 * created with the draft's project, receives the draft's session policy,
	 * and the submission is handed to the routed thread, which owns the
	 * durable message pipeline.
	 */
	const SubmitFirstMessage = (submission: ComposerSubmission) =>
		Effect.gen(function* () {
			const project = get(draft_thread_project);
			if (project === undefined) {
				return yield* Effect.fail({
					message: "Select a project at the top of the panel before sending.",
				});
			}
			const thread_id =
				created_thread_id ??
				(yield* client.CreateThread({
					project_id: project.project_id,
					title: "New thread",
				})).thread_id;
			created_thread_id = thread_id;
			const policy = get(draft_thread_policy);
			if (policy !== undefined) {
				yield* client.UpdateThreadSessionPolicy({ policy, thread_id });
			}
			pending_first_submission.set({ submission, thread_id });
			yield* Effect.promise(() => goto(ThreadRoutePath(thread_id)));
		});

	const UpdateDraftPolicy = (policy: ThreadSessionPolicy) => {
		draft_thread_policy.set(policy);
	};
</script>

<svelte:head><title>New thread · Artisan Editor</title></svelte:head>

<main class="relative h-full min-h-0 overflow-hidden" aria-label="New thread draft">
	<ThreadComposer
		onpolicychange={UpdateDraftPolicy}
		onsubmit={SubmitFirstMessage}
		policy={$draft_thread_policy}
	/>
</main>
