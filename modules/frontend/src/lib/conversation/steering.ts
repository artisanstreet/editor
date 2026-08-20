import type { ConversationItem, ConversationTurn } from "@artisan/protocol";

type ConversationSteeringItem = Pick<
	ConversationItem,
	"id" | "ordinal" | "run_id" | "source_refs" | "type"
>;
type ConversationSteeringTurn = Pick<ConversationTurn, "lifecycle" | "run_id">;

/**
 * Turn outcomes after which no further work lands, whatever the outcome.
 * `interrupted` belongs here: nothing more arrives on its own, and the resume
 * that could revive it is an explicit act rather than work already in flight.
 */
const settled_lifecycles = new Set(["completed", "failed", "interrupted", "cancelled"]);

const has_source_reference = (item: ConversationSteeringItem, source_reference: string) =>
	item.source_refs.some(
		(source) => source.reference === source_reference || source.event_id === source_reference,
	);

/**
 * Reports whether the engine has taken up a steering message yet.
 *
 * A steer settles in two stages, and only the second one is worth telling the
 * user about. The first is Artisan's own echo: the message becomes a canonical
 * user turn. Against a local Forge that takes milliseconds, so treating it as
 * settlement made the "Steering" label flash below perceptibility. The second
 * is the engine actually folding the steer into its turn, which is what the
 * user is waiting to see — and which shows up as the first non-user item any
 * run projects after the steering turn's own ordinal.
 */
export const ConversationSteeringAcknowledged = (
	items: ReadonlyArray<ConversationSteeringItem>,
	turns: ReadonlyArray<ConversationSteeringTurn>,
	source_reference: string,
): boolean => {
	const steer = items.find(
		(item) => item.type === "user_message" && has_source_reference(item, source_reference),
	);
	if (steer === undefined) return false;
	/**
	 * A message that never bound to a run is not a steer, so there is no engine
	 * to wait on and the label must not hang open. Neither may it hang when the
	 * turn ends without the engine ever producing anything: a run that settles
	 * has answered the steer, however unsatisfyingly.
	 */
	const run_id = steer.run_id;
	if (run_id === undefined) return true;
	/**
	 * Not the steering message's own turn — that is the user's, and it completes
	 * the instant the message lands. The run is done only when every turn bound
	 * to it has settled.
	 */
	const run_turns = turns.filter((candidate) => candidate.run_id === run_id);
	if (
		run_turns.length > 0 &&
		run_turns.every((candidate) => settled_lifecycles.has(candidate.lifecycle))
	)
		return true;
	/**
	 * Any engine work projected past the steer answers it, whichever run it
	 * belongs to. Requiring the steer's own run here left the label hanging when
	 * that run died unsettled — orphaned by a host restart — and a fresh run
	 * carried on instead: the reaction was on screen below the steer while this
	 * predicate waited on a run that could no longer produce anything. Later
	 * user messages stay excluded; another queued message is not a reaction.
	 */
	return items.some((item) => item.ordinal > steer.ordinal && item.type !== "user_message");
};
