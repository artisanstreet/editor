import { describe, expect, it } from "@effect/vitest";
import { Deferred, Effect, Exit, Fiber, Ref, Scope } from "effect";

import {
	BuildThreadMessageCommand,
	MakeSubmitGate,
	SubmitDurableCommand,
	ThreadInteractionError,
	type ThreadInteractionContext,
} from "../../modules/frontend/src/lib/thread-interaction/commands";

const Submission = (text: string, attachments: ReadonlyArray<unknown> = []) =>
	({
		attachments,
		text,
	}) as never;

const Context: ThreadInteractionContext = {
	session: {
		pending_question: undefined,
		policy: { engine_id: "codex" },
	} as ThreadInteractionContext["session"],
	thread: {
		primary_project: { project_id: "project-1", root_path: "C:/workspace" },
		thread_id: "thread-1",
	} as ThreadInteractionContext["thread"],
	thread_id: "thread-1",
	work: undefined,
};

describe("thread interaction commands", () => {
	it("builds the public thread send command and leaves idle versus steering routing to the backend", () => {
		expect(BuildThreadMessageCommand(Context, Submission("  Continue the work  "))).toEqual({
			_tag: "ready",
			command: {
				payload: {
					attachments: [],
					content: [{ text: "  Continue the work  ", type: "text" }],
					engine_id: "codex",
					text: "Continue the work",
					type: "thread.send_message",
				},
				thread_id: "thread-1",
			},
		});
	});

	it("routes a pending intake question through its existing response command", () => {
		expect(
			BuildThreadMessageCommand(
				{
					...Context,
					session: {
						...Context.session,
						pending_question: {
							question_id: "question-1",
							state: "pending",
							text: "Which directory should Artisan use?",
						},
					},
				},
				Submission("C:/workspace"),
			),
		).toEqual({
			_tag: "ready",
			command: {
				payload: {
					answers: { answer: ["C:/workspace"] },
					question_id: "question-1",
					type: "intake.respond_question",
				},
				thread_id: "thread-1",
			},
		});
	});

	it("carries the active coordinator run so the backend can steer it", () => {
		expect(
			BuildThreadMessageCommand(
				{
					...Context,
					work: { agent_id: "agent-1", engine_id: "codex", run_id: "run-1" } as never,
				},
				Submission("Keep going"),
			),
		).toMatchObject({
			_tag: "ready",
			command: { agent_id: "agent-1", run_id: "run-1" },
		});
	});

	it("does not invent a project working directory", () => {
		expect(
			BuildThreadMessageCommand({ ...Context, thread: undefined }, Submission("Hello")),
		).toMatchObject({
			_tag: "invalid",
			error: {
				_tag: "ThreadInteractionError",
				message: "Assign a project to this thread before sending a message.",
			},
		});
	});

	it("keeps an attachment-only message intact for the durable command", () => {
		const attachments = [
			{
				content_base64: "aGVsbG8=",
				id: "attachment-1",
				mime_type: "image/png",
				name: "design.png",
				position: 0,
				size_bytes: 5,
			},
		];

		expect(BuildThreadMessageCommand(Context, Submission("", attachments))).toMatchObject({
			_tag: "ready",
			command: {
				payload: {
					attachments: [
						{
							bytes: new Uint8Array([104, 101, 108, 108, 111]),
							client_token: "attachment-1",
							media_type: "image/png",
							name: "design.png",
						},
					],
					content: [{ client_token: "attachment-1", type: "image" }],
					text: "Attached image",
					type: "thread.send_message",
				},
			},
		});
	});

	it("keeps an accepted durable command successful while reconciliation is blocked", async () => {
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				const scope = yield* Scope.make();
				const started = yield* Deferred.make<void>();
				const hold = yield* Deferred.make<void>();

				const accepted = yield* SubmitDurableCommand(
					Effect.succeed("accepted"),
					Deferred.succeed(started, undefined).pipe(Effect.andThen(Deferred.await(hold))),
					scope,
				);
				yield* Deferred.await(started);
				yield* Scope.close(scope, Exit.void);

				return accepted;
			}),
		);

		expect(result).toBe("accepted");
	});

	it("keeps a rejected command as a submit failure without starting reconciliation", async () => {
		const error = new ThreadInteractionError({ message: "Command was rejected." });

		const output = await Effect.runPromise(
			Effect.gen(function* () {
				const scope = yield* Scope.make();
				const reconciled = yield* Deferred.make<void>();
				const exit = yield* SubmitDurableCommand(
					Effect.fail(error),
					Deferred.succeed(reconciled, undefined),
					scope,
				).pipe(Effect.exit);
				const reconciliation = yield* Deferred.poll(reconciled);
				yield* Scope.close(scope, Exit.void);

				return { exit, reconciliation };
			}),
		);

		expect(output.exit._tag).toBe("Failure");
		expect(output.reconciliation._tag).toBe("None");
	});

	it("admits one concurrent submit and releases the gate after completion", async () => {
		const output = await Effect.runPromise(
			Effect.gen(function* () {
				const gate = yield* MakeSubmitGate;
				const entered = yield* Deferred.make<void>();
				const release = yield* Deferred.make<void>();
				const invocations = yield* Ref.make(0);
				const Submit = Effect.gen(function* () {
					if (!(yield* gate.Acquire)) return false;

					return yield* Ref.update(invocations, (count) => count + 1).pipe(
						Effect.andThen(Deferred.succeed(entered, undefined)),
						Effect.andThen(Deferred.await(release)),
						Effect.as(true),
						Effect.ensuring(gate.Release),
					);
				});

				const first = yield* Submit.pipe(Effect.forkChild({ startImmediately: true }));
				yield* Deferred.await(entered);
				const second = yield* Submit;
				yield* Deferred.succeed(release, undefined);
				const first_result = yield* Fiber.join(first);
				const third = yield* Submit;

				return { first_result, invocations: yield* Ref.get(invocations), second, third };
			}),
		);

		expect(output).toEqual({ first_result: true, invocations: 2, second: false, third: true });
	});

	it("releases the submit gate after a failed submit", async () => {
		const output = await Effect.runPromise(
			Effect.gen(function* () {
				const gate = yield* MakeSubmitGate;
				const failure = yield* Effect.gen(function* () {
					if (!(yield* gate.Acquire)) return;

					yield* Effect.fail(new ThreadInteractionError({ message: "Submit failed." }));
				}).pipe(Effect.ensuring(gate.Release), Effect.exit);
				const retry = yield* gate.Acquire;
				yield* gate.Release;

				return { failure, retry };
			}),
		);

		expect(output.failure._tag).toBe("Failure");
		expect(output.retry).toBe(true);
	});
});
