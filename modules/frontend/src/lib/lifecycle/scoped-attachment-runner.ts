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
		/**
		 * The queue carries each pending key once; the mailbox holds its latest
		 * command. This bounds a noisy observer's backlog to its distinct keys
		 * without dropping work for a different attachment.
		 */
		const pending_keys = yield* Queue.unbounded<string>();
		const mailbox = new Map<string, RunnerCommand<Input>>();
		const queued_keys = new Set<string>();
		let next_id = 0;
		const Store = (command: RunnerCommand<Input>) => {
			mailbox.set(command.key, command);
			if (queued_keys.has(command.key)) return undefined;
			queued_keys.add(command.key);
			return command.key;
		};

		yield* Effect.gen(function* () {
			const fibers = new Map<string, Fiber.Fiber<void>>();
			while (true) {
				const key = yield* Queue.take(pending_keys);
				queued_keys.delete(key);
				let command = mailbox.get(key);
				if (command === undefined) continue;
				mailbox.delete(key);
				const previous = fibers.get(key);
				if (previous !== undefined) {
					fibers.delete(key);
					yield* Fiber.interrupt(previous);
				}
				/** An ingress during interruption supersedes the command just taken. */
				const replacement = mailbox.get(key);
				if (replacement !== undefined) {
					mailbox.delete(key);
					command = replacement;
				}
				if (command._tag === "Release") continue;
				const fiber = yield* Run(command.input).pipe(Effect.forkScoped);
				fibers.set(key, fiber);
			}
		}).pipe(Effect.forkScoped);

		const Enqueue = (command: RunnerCommand<Input>) =>
			Effect.gen(function* () {
				const key = Store(command);
				if (key !== undefined) yield* Queue.offer(pending_keys, key);
			});

		return {
			Attachment: (input: Input) => {
				const key = `attachment:${next_id++}`;
				const pending_key = Store({ _tag: "Run", input, key });
				if (pending_key !== undefined) Queue.offerUnsafe(pending_keys, pending_key);
				return () => {
					const pending_key = Store({ _tag: "Release", key });
					if (pending_key !== undefined) Queue.offerUnsafe(pending_keys, pending_key);
				};
			},
			Replace: (key: string, input: Input) =>
				Effect.gen(function* () {
					yield* Enqueue({ _tag: "Run", input, key });
				}),
			ReplaceUnsafe: (key: string, input: Input) => {
				const pending_key = Store({ _tag: "Run", input, key });
				if (pending_key !== undefined) Queue.offerUnsafe(pending_keys, pending_key);
			},
			Release: (key: string) =>
				Effect.gen(function* () {
					yield* Enqueue({ _tag: "Release", key });
				}),
		};
	});
