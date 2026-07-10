import { Effect } from "effect";

import type { EventEnvelope } from "@artisan/protocol";

import type { GraphTransaction, GraphTransitionInput } from "./graph-context";
import type { DependencyEvaluation } from "./dependency-evaluation";
import type { JoinEvaluation } from "./join-evaluation";

export interface GraphAdvancement {
	readonly advance_graph: (
		transaction: GraphTransaction,
		input: GraphTransitionInput,
	) => Effect.Effect<ReadonlyArray<EventEnvelope>, unknown>;
}

/** Converges dependency and join gates before deriving the aggregate group state. */
export function make_graph_advancement(
	dependencies: DependencyEvaluation,
	joins: JoinEvaluation,
): GraphAdvancement {
	const advance_graph = (transaction: GraphTransaction, input: GraphTransitionInput) =>
		Effect.gen(function* () {
			const events: Array<EventEnvelope> = [];

			while (true) {
				const dependency_events = yield* dependencies.evaluate_dependencies(
					transaction,
					input,
				);
				const join_events = yield* joins.resolve_joins(transaction, input);

				events.push(...dependency_events, ...join_events);

				if (dependency_events.length === 0 && join_events.length === 0) {
					break;
				}
			}

			events.push(...(yield* joins.update_group_state(transaction, input)));

			return events;
		});

	return { advance_graph };
}
