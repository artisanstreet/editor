import { Effect, Layer } from "effect";
import { describe, expect, it } from "vitest";

import type { InboundControlEnvelope, OutboundControlEnvelope } from "@artisan/protocol";

import { marketplace_routine_thread_id } from "../../modules/backend/src/marketplace/routines/repository";
import { JournalStore } from "../../modules/backend/src/persistence/journal-store";
import { MakeMarketplaceResponse } from "../../modules/backend/src/protocol/rpc/mutation-handlers/marketplace-response";
import { ConnectionResponseSink } from "../../modules/backend/src/protocol/rpc/query-handlers/project";
import { RuntimeMetadata } from "../../modules/backend/src/runtime/metadata";

const envelope = {
	kind: "marketplace.routine.install.decision",
	message_id: "marketplace_action",
	origin: "frontend",
	payload: {},
	protocol_version: 1,
	schema_version: 1,
	sent_at: "2026-08-15T00:00:00.000Z",
} as Extract<InboundControlEnvelope, { readonly kind: `marketplace.${string}` }>;

const MetadataLive = Layer.succeed(RuntimeMetadata, {
	instance_id: "marketplace_response_performance",
	MakeId: (prefix) => Effect.succeed(`${prefix}_marketplace_response`),
	Now: Effect.succeed("2026-08-15T00:00:00.000Z"),
});

describe("Marketplace action receipts", () => {
	it("uses one watermark read after an accepted action without replaying journal history", async () => {
		const calls = { program: 0, replay: 0, watermark: 0 };
		const output: Array<OutboundControlEnvelope> = [];
		const JournalLive = Layer.succeed(JournalStore, {
			ReadReplay: () =>
				Effect.sync(() => {
					calls.replay += 1;
					return [];
				}),
			ReadWatermark: () =>
				Effect.sync(() => {
					calls.watermark += 1;
					return 731;
				}),
		} as never);
		const SinkLive = Layer.succeed(ConnectionResponseSink, {
			Enqueue: (response: OutboundControlEnvelope) =>
				Effect.sync(() => {
					output.push(response);
				}),
			EnqueueError: () => Effect.void,
		});

		await Effect.gen(function* () {
			const response = yield* MakeMarketplaceResponse;
			yield* response.Action(
				envelope,
				Effect.sync(() => {
					calls.program += 1;
				}),
			);
		})
			.pipe(Effect.provide(Layer.mergeAll(JournalLive, MetadataLive, SinkLive)))
			.pipe(Effect.runPromise);

		expect(calls).toEqual({ program: 1, replay: 0, watermark: 1 });
		expect(output).toHaveLength(1);
		expect(output[0]).toMatchObject({
			correlation_id: "marketplace_action",
			kind: "command.receipt",
			payload: { journal_sequence: 731, status: "accepted" },
			thread_id: marketplace_routine_thread_id,
		});
	});

	it("rejects failed actions without reading a watermark or replay", async () => {
		const calls = { program: 0, replay: 0, watermark: 0 };
		const output: Array<OutboundControlEnvelope> = [];
		const JournalLive = Layer.succeed(JournalStore, {
			ReadReplay: () =>
				Effect.sync(() => {
					calls.replay += 1;
					return [];
				}),
			ReadWatermark: () =>
				Effect.sync(() => {
					calls.watermark += 1;
					return 731;
				}),
		} as never);
		const SinkLive = Layer.succeed(ConnectionResponseSink, {
			Enqueue: (response: OutboundControlEnvelope) =>
				Effect.sync(() => {
					output.push(response);
				}),
			EnqueueError: () => Effect.void,
		});

		await Effect.gen(function* () {
			const response = yield* MakeMarketplaceResponse;
			yield* response.Action(
				envelope,
				Effect.sync(() => {
					calls.program += 1;
				}).pipe(Effect.andThen(Effect.fail(new Error("rejected")))),
			);
		})
			.pipe(Effect.provide(Layer.mergeAll(JournalLive, MetadataLive, SinkLive)))
			.pipe(Effect.runPromise);

		expect(calls).toEqual({ program: 1, replay: 0, watermark: 0 });
		expect(output).toHaveLength(1);
		expect(output[0]).toMatchObject({
			correlation_id: "marketplace_action",
			kind: "command.receipt",
			payload: {
				error: { code: "marketplace.action_rejected" },
				status: "rejected",
			},
			thread_id: marketplace_routine_thread_id,
		});
	});
});
