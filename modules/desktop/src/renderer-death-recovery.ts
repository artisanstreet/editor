import { Effect, Fiber } from "effect";

export interface RendererDeathRecovery {
	/** Schedules one deferred authenticated recovery after a renderer disappears. */
	readonly Request: () => void;
	/** Prevents admission and awaits any active recovery's cancellation during shutdown. */
	readonly Close: () => Effect.Effect<void>;
}

interface RendererDeathRecoveryOptions {
	readonly IsAvailable: () => boolean;
	readonly Reconnect: () => Effect.Effect<void, unknown>;
	readonly ReportFailure: () => void;
	/** Electron must leave `render-process-gone` before navigating a replacement document. */
	readonly Schedule?: (task: () => void) => void;
}

/**
 * Contains one renderer-process loss without turning recovery into an
 * unbounded retry loop. Navigation is deliberately deferred: Electron cannot
 * safely replace a document synchronously from `render-process-gone`.
 */
export const make_renderer_death_recovery = (
	options: RendererDeathRecoveryOptions,
): RendererDeathRecovery => {
	let closed = false;
	let pending = false;
	let recovery_fiber: Fiber.Fiber<void> | undefined;
	const Schedule = options.Schedule ?? ((task) => void setTimeout(task, 0));

	const Request = () => {
		if (closed || pending || !options.IsAvailable()) return;
		pending = true;
		Schedule(() => {
			if (closed || !options.IsAvailable()) {
				pending = false;
				return;
			}
			const fiber = Effect.runFork(
				options
					.Reconnect()
					.pipe(
						Effect.catchCause(() =>
							closed ? Effect.void : Effect.sync(options.ReportFailure),
						),
					),
			);
			recovery_fiber = fiber;
			fiber.addObserver(() => {
				pending = false;
				if (recovery_fiber === fiber) recovery_fiber = undefined;
			});
		});
	};

	return {
		Close: () =>
			Effect.gen(function* () {
				closed = true;
				pending = false;
				const active = recovery_fiber;
				recovery_fiber = undefined;
				if (active !== undefined) yield* Fiber.interrupt(active);
			}),
		Request,
	};
};
