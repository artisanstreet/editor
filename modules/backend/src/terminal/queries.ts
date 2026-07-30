import { and, asc, eq, inArray, ne } from "drizzle-orm";
import { Context, Effect, Layer } from "effect";

import type { TerminalSession } from "@artisan/protocol";

import { Database } from "../persistence/database";
import { PreviewTargets, TerminalSessions } from "../persistence/tables";
import {
	DecodeStoredSession,
	NormalizeTerminalError,
	TerminalNotFound,
	type StoredTerminalSession,
	type TerminalRepositoryError,
} from "./model";

export class TerminalQueries extends Context.Service<
	TerminalQueries,
	{
		readonly List: (
			thread_id: string,
			workspace_id: string,
		) => Effect.Effect<ReadonlyArray<TerminalSession>, TerminalRepositoryError>;
		readonly ReadStale: (
			instance_id: string,
		) => Effect.Effect<ReadonlyArray<StoredTerminalSession>, TerminalRepositoryError>;
		readonly ReadOwned: (
			terminal_id: string,
			thread_id: string,
		) => Effect.Effect<StoredTerminalSession, TerminalRepositoryError>;
	}
>()("Artisan/TerminalQueries") {}

export const TerminalQueriesLive = Layer.effect(
	TerminalQueries,
	Effect.gen(function* () {
		const database = yield* Database;

		const ReadOwned = (terminal_id: string, thread_id: string) =>
			database.client
				.select()
				.from(TerminalSessions)
				.where(
					and(
						eq(TerminalSessions.terminal_id, terminal_id),
						eq(TerminalSessions.thread_id, thread_id),
					),
				)
				.limit(1)
				.pipe(
					Effect.flatMap(([row]) =>
						Effect.gen(function* () {
							if (!row) {
								return yield* new TerminalNotFound({ terminal_id });
							}

							return yield* DecodeStoredSession(row);
						}),
					),
					Effect.mapError(NormalizeTerminalError),
				);

		const ReadStale = (instance_id: string) =>
			database.client
				.select()
				.from(TerminalSessions)
				.where(
					and(
						inArray(TerminalSessions.state, ["opening", "active"]),
						ne(TerminalSessions.owner_instance_id, instance_id),
					),
				)
				.orderBy(asc(TerminalSessions.created_at), asc(TerminalSessions.terminal_id))
				.pipe(
					Effect.flatMap((rows) => Effect.forEach(rows, DecodeStoredSession)),
					Effect.mapError(NormalizeTerminalError),
				);

		const List = (thread_id: string, workspace_id: string) =>
			database.client
				.select()
				.from(TerminalSessions)
				.where(
					and(
						eq(TerminalSessions.thread_id, thread_id),
						eq(TerminalSessions.workspace_id, workspace_id),
					),
				)
				.orderBy(asc(TerminalSessions.created_at), asc(TerminalSessions.terminal_id))
				.pipe(
					Effect.flatMap((rows) =>
						Effect.gen(function* () {
							const stored = yield* Effect.forEach(rows, DecodeStoredSession);
							const terminal_ids = stored.map(({ terminal }) => terminal.terminal_id);
							const previews =
								terminal_ids.length === 0
									? []
									: yield* database.client
											.select({
												port: PreviewTargets.port,
												source_id: PreviewTargets.source_id,
												state: PreviewTargets.state,
												target_id: PreviewTargets.target_id,
												url: PreviewTargets.url,
											})
											.from(PreviewTargets)
											.where(
												and(
													eq(PreviewTargets.thread_id, thread_id),
													eq(PreviewTargets.workspace_id, workspace_id),
													eq(PreviewTargets.source_kind, "terminal"),
													inArray(PreviewTargets.source_id, terminal_ids),
												),
											);

							return stored.map(({ terminal }) => ({
								...terminal,
								associated_previews: previews
									.filter((preview) => preview.source_id === terminal.terminal_id)
									.map(({ port, state, target_id, url }) => ({
										port,
										state,
										target_id,
										url,
									})),
							}));
						}),
					),
					Effect.mapError(NormalizeTerminalError),
				);

		return { List, ReadOwned, ReadStale };
	}),
);
