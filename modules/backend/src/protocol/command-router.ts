import { Context, Effect, Layer } from "effect";

import type { CommandEnvelope, OutboundEnvelope } from "@artisan/protocol";

import type { JournalStoreError } from "../persistence/journal-store";
import { ThreadCommands } from "../threads/thread-commands";

export class CommandRouter extends Context.Service<
	CommandRouter,
	{
		readonly Dispatch: (
			command: CommandEnvelope,
		) => Effect.Effect<ReadonlyArray<OutboundEnvelope>, JournalStoreError>;
	}
>()("Artisan/CommandRouter") {}

export const CommandRouterLive = Layer.effect(
	CommandRouter,
	Effect.gen(function* () {
		const thread_commands = yield* ThreadCommands;

		return {
			Dispatch: thread_commands.HandleCreate,
		};
	}),
);
