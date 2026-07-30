import { eq } from "drizzle-orm";
import { Context, Effect, Layer, Schema } from "effect";

import { Database } from "../persistence/database";
import {
	EventStreams,
	JournalEvents,
	PreviewCommands,
	PreviewTargets,
	ThreadErasureClaims,
	Threads,
} from "../persistence/tables";
import { JournalNotifier } from "../persistence/journal-notifier";
import { RuntimeMetadata } from "../runtime/metadata";
import {
	type PreviewRegisterCommand,
	PreviewRepositoryError,
	PreviewRoutes,
	PreviewSource,
	type PreviewTargetProjection,
} from "./contracts";
import { DecodeTarget, TargetEventPayload } from "./storage-codec";
import { ValidatePreviewRegistrationPort } from "./validation";

export class PreviewRegistration extends Context.Service<
	PreviewRegistration,
	{
		readonly Register: (
			input: PreviewRegisterCommand,
		) => Effect.Effect<PreviewTargetProjection, PreviewRepositoryError>;
	}
>()("Artisan/PreviewRegistration") {}

const RequireSequence = (event: { readonly sequence: number } | undefined, message: string) =>
	event === undefined
		? Effect.fail(new PreviewRepositoryError({ code: "storage", message }))
		: Effect.succeed(event.sequence);

export const PreviewRegistrationLive = Layer.effect(
	PreviewRegistration,
	Effect.gen(function* () {
		const database = yield* Database;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		return {
			Register: (input) =>
				Effect.try({
					try: () => ({
						routes: Schema.decodeUnknownSync(PreviewRoutes)(input.routes ?? []),
						source:
							input.source === undefined
								? undefined
								: Schema.decodeUnknownSync(PreviewSource)(input.source),
					}),
					catch: () =>
						new PreviewRepositoryError({
							code: "invalid",
							message: "Preview routes or source are invalid",
						}),
				}).pipe(
					Effect.flatMap(({ routes, source }) =>
						ValidatePreviewRegistrationPort(input.url, input.port).pipe(
							Effect.flatMap((url) =>
								database.client.transaction((transaction) =>
									Effect.gen(function* () {
										const [command] = yield* transaction
											.select()
											.from(PreviewCommands)
											.where(eq(PreviewCommands.message_id, input.message_id))
											.limit(1);
										const payload_json = JSON.stringify(input);
										if (command !== undefined) {
											if (
												command.action !== "register" ||
												command.thread_id !== input.thread_id ||
												command.payload_json !== payload_json
											)
												return yield* Effect.fail(
													new PreviewRepositoryError({
														code: "invalid",
														message:
															"Preview command ID conflicts with prior intent",
													}),
												);
											const [existing] = yield* transaction
												.select()
												.from(PreviewTargets)
												.where(
													eq(PreviewTargets.target_id, input.target_id),
												)
												.limit(1);
											return existing === undefined
												? yield* Effect.fail(
														new PreviewRepositoryError({
															code: "storage",
															message:
																"Preview command has no target projection",
														}),
													)
												: yield* DecodeTarget(existing);
										}

										const [[thread], [erasing], [duplicate]] =
											yield* Effect.all([
												transaction
													.select({ thread_id: Threads.thread_id })
													.from(Threads)
													.where(eq(Threads.thread_id, input.thread_id))
													.limit(1),
												transaction
													.select({
														thread_id: ThreadErasureClaims.thread_id,
													})
													.from(ThreadErasureClaims)
													.where(
														eq(
															ThreadErasureClaims.thread_id,
															input.thread_id,
														),
													)
													.limit(1),
												transaction
													.select({ target_id: PreviewTargets.target_id })
													.from(PreviewTargets)
													.where(
														eq(
															PreviewTargets.target_id,
															input.target_id,
														),
													)
													.limit(1),
											]);
										if (thread === undefined || erasing !== undefined)
											return yield* Effect.fail(
												new PreviewRepositoryError({
													code: "not_found",
													message:
														"Thread is unavailable for preview mutation",
												}),
											);
										if (duplicate !== undefined)
											return yield* Effect.fail(
												new PreviewRepositoryError({
													code: "invalid",
													message: "Preview target already exists",
												}),
											);

										const now = yield* metadata.Now;
										const stream_id = `thread:${input.thread_id}`;
										const [stream] = yield* transaction
											.select()
											.from(EventStreams)
											.where(eq(EventStreams.stream_id, stream_id))
											.limit(1);
										const stream_sequence = (stream?.last_sequence ?? 0) + 1;
										yield* stream === undefined
											? transaction.insert(EventStreams).values({
													stream_id,
													last_sequence: stream_sequence,
												})
											: transaction
													.update(EventStreams)
													.set({ last_sequence: stream_sequence })
													.where(eq(EventStreams.stream_id, stream_id));

										const event_id = yield* metadata.MakeId("event");
										const [event] = yield* transaction
											.insert(JournalEvents)
											.values({
												agent_id: null,
												causation_id: input.message_id,
												correlation_id: input.message_id,
												event_id,
												event_type: "preview.target.updated",
												occurred_at: now,
												origin: "backend",
												payload_json: "{}",
												raw_origin_json: null,
												run_id: null,
												schema_version: 1,
												stream_id,
												stream_sequence,
												thread_id: input.thread_id,
											})
											.returning({ sequence: JournalEvents.sequence });
										const journal_sequence = yield* RequireSequence(
											event,
											"Preview registration event was not stored",
										);
										yield* transaction.insert(PreviewTargets).values({
											created_at: now,
											health_json: null,
											journal_sequence,
											last_error: null,
											launch_state: "idle",
											port: input.port,
											project_id: input.project_id,
											removed_at: null,
											routes_json: JSON.stringify(routes),
											source_id:
												source?.kind === "process"
													? source.process_id
													: source?.kind === "terminal"
														? source.terminal_id
														: null,
											source_kind: source?.kind ?? null,
											state: "registered",
											target_id: input.target_id,
											thread_id: input.thread_id,
											updated_at: now,
											url,
											workspace_id: input.workspace_id,
										});
										yield* transaction.insert(PreviewCommands).values({
											action: "register",
											created_at: now,
											journal_sequence,
											message_id: input.message_id,
											payload_json,
											thread_id: input.thread_id,
										});
										const [stored] = yield* transaction
											.select()
											.from(PreviewTargets)
											.where(eq(PreviewTargets.target_id, input.target_id))
											.limit(1);
										if (stored === undefined)
											return yield* Effect.fail(
												new PreviewRepositoryError({
													code: "storage",
													message:
														"Preview target projection was not stored",
												}),
											);
										yield* transaction
											.update(JournalEvents)
											.set({
												payload_json: JSON.stringify(
													TargetEventPayload(stored),
												),
											})
											.where(eq(JournalEvents.sequence, journal_sequence));
										return yield* DecodeTarget(stored);
									}),
								),
							),
						),
					),
					Effect.mapError((error) =>
						error instanceof PreviewRepositoryError
							? error
							: new PreviewRepositoryError({
									code: "storage",
									message: "Could not register preview target",
								}),
					),
					Effect.tap((target) => notifier.Publish(target.journal_sequence)),
				),
		};
	}),
);
