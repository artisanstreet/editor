import { Effect } from "effect";
import { ArtisanClient } from "@artisan/transport/client";

import * as FixtureData from "./data";
import { FixtureReceipt } from "./support";

void FixtureData;

export const FixtureClientPolicies = {
	UpdateModelBehaviour: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-model-behaviour-update");
		}),
	UpdateThreadSessionPolicy: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-session-policy-update");
		}),
	UpdateThreadRetentionPolicy: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-retention-update");
		}),
} satisfies Pick<
	typeof ArtisanClient.Service,
	"UpdateModelBehaviour" | "UpdateThreadSessionPolicy" | "UpdateThreadRetentionPolicy"
>;
