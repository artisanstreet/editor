import { Effect, Fiber } from "effect";

import {
	begin_pending_steering_lip,
	release_pending_steering_lip,
	type SteeringPendingLipState,
} from "./steering-pending-lip";

/** A lip that vanished the frame it appeared read as a glitch; hold it at least this long. */
const minimum_lip_display_ms = 150;

/**
 * The composer owns the reactive lip state its template renders; the stages
 * borrow it through accessors so the staging logic itself can live here.
 */
export interface SteeringStageHarness<Submission> {
	readonly Lip: () => SteeringPendingLipState<Submission>;
	readonly ReplaceLip: (next: SteeringPendingLipState<Submission>) => void;
	readonly SteeringChanged: (pending: boolean) => void;
	/** Recalls one queued steer durably; fails once the engine has claimed it. */
	readonly Withdraw: (command_id: string) => Effect.Effect<void, { readonly message: string }>;
}

/**
 * What the stages hold about one submitted steer while it can still be taken
 * back: the durable name its send earned at receipt, the settlement fiber
 * watching it, and whether the user asked it withdrawn before either existed.
 */
interface RegisteredSteer {
	command_id?: string;
	settlement?: Fiber.RuntimeFiber<unknown, unknown>;
	withdraw_requested: boolean;
}

/**
 * A steer crosses two boundaries after submit, and each has its own surface.
 * Queued: the message exists only in the composer's lip, and nothing steers
 * yet. Taken up: Artisan projected the canonical message, so the lip yields to
 * the transcript and the "Steering" label rises. Settled: the engine visibly
 * reacted — or the wait expired — and the label lowers. Generations keep a
 * stale stage from operating a later steer's surfaces.
 *
 * While a steer is still queued it remains the user's to recall: `Withdraw`
 * acts on intent immediately (the lip closes), recalls the durable send once
 * its command id is known, and interrupts the settlement watcher so no label
 * ever narrates a message that was taken back.
 */
export const MakeSteeringStages = <Submission>(harness: SteeringStageHarness<Submission>) => {
	/** Which steer's acknowledgement the transcript's "Steering" label narrates. */
	let label_generation: number | undefined;
	const steers = new Map<number, RegisteredSteer>();
	const ReleaseLip = (generation: number) =>
		Effect.gen(function* () {
			const pending_lip = harness.Lip().pending.find((lip) => lip.generation === generation);
			if (pending_lip === undefined) return;
			const remaining_ms = minimum_lip_display_ms - (Date.now() - pending_lip.started_at);
			if (remaining_ms > 0) yield* Effect.sleep(remaining_ms);
			harness.ReplaceLip(release_pending_steering_lip(harness.Lip(), generation).state);
		});
	return {
		/** Hands the settlement fiber to the steer it watches, for interruption on recall. */
		Adopt: (generation: number, settlement: Fiber.RuntimeFiber<unknown, unknown>): void => {
			const steer = steers.get(generation);
			if (steer !== undefined) steer.settlement = settlement;
		},
		/** Queues the steer: only its lip row marks it, and the label waits for take-up. */
		Begin: (submission: Submission, started_at: number): number => {
			const next = begin_pending_steering_lip(harness.Lip(), submission, started_at);
			harness.ReplaceLip(next.state);
			steers.set(next.begun.generation, { withdraw_requested: false });
			return next.begun.generation;
		},
		/**
		 * Names the steer's durable send once the receipt arrives. Returns whether
		 * a withdrawal was requested while the send was still nameless, so the
		 * caller can complete that recall instead of starting settlement.
		 */
		Bind: (generation: number, command_id: string): boolean => {
			const steer = steers.get(generation);
			if (steer === undefined) return false;
			steer.command_id = command_id;
			return steer.withdraw_requested;
		},
		ReleaseLip,
		/** Only the steer that raised the label may lower it; a stale settlement has no authority. */
		Settle: (generation: number) =>
			Effect.gen(function* () {
				yield* ReleaseLip(generation);
				steers.delete(generation);
				if (label_generation !== generation) return;
				label_generation = undefined;
				harness.SteeringChanged(false);
			}),
		/**
		 * The steer has left the queue: Artisan projected its canonical message,
		 * so the lip's copy is redundant and the wait is now the engine's to
		 * answer. Only from here may the transcript say "Steering" — while the
		 * message was still queued there was no steer in flight to narrate.
		 */
		TakeUp: (generation: number) =>
			Effect.gen(function* () {
				yield* ReleaseLip(generation);
				label_generation = generation;
				harness.SteeringChanged(true);
			}),
		/**
		 * Recalls a queued steer. Intent acts now — the lip closes even while the
		 * send is still nameless, and `Bind` finishes the recall when the name
		 * arrives. Returns whether the steer is (or will be) withdrawn; a refusal
		 * fails instead, and the steer proceeds with its surfaces intact.
		 */
		Withdraw: (generation: number): Effect.Effect<boolean, { readonly message: string }> =>
			Effect.gen(function* () {
				const steer = steers.get(generation);
				if (steer === undefined) return false;
				steer.withdraw_requested = true;
				yield* ReleaseLip(generation);
				const command_id = steer.command_id;
				if (command_id === undefined) return true;
				yield* harness.Withdraw(command_id);
				steers.delete(generation);
				if (steer.settlement !== undefined) yield* Fiber.interrupt(steer.settlement);
				return true;
			}),
	};
};
