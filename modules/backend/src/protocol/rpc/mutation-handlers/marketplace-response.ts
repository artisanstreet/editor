import { Effect, Schema } from "effect";

import { OutboundControlEnvelope, type InboundControlEnvelope } from "@artisan/protocol";

import { marketplace_capability_thread_id } from "../../../marketplace/capabilities/repository";
import { marketplace_routine_thread_id } from "../../../marketplace/routines/repository";
import { JournalStore } from "../../../persistence/journal-store";
import { RuntimeMetadata } from "../../../runtime/metadata";
import type { ReadyState } from "../../connection-state";
import { ConnectionResponseSink } from "../query-handlers/project";

type MarketplaceEnvelope = Extract<
	InboundControlEnvelope,
	{ readonly kind: `marketplace.${string}` }
>;

export const MakeMarketplaceResponse = Effect.gen(function* () {
	const journal = yield* JournalStore;
	const metadata = yield* RuntimeMetadata;
	const sink = yield* ConnectionResponseSink;

	const Result = <Payload>(
		envelope: MarketplaceEnvelope,
		current: ReadyState,
		kind:
			| "marketplace.routine.invoke.result"
			| "marketplace.capability.invoke.result"
			| "marketplace.capability.oauth.begin.result",
		program: Effect.Effect<Payload, unknown>,
	) =>
		program.pipe(
			Effect.flatMap((payload) =>
				Effect.gen(function* () {
					const candidate: unknown = {
						correlation_id: envelope.message_id,
						kind,
						message_id: yield* metadata.MakeId("message"),
						origin: "backend",
						payload,
						protocol_version: 1,
						schema_version: 1,
						sent_at: yield* metadata.Now,
					};
					const response =
						yield* Schema.decodeUnknownEffect(OutboundControlEnvelope)(candidate);
					yield* sink.Enqueue(response);
				}),
			),
			Effect.catch(() =>
				sink.EnqueueError(
					current,
					"marketplace.unavailable",
					"The Marketplace operation could not be completed.",
					true,
					envelope.message_id,
				),
			),
		);

	const Action = (envelope: MarketplaceEnvelope, program: Effect.Effect<unknown, unknown>) =>
		program.pipe(
			Effect.andThen(journal.ReadWatermark()),
			Effect.flatMap((journal_sequence) =>
				Effect.gen(function* () {
					yield* sink.Enqueue({
						causation_id: envelope.message_id,
						correlation_id: envelope.message_id,
						kind: "command.receipt",
						message_id: yield* metadata.MakeId("message"),
						origin: "backend",
						payload: {
							journal_sequence,
							status: "accepted",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: yield* metadata.Now,
						thread_id: envelope.kind.includes("capability")
							? marketplace_capability_thread_id
							: marketplace_routine_thread_id,
					});
				}),
			),
			Effect.catch(() =>
				Effect.gen(function* () {
					yield* sink.Enqueue({
						causation_id: envelope.message_id,
						correlation_id: envelope.message_id,
						kind: "command.receipt",
						message_id: yield* metadata.MakeId("message"),
						origin: "backend",
						payload: {
							error: {
								code: "marketplace.action_rejected",
								message: "The Marketplace action was rejected before completion.",
								retryable: false,
							},
							status: "rejected",
						},
						protocol_version: 1,
						schema_version: 1,
						sent_at: yield* metadata.Now,
						thread_id: envelope.kind.includes("capability")
							? marketplace_capability_thread_id
							: marketplace_routine_thread_id,
					});
				}),
			),
		);

	const Reject = (envelope: MarketplaceEnvelope, current: ReadyState, message: string) =>
		sink.EnqueueError(
			current,
			"marketplace.action_rejected",
			message,
			false,
			envelope.message_id,
		);

	return { Action, Reject, Result };
});
