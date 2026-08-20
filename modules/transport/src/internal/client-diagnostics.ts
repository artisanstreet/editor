import { Cause, Effect, Queue, Ref, Stream } from "effect";

import type { TransportDiagnosticEvent, TransportDiagnosticsSnapshot } from "../client-api/service";
import type { TransportRuntime } from "../transport-runtime";

/** A diagnostic event authored at a record site; the recorder stamps `at`. */
export type TransportDiagnosticEventInput = TransportDiagnosticEvent extends infer Event
	? Event extends TransportDiagnosticEvent
		? Omit<Event, "at">
		: never
	: never;

/**
 * Records the transport client's execution history: a complete journal for
 * after-the-fact reads plus a live feed for continuous observers. Recording
 * never fails and never blocks the connection work being observed.
 */
export interface ClientDiagnostics {
	readonly Changes: Stream.Stream<TransportDiagnosticEvent>;
	readonly Record: (event: TransportDiagnosticEventInput) => Effect.Effect<void>;
	readonly Snapshot: Effect.Effect<TransportDiagnosticsSnapshot>;
	readonly Shutdown: Effect.Effect<void>;
}

interface JournalState {
	readonly events: ReadonlyArray<TransportDiagnosticEvent>;
}

/**
 * Renders an unknown failure cause as one short human-readable line. Pure
 * formatting for journal dumps only — never parsed back into a value.
 */
export function describe_diagnostic_cause(cause: unknown): string {
	if (cause instanceof Error) {
		return cause.message;
	}

	if (typeof cause === "string") {
		return cause;
	}

	try {
		return (JSON.stringify(cause) ?? String(cause)).slice(0, 240);
	} catch {
		return String(cause);
	}
}

/** Creates one lossless journal of transport events. */
export const make_client_diagnostics = (runtime: typeof TransportRuntime.Service) =>
	Effect.gen(function* () {
		const journal = yield* Ref.make<JournalState>({ events: [] });
		const live = yield* Queue.unbounded<TransportDiagnosticEvent, Cause.Done<void>>();

		const record = (input: TransportDiagnosticEventInput) =>
			Effect.gen(function* () {
				const at = yield* runtime.Now;
				const event = { ...input, at } as TransportDiagnosticEvent;

				yield* Ref.update(journal, (state) => ({ events: [...state.events, event] }));
				Queue.offerUnsafe(live, event);
			});

		const snapshot = Ref.get(journal).pipe(
			Effect.map(
				(state): TransportDiagnosticsSnapshot => ({ dropped: 0, events: state.events }),
			),
		);

		return {
			Changes: Stream.fromQueue(live),
			Record: record,
			Shutdown: Queue.end(live).pipe(Effect.asVoid),
			Snapshot: snapshot,
		} satisfies ClientDiagnostics;
	});
