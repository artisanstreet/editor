import { Layer } from "effect";
import { ArtisanClient } from "@artisan/transport/client";

export * from "./data";
import { FixtureClientCommands } from "./commands";
import { FixtureClientPolicies } from "./policies";
import { FixtureClientQueries } from "./queries";

/** Complete deterministic Artisan client service used only by fixture compositions. */
export const FixtureArtisanClientService = {
	...FixtureClientQueries,
	...FixtureClientCommands,
	...FixtureClientPolicies,
} satisfies typeof ArtisanClient.Service;

/** Explicit test/visual Layer; production bootstraps must supply the live client Layer. */
export const FixtureArtisanClientLayer = Layer.succeed(ArtisanClient, FixtureArtisanClientService);
