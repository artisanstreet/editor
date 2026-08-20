import { Context, Effect, Layer, Schema } from "effect";

import { DefaultAgentNameDatasetId, type AgentNameDatasetId } from "@artisan/protocol";
import british from "@artisan/data/names/british-females.json" with { type: "json" };
import norwegian from "@artisan/data/names/norwegian-females.json" with { type: "json" };

import { SessionDefaultsService } from "../settings/session-defaults-service";
import type { JournalStoreError } from "../persistence/journal-store";

const NameList = Schema.Array(Schema.NonEmptyString);

/** Validates each bundled bank once and reads the durable selection for new identities. */
export class AgentNameCatalog extends Context.Service<
	AgentNameCatalog,
	{ readonly Names: Effect.Effect<ReadonlyArray<string>, JournalStoreError> }
>()("Artisan/AgentNameCatalog") {}

export const AgentNameCatalogLive = Layer.effect(
	AgentNameCatalog,
	Effect.gen(function* () {
		const defaults = yield* SessionDefaultsService;
		const default_names = yield* Schema.decodeUnknownEffect(NameList)(norwegian);
		const datasets = new Map<AgentNameDatasetId, ReadonlyArray<string>>([
			["norwegian", default_names],
			["british", yield* Schema.decodeUnknownEffect(NameList)(british)],
		]);
		const Names = Effect.gen(function* () {
			const selected = (yield* defaults.Read).agent_name_dataset ?? DefaultAgentNameDatasetId;
			return datasets.get(selected) ?? default_names;
		});
		return AgentNameCatalog.of({ Names });
	}),
);
