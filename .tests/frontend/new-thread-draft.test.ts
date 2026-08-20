import { Effect, Exit, Layer, Option, Scope } from "effect";
import { describe, expect, it } from "vitest";

import { MakeSnowflakeIdLive, type ProjectRef } from "@artisan/protocol";
import { ArtisanClient, ArtisanClientError } from "@artisan/transport/client";
import {
	ComposerDraftStore,
	ComposerDraftStoreLive,
} from "../../modules/frontend/src/lib/composer/draft-store";
import {
	PrepareNewThreadDraft,
	SubmitNewThreadDraft,
	new_thread_draft_key,
} from "../../modules/frontend/src/lib/root/new-thread-draft";
import {
	DraftThreadController,
	DraftThreadControllerLive,
} from "../../modules/frontend/src/lib/root/draft-thread";
import {
	FixtureArtisanClientService,
	fixture_project,
} from "../../modules/frontend/src/lib/runtime/fixtures/client";

const draft_key = new_thread_draft_key(undefined);
const submission = { attachments: [], text: "A message that was already sent." };
const policy = {
	engine_id: "codex" as const,
	model: "gpt-5.6-sol",
	permission: "supervised" as const,
	permission_mode: "on_request" as const,
	reasoning_effort: "xhigh" as const,
	sandbox_mode: "workspace_write" as const,
	service_tier: "fast" as const,
	strict_clarification: false,
	web_search_enabled: false,
};

const thread_primary_project: ProjectRef = {
	display_name: "Thread project",
	project_id: "thread-project",
	root_path: "C:\\workspace\\thread-project",
};

const client_failure = (message: string) =>
	new ArtisanClientError({
		cause: undefined,
		code: "protocol",
		message,
		protocol_code: "test_failure",
		retryable: false,
	});

const RunWithDraftServices = <Success, Failure>(
	program: Effect.Effect<
		Success,
		Failure,
		DraftThreadController | ComposerDraftStore | Scope.Scope
	>,
	client_layer = Layer.succeed(ArtisanClient, FixtureArtisanClientService),
) =>
	Effect.runPromise(
		Effect.scoped(
			program.pipe(
				Effect.provide(DraftThreadControllerLive),
				Effect.provide(ComposerDraftStoreLive),
				Effect.provide(MakeSnowflakeIdLive(29).pipe(Layer.orDie)),
				Effect.provide(client_layer),
			),
		),
	);

describe("new-thread draft lifecycle", () => {
	it("clears stale text and advances the mounted draft when New thread is explicit", async () => {
		const result = await RunWithDraftServices(
			Effect.gen(function* () {
				const drafts = yield* ComposerDraftStore;
				const controller = yield* DraftThreadController;
				yield* controller.Initialize(fixture_project, policy);
				yield* drafts.Write(draft_key, {
					attachments: [],
					text: submission.text,
					tokens: [],
				});

				yield* PrepareNewThreadDraft(draft_key);

				return {
					draft: yield* drafts.Read(draft_key),
					revision: yield* controller.CurrentRevision,
					state: yield* controller.Current,
				};
			}),
		);

		expect(Option.isNone(result.draft)).toBe(true);
		expect(result.revision).toBe(1);
		expect(result.state).toEqual({ _tag: "Uninitialized" });
		expect(new_thread_draft_key("project-one")).toBe("draft:project-one");
	});

	it("clears the composer draft before successful first-message navigation", async () => {
		const result = await RunWithDraftServices(
			Effect.gen(function* () {
				const drafts = yield* ComposerDraftStore;
				const controller = yield* DraftThreadController;
				yield* controller.Initialize(fixture_project, policy);
				yield* drafts.Write(draft_key, {
					attachments: [],
					text: submission.text,
					tokens: [],
				});

				const created = yield* SubmitNewThreadDraft(draft_key, submission);
				return { created, draft: yield* drafts.Read(draft_key) };
			}),
		);

		expect(result.created.submission).toEqual(submission);
		expect(Option.isNone(result.draft)).toBe(true);
	});

	it("keeps a failed atomic create retryable without publishing a partial thread", async () => {
		let create_attempts = 0;
		const client_layer = Layer.succeed(ArtisanClient, {
			...FixtureArtisanClientService,
			CreateThread: (input) =>
				Effect.gen(function* () {
					create_attempts += 1;
					if (create_attempts === 1)
						return yield* Effect.fail(client_failure("create lost"));
					return yield* FixtureArtisanClientService.CreateThread(input);
				}),
		});
		const result = await RunWithDraftServices(
			Effect.gen(function* () {
				const drafts = yield* ComposerDraftStore;
				const controller = yield* DraftThreadController;
				yield* controller.Initialize(fixture_project, policy);
				yield* drafts.Write(draft_key, {
					attachments: [],
					text: submission.text,
					tokens: [],
				});

				const failed = yield* Effect.exit(SubmitNewThreadDraft(draft_key, submission));
				const state_after_failure = yield* controller.Current;
				const retained_after_failure = yield* drafts.Read(draft_key);
				const created = yield* SubmitNewThreadDraft(draft_key, submission);
				return { created, failed, retained_after_failure, state_after_failure };
			}),
			client_layer,
		);

		expect(result.failed._tag).toBe("Failure");
		expect(Option.getOrThrow(result.retained_after_failure).text).toBe(submission.text);
		expect(result.state_after_failure).toMatchObject({ _tag: "Ready" });
		expect(result.created).toMatchObject({ _tag: "Created", submission });
		expect(create_attempts).toBe(2);
	});

	it("sends selected policy with one create request and retries a created draft locally", async () => {
		let creates = 0;
		let policy_updates = 0;
		let observed_policy: unknown;
		const client_layer = Layer.succeed(ArtisanClient, {
			...FixtureArtisanClientService,
			CreateThread: (input) =>
				Effect.gen(function* () {
					creates += 1;
					observed_policy = input.policy;
					return yield* FixtureArtisanClientService.CreateThread(input);
				}),
			UpdateThreadSessionPolicy: () =>
				Effect.gen(function* () {
					policy_updates += 1;
					return yield* Effect.die("creation must not update policy separately");
				}),
		});
		const result = await RunWithDraftServices(
			Effect.gen(function* () {
				const controller = yield* DraftThreadController;
				yield* controller.Initialize(fixture_project, policy);
				const created = yield* controller.Submit(submission);
				const retried = yield* controller.Retry;
				return { created, retried };
			}),
			client_layer,
		);

		expect(result.retried).toEqual(result.created);
		expect(observed_policy).toEqual(policy);
		expect(creates).toBe(1);
		expect(policy_updates).toBe(0);
	});

	it("creates from an authoritative thread project reference without a catalog project", async () => {
		let selected_project_id: string | undefined;
		const client_layer = Layer.succeed(ArtisanClient, {
			...FixtureArtisanClientService,
			CreateThread: (input) =>
				Effect.gen(function* () {
					selected_project_id = input.project_id;
					return yield* FixtureArtisanClientService.CreateThread(input);
				}),
		});
		const result = await RunWithDraftServices(
			Effect.gen(function* () {
				const controller = yield* DraftThreadController;
				yield* controller.Initialize(thread_primary_project, policy);
				return yield* controller.Submit(submission);
			}),
			client_layer,
		);

		expect(selected_project_id).toBe(thread_primary_project.project_id);
		expect(result.project).toEqual(thread_primary_project);
	});

	/**
	 * Both halves of the caller contract the route has to honour.
	 *
	 * A revision captured before a reset is stale by definition and is refused —
	 * that is the guard protecting a fresh draft from an alignment whose seed
	 * policy only finished afterwards. A revision read at the start of the
	 * attempt is not stale, and must align, because the surface still names the
	 * project the draft should carry.
	 *
	 * The route was passing a revision mirrored through a forked stream, which
	 * trails the controller by a scheduler turn and therefore reads as stale for
	 * the whole gap after a reset. That refusal was discarded, so the draft kept
	 * no project while its composer stayed armed, and the first send after every
	 * "New thread" failed `DraftProjectRequired` into a swallowed error.
	 */
	it("refuses a pre-reset revision and aligns one read after it", async () => {
		const result = await RunWithDraftServices(
			Effect.gen(function* () {
				const controller = yield* DraftThreadController;
				const captured_before_reset = yield* controller.CurrentRevision;
				yield* controller.Reset(Effect.void);

				const stale = yield* controller.AlignAtRevision(
					captured_before_reset,
					fixture_project,
					policy,
				);
				const refused_submit = yield* Effect.exit(controller.Submit(submission));

				const current = yield* controller.CurrentRevision;
				const fresh = yield* controller.AlignAtRevision(current, fixture_project, policy);
				return {
					created: yield* Effect.exit(controller.Submit(submission)),
					fresh,
					refused_submit,
					stale,
				};
			}),
		);

		expect(result.stale).toBe(false);
		expect(JSON.stringify(result.refused_submit)).toContain("DraftProjectRequired");
		expect(result.fresh).toBe(true);
		expect(result.created._tag).toBe("Success");
	});

	/** The routed thread must be able to take delivery of the retained first message. */
	it("hands the retained first submission to the thread it created", async () => {
		const result = await RunWithDraftServices(
			Effect.gen(function* () {
				const controller = yield* DraftThreadController;
				yield* controller.Initialize(fixture_project, policy);
				const created = yield* controller.Submit(submission);
				const claim = yield* controller.AwaitPendingSubmissionClaim(created.thread_id);
				if (claim !== undefined) yield* claim.Complete;
				return { claim, created, settled: yield* controller.Current };
			}),
		);

		expect(result.claim?.submission).toEqual(submission);
		expect(result.claim?.command_id).toBe(result.created.command_id);
		expect(result.settled).toEqual({ _tag: "Uninitialized" });
	});

	/**
	 * A route scope that took the claim and died without releasing it used to
	 * hold the controller shut for the rest of the session: every later draft
	 * created its thread, then waited forever for a claim that never came, so
	 * the first message was never sent and the new thread stayed empty.
	 */
	it("re-offers a scoped claim so a replacing route scope can deliver it", async () => {
		const result = await RunWithDraftServices(
			Effect.gen(function* () {
				const controller = yield* DraftThreadController;
				yield* controller.Initialize(fixture_project, policy);
				const created = yield* controller.Submit(submission);

				const first_route_scope = yield* Scope.make();
				const first = yield* controller
					.AwaitPendingSubmissionClaim(created.thread_id)
					.pipe(Scope.provide(first_route_scope));
				const replacement_route_scope = yield* Scope.make();
				/** While the outgoing scope owns it, the replacement cannot take delivery. */
				const contended = yield* controller
					.AwaitPendingSubmissionClaim(created.thread_id)
					.pipe(
						Scope.provide(replacement_route_scope),
						Effect.timeoutOption("2 seconds"),
					);
				yield* Scope.close(first_route_scope, Exit.void);
				/**
				 * The replacing scope acquires after the first scope's registered
				 * finalizer releases its claim during close.
				 */
				const second = yield* controller
					.AwaitPendingSubmissionClaim(created.thread_id)
					.pipe(Scope.provide(replacement_route_scope));
				if (second !== undefined) yield* second.Complete;
				yield* Scope.close(replacement_route_scope, Exit.void);

				return { contended, first, second };
			}),
		);

		expect(result.first?.submission).toEqual(submission);
		expect(Option.isNone(result.contended)).toBe(true);
		expect(result.second?.submission).toEqual(submission);
	});
});
