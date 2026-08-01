import { Effect, Fiber, Queue, Scope } from "effect";

type RunnerCommand<Input> =
	| { readonly _tag: "Run"; readonly input: Input; readonly key: string }
	| { readonly _tag: "Release"; readonly key: string };

/**
 * Owns component-local scoped fibers started from synchronous attachment and
 * browser-observer callbacks. Replacing a key interrupts its previous work
 * before the latest work starts, so a slow external operation can never hold
 * the command loop hostage.
 */
export const MakeScopedAttachmentRunner = <Input>(
	Run: (input: Input) => Effect.Effect<void, never, Scope.Scope>,
) =>
	Effect.gen(function* () {
		const commands = yield* Queue.unbounded<RunnerCommand<Input>>();
		let next_id = 0;

		yield* Effect.gen(function* () {
			const fibers = new Map<string, Fiber.Fiber<void>>();
			while (true) {
				const command = yield* Queue.take(commands);
				const previous = fibers.get(command.key);
				if (previous !== undefined) {
					fibers.delete(command.key);
					yield* Fiber.interrupt(previous);
				}
				if (command._tag === "Release") continue;
				const fiber = yield* Run(command.input).pipe(Effect.forkScoped);
				fibers.set(command.key, fiber);
			}
		}).pipe(Effect.forkScoped);

		const Enqueue = (command: RunnerCommand<Input>) =>
			Effect.gen(function* () {
				yield* Queue.offer(commands, command);
			});

		return {
			Attachment: (input: Input) => {
				const key = `attachment:${next_id++}`;
				Queue.offerUnsafe(commands, { _tag: "Run", input, key });
				return () => Queue.offerUnsafe(commands, { _tag: "Release", key });
			},
			Replace: (key: string, input: Input) =>
				Effect.gen(function* () {
					yield* Enqueue({ _tag: "Run", input, key });
				}),
			ReplaceUnsafe: (key: string, input: Input) =>
				Queue.offerUnsafe(commands, { _tag: "Run", input, key }),
			Release: (key: string) =>
				Effect.gen(function* () {
					yield* Enqueue({ _tag: "Release", key });
				}),
		};
	});
