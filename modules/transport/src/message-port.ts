import { Cause, Data, Deferred, Effect, Queue } from "effect";

/** Identifies a normalized MessagePort adapter failure. */
export type MessagePortErrorCode =
	| "closed"
	| "configuration"
	| "message_error"
	| "overflow"
	| "send";

/** Reports a bounded, shell-neutral MessagePort failure. */
export class MessagePortError extends Data.TaggedError("MessagePortError")<{
	readonly cause: unknown;
	readonly code: MessagePortErrorCode;
	readonly dropped_messages: number;
}> {}

/** Describes why a normalized MessagePort stopped accepting work. */
export interface MessagePortClose {
	readonly code: "closed" | "message_error" | "overflow";
	readonly dropped_messages: number;
}

/** Configures the native callback buffer owned by one adapted port. */
export interface MessagePortAdapterOptions {
	readonly incoming_capacity?: number;
}

/** Supplies the native operations required by the deep MessagePort adapter. */
export interface MessagePortAdapterHooks {
	readonly add_close_listener: (listener: () => void) => () => void;
	readonly add_message_error_listener: (listener: (cause: unknown) => void) => () => void;
	readonly add_message_listener: (listener: (message: unknown) => void) => () => void;
	readonly close: () => void;
	readonly post_message: (message: unknown, transfer?: ReadonlyArray<object>) => void;
	readonly start: () => void;
}

/**
 * Hides native event-emitter and event-target differences behind one bounded
 * Effect interface. A MessagePort is reliable while open; overflow therefore
 * closes the port as an explicit gap instead of inventing packet acknowledgements.
 */
export interface MessagePortLike {
	readonly Close: Effect.Effect<void>;
	readonly Closed: Effect.Effect<MessagePortClose>;
	readonly Receive: Effect.Effect<unknown, MessagePortError>;
	readonly Send: (
		message: unknown,
		transfer?: ReadonlyArray<object>,
	) => Effect.Effect<void, MessagePortError>;
}

function port_error(code: MessagePortErrorCode, cause: unknown, dropped_messages: number) {
	return new MessagePortError({ cause, code, dropped_messages });
}

function validate_capacity(capacity: number) {
	return Number.isSafeInteger(capacity) && capacity > 0;
}

/** Builds a scope-owned MessagePortLike from shell-specific listener hooks. */
export function make_message_port_like(
	hooks: MessagePortAdapterHooks,
	options: MessagePortAdapterOptions = {},
) {
	const incoming_capacity = options.incoming_capacity ?? 256;

	return Effect.gen(function* () {
		if (!validate_capacity(incoming_capacity)) {
			return yield* Effect.fail(
				port_error(
					"configuration",
					new Error("incoming_capacity must be a positive safe integer"),
					0,
				),
			);
		}

		const incoming = yield* Effect.acquireRelease(
			Queue.dropping<unknown, MessagePortError>(incoming_capacity),
			Queue.shutdown,
		);
		const closed = yield* Deferred.make<MessagePortClose>();
		let close_state: MessagePortClose | undefined;

		const finish = (state: MessagePortClose, failure?: MessagePortError) => {
			if (close_state) {
				return;
			}

			close_state = state;
			Deferred.doneUnsafe(closed, Effect.succeed(state));

			if (failure) {
				Queue.failCauseUnsafe(incoming, Cause.fail(failure));
			} else {
				Queue.failCauseUnsafe(
					incoming,
					Cause.fail(
						port_error(
							"closed",
							new Error("MessagePort closed"),
							state.dropped_messages,
						),
					),
				);
			}
		};

		const close_native = () => {
			try {
				hooks.close();
			} catch {
				/** Native close is best-effort after local state has already closed. */
			}
		};
		const on_message = (message: unknown) => {
			if (close_state) {
				return;
			}

			if (Queue.offerUnsafe(incoming, message)) {
				return;
			}

			const state: MessagePortClose = {
				code: "overflow",
				dropped_messages: 1,
			};

			finish(
				state,
				port_error(
					"overflow",
					new Error("incoming MessagePort buffer overflowed"),
					state.dropped_messages,
				),
			);
			close_native();
		};
		const on_close = () => {
			finish({ code: "closed", dropped_messages: 0 });
		};
		const on_message_error = (cause: unknown) => {
			const state: MessagePortClose = { code: "message_error", dropped_messages: 0 };

			finish(state, port_error("message_error", cause, 0));
			close_native();
		};
		const remove_listeners = (removers: ReadonlyArray<() => void>) => {
			for (const remove of [...removers].reverse()) {
				try {
					remove();
				} catch {
					/** Listener removal is best-effort while the native port is closing. */
				}
			}
		};
		const acquire_listeners = Effect.try({
			try: () => {
				const removers: Array<() => void> = [];

				try {
					removers.push(hooks.add_message_listener(on_message));
					removers.push(hooks.add_close_listener(on_close));
					removers.push(hooks.add_message_error_listener(on_message_error));
					hooks.start();
				} catch (cause) {
					remove_listeners(removers);
					close_native();

					throw cause;
				}

				return removers;
			},
			catch: (cause) => port_error("message_error", cause, 0),
		}).pipe(
			Effect.tapError((failure) =>
				Effect.sync(() => {
					finish({ code: "message_error", dropped_messages: 0 }, failure);
				}),
			),
		);

		yield* Effect.acquireRelease(acquire_listeners, (removers) =>
			Effect.sync(() => {
				remove_listeners(removers);
				finish({ code: "closed", dropped_messages: 0 });
				close_native();
			}),
		);

		const Close = Effect.sync(() => {
			finish({ code: "closed", dropped_messages: 0 });
			close_native();
		});
		const Send = (message: unknown, transfer?: ReadonlyArray<object>) =>
			Effect.try({
				try: () => {
					if (close_state) {
						throw port_error(
							"closed",
							new Error("cannot send after MessagePort close"),
							close_state.dropped_messages,
						);
					}

					hooks.post_message(message, transfer);
				},
				catch: (cause) =>
					cause instanceof MessagePortError
						? cause
						: port_error("send", cause, close_state?.dropped_messages ?? 0),
			});

		return {
			Close,
			Closed: Deferred.await(closed),
			Receive: Queue.take(incoming),
			Send,
		} satisfies MessagePortLike;
	});
}
