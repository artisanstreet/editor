import { and, eq } from "drizzle-orm";
import { Context, Effect, Layer } from "effect";

import { Database, type DatabaseClient } from "../persistence/database";
import { TerminalCommands } from "../persistence/tables";
import { RuntimeMetadata } from "../runtime/metadata";
import {
	NormalizeTerminalError,
	TerminalInvariantError,
	type TerminalRepositoryError,
} from "./model";

interface CompletionInput {
	readonly failure?: string;
	readonly generation: number;
	readonly journal_sequence: number;
	readonly message_id: string;
	readonly status: "completed" | "failed";
}

type TerminalTransaction = DatabaseClient;

export class TerminalCompletion extends Context.Service<
	TerminalCompletion,
	{
		readonly CompleteClaim: (
			transaction: TerminalTransaction,
			input: CompletionInput,
		) => Effect.Effect<void, unknown>;
		readonly CompleteCommand: (
			input: CompletionInput,
		) => Effect.Effect<void, TerminalRepositoryError>;
	}
>()("Artisan/TerminalCompletion") {}

export const TerminalCompletionLive = Layer.effect(
	TerminalCompletion,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;

		const CompleteClaim = (transaction: TerminalTransaction, input: CompletionInput) =>
			Effect.gen(function* () {
				const updated = yield* transaction
					.update(TerminalCommands)
					.set({
						failure: input.failure ?? null,
						journal_sequence: input.journal_sequence,
						status: input.status,
						updated_at: yield* metadata.Now,
					})
					.where(
						and(
							eq(TerminalCommands.message_id, input.message_id),
							eq(TerminalCommands.generation, input.generation),
							eq(TerminalCommands.status, "dispatching"),
						),
					)
					.returning({ message_id: TerminalCommands.message_id });

				if (updated.length !== 1) {
					return yield* new TerminalInvariantError({
						message: `Terminal command ${input.message_id} completion did not update its dispatching claim`,
					});
				}
			});

		const CompleteCommand = (input: CompletionInput) =>
			database.client
				.transaction((transaction) => CompleteClaim(transaction, input))
				.pipe(Effect.mapError(NormalizeTerminalError));

		return { CompleteClaim, CompleteCommand };
	}),
);
