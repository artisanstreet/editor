import { Effect, Layer, Ref } from "effect";
import { ArtisanClient } from "@artisan/transport/client";

export * from "./data";
import { FixtureClientCommands } from "./commands";
import { FixtureClientPolicies } from "./policies";
import { FixtureClientQueries, MakeFixtureClientQueries } from "./queries";
import { FixtureInteractiveInstallationReports } from "./project-identity-queries";

/** Complete deterministic Artisan client service used only by fixture compositions. */
export const FixtureArtisanClientService = {
	...FixtureClientQueries,
	...FixtureClientCommands,
	...FixtureClientPolicies,
} satisfies typeof ArtisanClient.Service;

/** Explicit test/visual Layer; production bootstraps must supply the live client Layer. */
export const FixtureArtisanClientLayer = Layer.effect(
	ArtisanClient,
	Effect.gen(function* () {
		const installation_reports = yield* Ref.make(FixtureInteractiveInstallationReports);
		return ArtisanClient.of({
			...MakeFixtureClientQueries(installation_reports),
			...FixtureClientCommands,
			...FixtureClientPolicies,
		});
	}),
);
