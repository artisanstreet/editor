import { describe, expect, it } from "@effect/vitest";
import { Effect, Fiber, Option, Ref, Stream, SubscriptionRef } from "effect";
import { TestClock } from "effect/testing";

import {
	ApplyAuthoritativeThreadRefresh,
	AppendBoundedTerminalOutput,
	ApplyThreadListUpdate,
	ApplyThreadListSubscriptionFailure,
	ApplyThreadTranscriptUpdate,
	BuildLiveWorkspaceMessageCommand,
	IsCurrentTerminalOutputWatcher,
	IsCurrentThreadSelection,
	RunThreadListSubscription,
	ShouldRefreshForConnection,
	ToLiveWorkspacePhase,
	type LiveWorkspaceSnapshot,
} from "../../modules/frontend/src/lib/live-workspace/store";

const EmptySnapshot: LiveWorkspaceSnapshot = {
	error: Option.none(),
	errors: {},
	global_guidance: Option.none(),
	capabilities: Option.none(),
	capability_detail: Option.none(),
	capability_oauth: Option.none(),
	git_diff: Option.none(),
	git_workspace: Option.none(),
	model_behaviour: Option.none(),
	orchestration_graph: Option.none(),
	orchestration_groups: Option.none(),
	phase: "ready",
	preview_asset_metadata: Option.none(),
	preview_inspection_result: Option.none(),
	preview_inspection_session: Option.none(),
	preview_target: Option.none(),
	preview_targets: [],
	routine_detail: Option.none(),
	routines: Option.none(),
	selected_group_id: Option.none(),
	selected_thread_id: Option.some("thread-1"),
	session: Option.none(),
	surface_items: Option.none(),
	surface_usage: Option.none(),
	terminals: [],
	terminal_output: {},
	thread_work: Option.none(),
	tool_approvals: Option.none(),
	tool_invocations: Option.none(),
	tool_registry: Option.none(),
	transcript: Option.none(),
	threads: [],
	workspace_change_diff: Option.none(),
	workspace_changes: Option.none(),
	workspace_conflicts: Option.none(),
	workspace_file: Option.none(),
	workspace_file_page: Option.none(),
};

describe("live workspace state", () => {
	it("bounds terminal scrollback at decoded text boundaries", () => {
		expect(AppendBoundedTerminalOutput("1234", "5678", 6)).toBe("345678");
	});

	it("maps the desktop lifecycle into explicit renderer states", () => {
		expect(ToLiveWorkspacePhase("connecting")).toBe("connecting");
		expect(ToLiveWorkspacePhase("ready")).toBe("ready");
		expect(ToLiveWorkspacePhase("reconnecting")).toBe("reconnecting");
		expect(ToLiveWorkspacePhase("stale")).toBe("stale");
		expect(ToLiveWorkspacePhase("error")).toBe("error");
		expect(ToLiveWorkspacePhase("unavailable")).toBe("error");
	});

	it("reloads projections after a reconnect becomes ready", () => {
		expect(ShouldRefreshForConnection("reconnecting")).toBe(false);
		expect(ShouldRefreshForConnection("ready")).toBe(true);
	});

	it("clears a removed selected thread rather than retaining stale local state", () => {
		const updated = ApplyThreadListUpdate(EmptySnapshot, {
			thread_id: "thread-1",
			type: "remove",
		});

		expect(Option.isNone(updated.selected_thread_id)).toBe(true);
		expect(updated.threads).toEqual([]);
	});

	it("clears selected work when an authoritative snapshot no longer contains it", () => {
		const updated = ApplyThreadListUpdate(EmptySnapshot, {
			threads: [],
			type: "snapshot",
		});

		expect(updated.threads).toEqual([]);
		expect(Option.isNone(updated.selected_thread_id)).toBe(true);
		expect(Option.isNone(updated.thread_work)).toBe(true);
		expect(updated.phase).toBe("empty");
	});

	it("selects and marks the first thread ready when the live list creates it", () => {
		const updated = ApplyThreadListUpdate(
			{
				...EmptySnapshot,
				selected_thread_id: Option.none(),
				phase: "empty",
			},
			{
				thread: { thread_id: "thread-2" } as never,
				type: "upsert",
			},
		);

		expect(Option.getOrUndefined(updated.selected_thread_id)).toBe("thread-2");
		expect(updated.phase).toBe("ready");
	});

	it("rejects a late work result after the user selected another thread", () => {
		const selected_other_thread = {
			...EmptySnapshot,
			selected_thread_id: Option.some("thread-2"),
		};

		expect(IsCurrentThreadSelection(selected_other_thread, 2, "thread-1", 1)).toBe(false);
	});

	it("builds a first Codex message without inventing active-run metadata", () => {
		const command = BuildLiveWorkspaceMessageCommand(
			{
				...EmptySnapshot,
				threads: [
					{
						thread_id: "thread-1",
						primary_project: {
							project_id: "project-1",
							root_path: "C:/workspace",
						},
					} as never,
				],
			},
			"  Start the implementation  ",
		);

		expect(Option.getOrUndefined(command)).toEqual({
			payload: {
				engine_id: "codex",
				mentioned_projects: [{ project_id: "project-1", root_path: "C:/workspace" }],
				text: "Start the implementation",
				type: "thread.send_message",
				working_directory: "C:/workspace",
			},
			thread_id: "thread-1",
		});
	});

	it("answers a strict intake clarification without requiring active work", () => {
		const command = BuildLiveWorkspaceMessageCommand(
			{
				...EmptySnapshot,
				session: Option.some({
					pending_question: {
						question_id: "question-1",
						state: "pending",
						text: "Which directory should Artisan use?",
					},
				} as never),
			},
			"C:/workspace",
		);

		expect(Option.getOrUndefined(command)).toEqual({
			payload: {
				answers: { answer: ["C:/workspace"] },
				question_id: "question-1",
				type: "intake.respond_question",
			},
			thread_id: "thread-1",
		});
	});

	it("fences terminal output after moving from terminal A's selected workspace to B", () => {
		const selected_b = {
			...EmptySnapshot,
			selected_thread_id: Option.some("thread-2"),
			terminals: [
				{
					terminal_id: "terminal-b",
					thread_id: "thread-2",
					workspace_id: "workspace-b",
				} as never,
			],
		};

		expect(
			IsCurrentTerminalOutputWatcher(selected_b, 2, {
				selection_generation: 1,
				terminal_id: "terminal-a",
				thread_id: "thread-1",
				workspace_id: "workspace-a",
			}),
		).toBe(false);
	});

	it("deduplicates replayed transcript appends without reconstructing raw events", () => {
		const entry = { event_id: "entry-1" } as never;
		const updated = ApplyThreadTranscriptUpdate(
			{ ...EmptySnapshot, transcript: Option.some({ entries: [entry] } as never) },
			{ entries: [entry], journal_sequence: 2, type: "append" },
		);

		expect(Option.getOrUndefined(updated.transcript)?.entries).toHaveLength(1);
	});

	it("clears a prior connection error after an authoritative refresh succeeds", () => {
		const recovered = ApplyAuthoritativeThreadRefresh(
			{
				...EmptySnapshot,
				error: Option.some("Temporary connection failure"),
				phase: "error",
			},
			[],
		);

		expect(Option.isNone(recovered.error)).toBe(true);
		expect(recovered.phase).toBe("empty");
	});

	it("retains the last projection but makes a lost thread subscription recoverable", () => {
		const failed = ApplyThreadListSubscriptionFailure(
			{ ...EmptySnapshot, threads: [] },
			"Thread subscription disconnected",
		);

		expect(failed.phase).toBe("error");
		expect(Option.getOrUndefined(failed.error)).toBe("Thread subscription disconnected");
	});

	it.effect(
		"retries a lost subscription and applies its recovered stream without another ready event",
		() =>
			Effect.gen(function* () {
				const attempts = yield* Ref.make(0);
				const updates = yield* Ref.make(0);
				const subscribe = Effect.gen(function* () {
					const attempt = yield* Ref.getAndUpdate(attempts, (count) => count + 1);
					if (attempt === 0) {
						return yield* Effect.fail({ message: "Initial subscription loss" });
					}

					return Stream.concat(
						Stream.fromEffect(
							Ref.update(updates, (count) => count + 1).pipe(
								Effect.as({
									journal_sequence: 1,
									threads: [],
									type: "snapshot",
								} as const),
							),
						),
						Stream.never,
					);
				});
				const fiber = yield* RunThreadListSubscription(
					subscribe,
					() => Effect.void,
					() => Effect.void,
				).pipe(Effect.forkScoped);

				yield* TestClock.adjust("100 millis");

				expect(yield* Ref.get(attempts)).toBe(2);
				expect(yield* Ref.get(updates)).toBe(1);
				yield* Fiber.interrupt(fiber);
			}).pipe(Effect.provide(TestClock.layer())),
	);

	it.effect("replays current state to a late renderer subscription", () =>
		Effect.gen(function* () {
			const state = yield* SubscriptionRef.make("before-subscribe");
			const replayed = yield* SubscriptionRef.changes(state).pipe(
				Stream.take(1),
				Stream.runHead,
			);

			expect(Option.getOrUndefined(replayed)).toBe("before-subscribe");
		}),
	);
});
