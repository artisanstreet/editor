import { and, eq, isNull, lte, ne, or, sql } from "drizzle-orm";
import { Context, Effect, Layer } from "effect";

import { EngineRegistry } from "@artisan/engines";
import type {
	CommandEnvelope,
	EventEnvelope,
	UsageInterruption,
	UsageInterruptionAlternative,
} from "@artisan/protocol";

import { RuntimeCatalogService } from "../../runtime/catalog";
import { RuntimeMetadata } from "../../runtime/metadata";
import { Database, type DatabaseClient } from "../database";
import { AppendJournalEventInTransaction, CommandIdConflict, JournalStore } from "../journal-store";
import { JournalNotifier } from "../journal-notifier";
import {
	JournalCommands,
	OrchestrationCoordinators,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRuns,
	ThreadErasureClaims,
	ThreadTombstones,
	Threads,
	UsageInterruptions,
} from "../tables";
import {
	OrchestrationCommandConflict,
	OrchestrationFailure,
	type AcceptedOrchestrationCommand,
	type OrchestrationError,
} from "../orchestration/contracts";
import { ReconcileRootThreadLiveStatus } from "../orchestration/thread-lifecycle-status";
import { DecodeUsageInterruptionRow } from "./model";

export const usage_interruption_continuation_text =
	"Continue the interrupted task from the last durable state. Re-check current workspace state before repeating any side effect.";

const normalize_label = (value: string) =>
	value
		.toLowerCase()
		.replace(/\b(claude|codex|code|model)\b/g, "")
		.replace(/[^a-z]+/g, " ")
		.trim();

const normalize_bucket_id = (value: string) => value.toLowerCase().replace(/[^a-z0-9]+/g, "");

interface UsageEvidence {
	readonly alternatives: ReadonlyArray<UsageInterruptionAlternative>;
	readonly matching_reset: string | undefined;
	readonly source_affected_model_id?: string;
	readonly source_available: boolean;
	readonly source_limit_label?: string;
	readonly source_limit_scope?: UsageInterruption["limit_scope"];
}

const NextUsagePoll = (now: string) => new Date(Date.parse(now) + 5 * 60 * 1_000).toISOString();

const command_matches = (command: CommandEnvelope, row: typeof JournalCommands.$inferSelect) =>
	row.payload_json === JSON.stringify(command.payload) &&
	row.thread_id === command.thread_id &&
	row.run_id === (command.run_id ?? null) &&
	row.agent_id === (command.agent_id ?? null) &&
	row.causation_id === (command.causation_id ?? null) &&
	row.origin === command.origin &&
	row.raw_origin_json === (command.raw_origin ? JSON.stringify(command.raw_origin) : null) &&
	row.schema_version === command.schema_version &&
	row.sent_at === command.sent_at;

/** Owns decisions and restart-safe launches for durable usage interruptions. */
export class UsageInterruptionService extends Context.Service<
	UsageInterruptionService,
	{
		readonly MarkTargetContinued: (
			target_run_id: string,
		) => Effect.Effect<void, OrchestrationError>;
		readonly MarkTargetFailed: (
			target_run_id: string,
		) => Effect.Effect<void, OrchestrationError>;
		readonly CancelSuperseded: (
			thread_id: string,
			active_run_id: string,
		) => Effect.Effect<void, OrchestrationError>;
		readonly RefreshPendingEvidence: Effect.Effect<void, OrchestrationError>;
		readonly Resolve: (
			command: CommandEnvelope,
		) => Effect.Effect<AcceptedOrchestrationCommand, OrchestrationError>;
		readonly ScanDue: Effect.Effect<boolean, OrchestrationError>;
	}
>()("Artisan/UsageInterruptionService") {}

export const UsageInterruptionServiceLive = Layer.effect(
	UsageInterruptionService,
	Effect.gen(function* () {
		const database = yield* Database;
		const engines = yield* EngineRegistry;
		const catalog_service = yield* RuntimeCatalogService;
		const catalog = yield* catalog_service.Get;
		const journal = yield* JournalStore;
		const metadata = yield* RuntimeMetadata;
		const notifier = yield* JournalNotifier;

		const RefreshEvidence = (interruption: UsageInterruption): Effect.Effect<UsageEvidence> =>
			Effect.gen(function* () {
				const engine = yield* engines.Get(interruption.source_engine_id);
				if (engine.Usage === undefined)
					return {
						alternatives: [],
						matching_reset: undefined,
						source_available: false,
					} satisfies UsageEvidence;
				const usage = yield* engine.Usage;
				const shared_windows = usage.windows.filter((window) => window.scope === "shared");
				const shared_depleted = shared_windows.some(
					(window) => window.percent_used >= 100 && window.scope === "shared",
				);
				const non_model_windows = usage.windows.filter(
					(window) => window.scope !== "model",
				);
				const matching_source_window =
					interruption.limit_id !== undefined
						? usage.windows.find(
								(window) =>
									normalize_bucket_id(window.id) ===
									normalize_bucket_id(interruption.limit_id ?? ""),
							)
						: interruption.limit_scope === "model"
							? usage.windows.find(
									(window) =>
										window.scope === "model" &&
										window.label !== undefined &&
										normalize_label(window.label) ===
											normalize_label(
												interruption.limit_label ??
													interruption.affected_model_id ??
													interruption.source_model_id ??
													"",
											).trim(),
								)
							: undefined;
				const source_available =
					!shared_depleted &&
					(interruption.limit_id === undefined && interruption.limit_scope !== "model"
						? non_model_windows.length > 0 &&
							non_model_windows.every((window) => window.percent_used < 100)
						: matching_source_window !== undefined &&
							matching_source_window.percent_used < 100);
				const matching_reset =
					interruption.limit_id === undefined && interruption.limit_scope !== "model"
						? non_model_windows
								.filter((window) => window.percent_used >= 100)
								.map((window) => window.resets_at)
								.filter((reset): reset is string => reset !== undefined)
								.sort()
								.at(-1)
						: matching_source_window?.resets_at;
				const source_limit_scope =
					interruption.limit_scope === "shared"
						? undefined
						: matching_source_window?.scope;
				const source_limit_label = matching_source_window?.label;
				const source_bucket_models =
					source_limit_scope !== "model" || source_limit_label === undefined
						? []
						: catalog.manifest.models.filter((model) => {
								if (
									model.harness !== interruption.source_engine_id ||
									model.disabled !== undefined
								)
									return false;
								const label = normalize_label(source_limit_label);
								return (
									label.length >= 3 &&
									(normalize_label(model.name).includes(label) ||
										normalize_label(model.native_model_id).includes(label))
								);
							});
				const source_affected_model_id =
					source_bucket_models.length === 1 &&
					source_bucket_models[0]?.native_model_id === interruption.source_model_id
						? interruption.source_model_id
						: undefined;
				const source_refinement = {
					...(source_affected_model_id === undefined ? {} : { source_affected_model_id }),
					...(source_limit_label === undefined ? {} : { source_limit_label }),
					...(source_limit_scope === undefined ? {} : { source_limit_scope }),
				};
				const verified_at = yield* metadata.Now;
				const alternatives: Array<UsageInterruptionAlternative> = [];
				if (interruption.limit_scope === "shared" || shared_depleted)
					return {
						alternatives,
						matching_reset,
						...source_refinement,
						source_available,
					} as const;
				for (const window of usage.windows) {
					if (
						window.scope !== "model" ||
						window.label === undefined ||
						window.percent_used >= 100
					)
						continue;
					const label = normalize_label(window.label);
					const matches = catalog.manifest.models.filter((model) => {
						if (
							model.harness !== interruption.source_engine_id ||
							model.disabled !== undefined
						)
							return false;
						const native = normalize_label(model.native_model_id);
						const name = normalize_label(model.name);
						return (
							label.length >= 3 && (name.includes(label) || native.includes(label))
						);
					});
					if (matches.length !== 1) continue;
					const model = matches[0];
					if (
						model === undefined ||
						model.native_model_id === interruption.source_model_id ||
						alternatives.some((item) => item.model_id === model.native_model_id)
					)
						continue;
					alternatives.push({
						display_name: model.name,
						engine_id: model.harness,
						model_id: model.native_model_id,
						verified_at,
					});
					if (alternatives.length >= 16) break;
				}
				const source_model_bucket_proven =
					matching_source_window?.scope === "model" &&
					matching_source_window.percent_used >= 100;
				if (
					source_model_bucket_proven &&
					shared_windows.length > 0 &&
					shared_windows.every((window) => window.percent_used < 100)
				) {
					const depleted_model_labels = usage.windows
						.filter(
							(window) =>
								window.scope === "model" &&
								window.percent_used >= 100 &&
								window.label !== undefined,
						)
						.map((window) => normalize_label(window.label ?? ""));
					for (const model of catalog.manifest.models) {
						if (
							model.harness !== interruption.source_engine_id ||
							model.disabled !== undefined ||
							model.native_model_id === interruption.source_model_id ||
							alternatives.some((item) => item.model_id === model.native_model_id)
						)
							continue;
						const native = normalize_label(model.native_model_id);
						const name = normalize_label(model.name);
						if (
							depleted_model_labels.some(
								(label) =>
									label.length >= 3 &&
									(name.includes(label) || native.includes(label)),
							)
						)
							continue;
						alternatives.push({
							display_name: model.name,
							engine_id: model.harness,
							model_id: model.native_model_id,
							verified_at,
						});
						if (alternatives.length >= 16) break;
					}
				}
				return {
					alternatives,
					matching_reset,
					...source_refinement,
					source_available,
				} as const;
			}).pipe(
				Effect.catch(() =>
					Effect.succeed({
						alternatives: [],
						matching_reset: undefined,
						source_available: false,
					} satisfies UsageEvidence),
				),
			);

		const AppendUpdated = (
			transaction: DatabaseClient,
			interruption: UsageInterruption,
			causation_id: string,
		) =>
			AppendJournalEventInTransaction(transaction, metadata, {
				agent_id: interruption.source_agent_id,
				causation_id,
				correlation_id: causation_id,
				payload: { interruption, type: "usage.interruption.updated" },
				run_id: interruption.target_run_id ?? interruption.source_run_id,
				thread_id: interruption.thread_id,
			});

		const TransitionTarget = (target_run_id: string, state: "continued" | "failed") =>
			Effect.gen(function* () {
				const now = yield* metadata.Now;
				const event = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [updated] = yield* transaction
							.update(UsageInterruptions)
							.set({
								...(state === "continued"
									? { continued_at: now }
									: { failed_at: now }),
								revision: sql`${UsageInterruptions.revision} + 1`,
								state,
								updated_at: now,
							})
							.where(
								and(
									eq(UsageInterruptions.target_run_id, target_run_id),
									eq(UsageInterruptions.state, "launching"),
								),
							)
							.returning();
						if (updated === undefined) return undefined;
						const interruption = yield* DecodeUsageInterruptionRow(updated);
						return yield* AppendUpdated(
							transaction,
							interruption,
							`usage-target:${target_run_id}:${state}`,
						);
					}),
				);
				if (event !== undefined) yield* notifier.Publish(event.journal_sequence);
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof OrchestrationFailure
						? cause
						: new OrchestrationFailure({ cause }),
				),
			);

		const ClaimLaunch = (
			transaction: DatabaseClient,
			row: typeof UsageInterruptions.$inferSelect,
			input: {
				readonly command_id: string;
				readonly target_engine_id: string;
				readonly target_model_id?: string;
			},
		) =>
			Effect.gen(function* () {
				const [[thread], [claim], [tombstone], [coordinator], [source]] = yield* Effect.all(
					[
						transaction
							.select()
							.from(Threads)
							.where(eq(Threads.thread_id, row.thread_id))
							.limit(1),
						transaction
							.select()
							.from(ThreadErasureClaims)
							.where(eq(ThreadErasureClaims.thread_id, row.thread_id))
							.limit(1),
						transaction
							.select()
							.from(ThreadTombstones)
							.where(eq(ThreadTombstones.thread_id, row.thread_id))
							.limit(1),
						transaction
							.select()
							.from(OrchestrationCoordinators)
							.where(eq(OrchestrationCoordinators.thread_id, row.thread_id))
							.limit(1),
						transaction
							.select()
							.from(OrchestrationRuns)
							.where(eq(OrchestrationRuns.run_id, row.source_run_id))
							.limit(1),
					],
				);
				if (!thread || claim || tombstone) {
					yield* transaction
						.delete(UsageInterruptions)
						.where(eq(UsageInterruptions.interruption_id, row.interruption_id));
					return { _tag: "discarded" as const };
				}
				if (!source || coordinator?.active_run_id !== row.source_run_id) {
					const now = yield* metadata.Now;
					const [cancelled] = yield* transaction
						.update(UsageInterruptions)
						.set({
							cancelled_at: now,
							revision: sql`${UsageInterruptions.revision} + 1`,
							state: "cancelled",
							updated_at: now,
						})
						.where(
							and(
								eq(UsageInterruptions.interruption_id, row.interruption_id),
								eq(UsageInterruptions.revision, row.revision),
							),
						)
						.returning();
					if (cancelled === undefined) return { _tag: "raced" as const };
					const interruption = yield* DecodeUsageInterruptionRow(cancelled);
					return {
						_tag: "cancelled" as const,
						event: yield* AppendUpdated(transaction, interruption, input.command_id),
						row: cancelled,
					};
				}
				if (source.status !== "failed") return { _tag: "source_pending" as const };
				const now = yield* metadata.Now;
				const run_id = yield* metadata.MakeId("run");
				const target_catalog_model =
					input.target_model_id === undefined
						? undefined
						: catalog.manifest.models.find(
								(model) =>
									model.harness === input.target_engine_id &&
									model.native_model_id === input.target_model_id,
							);
				const target_context = target_catalog_model?.capabilities.context_window;
				const default_context = target_context?.options.find(
					(option) => option.id === target_context.default,
				)?.native_suffix;
				const default_speed = target_catalog_model?.capabilities.speed_options.find(
					(option) => option.default,
				)?.native_value;
				const target_thinking = target_catalog_model?.capabilities.thinking;
				const default_reasoning =
					target_thinking?.availability === "supported"
						? target_thinking.options.find(
								(option) => option.id === target_thinking.default,
							)?.native_value
						: undefined;
				const switches_model =
					input.target_engine_id !== row.source_engine_id ||
					(input.target_model_id ?? row.source_model_id) !== row.source_model_id;
				const claimed = yield* transaction
					.update(UsageInterruptions)
					.set({
						affected_model_id: row.affected_model_id,
						alternatives_json: row.alternatives_json,
						continuation_command_id: input.command_id,
						limit_label: row.limit_label,
						limit_scope: row.limit_scope,
						revision: sql`${UsageInterruptions.revision} + 1`,
						state: "launching",
						target_engine_id: input.target_engine_id,
						target_model_id: input.target_model_id ?? null,
						target_run_id: run_id,
						updated_at: now,
					})
					.where(
						and(
							eq(UsageInterruptions.interruption_id, row.interruption_id),
							eq(UsageInterruptions.revision, row.revision),
							or(
								eq(UsageInterruptions.state, "scheduled"),
								eq(UsageInterruptions.state, "awaiting_decision"),
							),
						),
					)
					.returning();
				if (claimed.length === 0) return { _tag: "raced" as const };
				yield* transaction
					.update(OrchestrationCoordinators)
					.set({
						active_run_id: run_id,
						engine_id: input.target_engine_id,
						...(!switches_model || input.target_model_id === undefined
							? {}
							: {
									policy_context_window: default_context || null,
									policy_model: input.target_model_id,
									policy_model_id: null,
									policy_profile_id: null,
									policy_provider_route_id: null,
									policy_reasoning_effort: default_reasoning ?? "medium",
									policy_service_tier: default_speed ?? "standard",
									policy_variant_id: null,
									policy_catalog_revision: null,
								}),
						updated_at: now,
					})
					.where(eq(OrchestrationCoordinators.thread_id, row.thread_id));
				yield* transaction.insert(OrchestrationRuns).values({
					agent_id: row.source_agent_id,
					catalog_revision: switches_model ? null : source.catalog_revision,
					created_at: now,
					engine_id: input.target_engine_id,
					model_id: input.target_model_id ?? row.source_model_id,
					native_resume_json: null,
					native_thread_id: null,
					profile_id: switches_model ? null : source.profile_id,
					provider_route_id: switches_model ? null : source.provider_route_id,
					run_id,
					status: "queued",
					thread_id: row.thread_id,
					updated_at: now,
					variant_id: switches_model ? null : source.variant_id,
					working_directory: source.working_directory,
				});
				yield* ReconcileRootThreadLiveStatus(transaction, row.thread_id, now);
				yield* transaction.insert(OrchestrationMessages).values({
					agent_id: row.source_agent_id,
					command_id: input.command_id,
					created_at: now,
					delivery: "queued",
					message_id: input.command_id,
					run_id,
					text: usage_interruption_continuation_text,
					thread_id: row.thread_id,
				});
				yield* transaction.insert(OrchestrationOutbox).values({
					agent_id: row.source_agent_id,
					command_id: input.command_id,
					created_at: now,
					kind: "start",
					payload_json: JSON.stringify({
						engine_id: input.target_engine_id,
						text: usage_interruption_continuation_text,
						type: "thread.send_message",
						working_directory: source.working_directory,
					}),
					run_id,
					status: "pending",
					thread_id: row.thread_id,
					updated_at: now,
				});
				const launched = claimed[0];
				if (launched === undefined) return { _tag: "raced" as const };
				const interruption = yield* DecodeUsageInterruptionRow(launched);
				return {
					_tag: "launched" as const,
					event: yield* AppendUpdated(transaction, interruption, input.command_id),
					row: launched,
				};
			});

		const Resolve = (command: CommandEnvelope) =>
			Effect.gen(function* () {
				if (command.payload.type !== "usage.interruption.resolve")
					return yield* new OrchestrationFailure({
						cause: new Error("Invalid usage interruption command"),
					});
				const payload = command.payload;
				const action = payload.action;
				const [replayed] = yield* database.client
					.select()
					.from(JournalCommands)
					.where(eq(JournalCommands.message_id, command.message_id))
					.limit(1);
				if (replayed !== undefined) {
					if (!command_matches(command, replayed))
						return yield* new CommandIdConflict({ message_id: command.message_id });
					const [interruption] = yield* database.client
						.select()
						.from(UsageInterruptions)
						.where(eq(UsageInterruptions.interruption_id, payload.interruption_id))
						.limit(1);
					const events = yield* journal.ReadCorrelatedEvents(command.message_id);
					const run_id =
						interruption?.target_run_id ??
						interruption?.source_run_id ??
						events.at(-1)?.run_id;
					if (run_id === undefined)
						return yield* new OrchestrationFailure({
							cause: new Error("Accepted usage interruption command has no run"),
						});
					return {
						events,
						journal_sequence: events.at(-1)?.journal_sequence ?? 0,
						run_id,
						status: "duplicate" as const,
					};
				}
				const [prior] = yield* database.client
					.select()
					.from(UsageInterruptions)
					.where(eq(UsageInterruptions.interruption_id, payload.interruption_id))
					.limit(1);
				if (!prior || prior.thread_id !== command.thread_id)
					return yield* new OrchestrationFailure({
						cause: new Error("Usage interruption not found"),
					});
				const snapshot = yield* DecodeUsageInterruptionRow(prior).pipe(
					Effect.mapError((cause) => new OrchestrationFailure({ cause })),
				);
				const evidence: UsageEvidence =
					action.type === "continue"
						? yield* RefreshEvidence(snapshot)
						: {
								alternatives: snapshot.alternatives,
								matching_reset: undefined,
								source_available: false,
							};
				const alternatives = evidence.alternatives;
				if (action.type === "continue") {
					const same_source =
						action.target_engine_id === snapshot.source_engine_id &&
						(action.target_model_id ?? snapshot.source_model_id) ===
							snapshot.source_model_id;
					if (same_source && !evidence.source_available)
						return yield* new OrchestrationFailure({
							cause: new Error("The depleted allowance is not yet available"),
						});
					if (
						!same_source &&
						!alternatives.some(
							(item) =>
								item.engine_id === action.target_engine_id &&
								item.model_id === action.target_model_id,
						)
					)
						return yield* new OrchestrationFailure({
							cause: new Error(
								"The selected model does not have fresh independent usage evidence",
							),
						});
				}
				const result = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [existing] = yield* transaction
							.select()
							.from(JournalCommands)
							.where(eq(JournalCommands.message_id, command.message_id))
							.limit(1);
						if (existing) {
							if (!command_matches(command, existing))
								return yield* new CommandIdConflict({
									message_id: command.message_id,
								});
							return { duplicate: true as const };
						}
						const [current] = yield* transaction
							.select()
							.from(UsageInterruptions)
							.where(eq(UsageInterruptions.interruption_id, payload.interruption_id))
							.limit(1);
						if (!current || current.revision !== payload.expected_revision)
							return yield* new OrchestrationCommandConflict({
								message_id: command.message_id,
							});
						const accepted_at = yield* metadata.Now;
						yield* transaction.insert(JournalCommands).values({
							accepted_at,
							agent_id: command.agent_id ?? null,
							causation_id: command.causation_id ?? null,
							message_id: command.message_id,
							origin: command.origin,
							payload_json: JSON.stringify(command.payload),
							payload_type: command.payload.type,
							raw_origin_json: command.raw_origin
								? JSON.stringify(command.raw_origin)
								: null,
							run_id: command.run_id ?? null,
							schema_version: command.schema_version,
							sent_at: command.sent_at,
							status: "accepted",
							thread_id: command.thread_id,
						});
						let updated = current;
						let event: EventEnvelope | undefined;
						if (action.type === "continue") {
							const target_model_id =
								action.target_model_id ?? current.source_model_id ?? undefined;
							const outcome = yield* ClaimLaunch(
								transaction,
								{
									...current,
									affected_model_id:
										evidence.source_affected_model_id ??
										current.affected_model_id,
									alternatives_json: JSON.stringify(alternatives),
									limit_label: evidence.source_limit_label ?? current.limit_label,
									limit_scope: evidence.source_limit_scope ?? current.limit_scope,
								},
								{
									command_id: command.message_id,
									target_engine_id: action.target_engine_id,
									...(target_model_id === undefined ? {} : { target_model_id }),
								},
							);
							if (
								outcome._tag === "raced" ||
								outcome._tag === "discarded" ||
								outcome._tag === "source_pending"
							)
								return yield* new OrchestrationCommandConflict({
									message_id: command.message_id,
								});
							updated = outcome.row;
							event = outcome.event;
						} else {
							const enabled = action.type === "set_auto_continue" && action.enabled;
							const state =
								action.type === "cancel"
									? "cancelled"
									: enabled
										? "scheduled"
										: "awaiting_decision";
							const [changed] = yield* transaction
								.update(UsageInterruptions)
								.set({
									alternatives_json: JSON.stringify(alternatives),
									auto_continue:
										action.type === "set_auto_continue"
											? action.enabled
											: current.auto_continue,
									...(state === "cancelled" ? { cancelled_at: accepted_at } : {}),
									resume_not_before:
										state === "scheduled"
											? (current.resets_at ?? NextUsagePoll(accepted_at))
											: null,
									revision: sql`${UsageInterruptions.revision} + 1`,
									state,
									updated_at: accepted_at,
								})
								.where(
									and(
										eq(
											UsageInterruptions.interruption_id,
											current.interruption_id,
										),
										eq(UsageInterruptions.revision, current.revision),
									),
								)
								.returning();
							if (changed === undefined)
								return yield* new OrchestrationCommandConflict({
									message_id: command.message_id,
								});
							updated = changed;
						}
						const decoded = yield* DecodeUsageInterruptionRow(updated);
						if (event === undefined)
							event = yield* AppendUpdated(transaction, decoded, command.message_id);
						return {
							duplicate: false as const,
							event,
							run_id: updated.target_run_id ?? updated.source_run_id,
						};
					}),
				);
				if (result.duplicate) {
					const events = yield* journal.ReadCorrelatedEvents(command.message_id);
					return {
						events,
						journal_sequence: events.at(-1)?.journal_sequence ?? 0,
						run_id: prior.target_run_id ?? prior.source_run_id,
						status: "duplicate" as const,
					};
				}
				yield* notifier.Publish(result.event.journal_sequence);
				return {
					events: [result.event],
					journal_sequence: result.event.journal_sequence,
					run_id: result.run_id,
					status: "accepted" as const,
				};
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof OrchestrationCommandConflict ||
					cause instanceof OrchestrationFailure
						? cause
						: cause instanceof CommandIdConflict
							? new OrchestrationCommandConflict({ message_id: cause.message_id })
							: new OrchestrationFailure({ cause }),
				),
			);

		const ScanDue = Effect.gen(function* () {
			const now = yield* metadata.Now;
			const launching = yield* database.client
				.select({
					status: OrchestrationRuns.status,
					target_run_id: UsageInterruptions.target_run_id,
				})
				.from(UsageInterruptions)
				.leftJoin(
					OrchestrationRuns,
					eq(OrchestrationRuns.run_id, UsageInterruptions.target_run_id),
				)
				.where(eq(UsageInterruptions.state, "launching"))
				.limit(32);
			for (const launch of launching) {
				if (launch.target_run_id === null) continue;
				if (["running", "waiting", "completed"].includes(launch.status ?? "")) {
					yield* TransitionTarget(launch.target_run_id, "continued");
				} else if (["failed", "cancelled", "closed"].includes(launch.status ?? "")) {
					yield* TransitionTarget(launch.target_run_id, "failed");
				}
			}
			const rows = yield* database.client
				.select()
				.from(UsageInterruptions)
				.where(
					and(
						eq(UsageInterruptions.state, "scheduled"),
						or(
							isNull(UsageInterruptions.resume_not_before),
							lte(UsageInterruptions.resume_not_before, now),
						),
					),
				)
				.limit(32);
			let launched_any = false;
			for (const row of rows) {
				const snapshot = yield* DecodeUsageInterruptionRow(row).pipe(
					Effect.mapError((cause) => new OrchestrationFailure({ cause })),
				);
				const evidence = yield* RefreshEvidence(snapshot);
				const alternatives = evidence.alternatives;
				const command_id =
					row.continuation_command_id ?? `usage-continuation:${row.interruption_id}`;
				if (!evidence.source_available) {
					const next_reset = evidence.matching_reset;
					const future_reset =
						next_reset !== undefined && Date.parse(next_reset) > Date.parse(now)
							? new Date(next_reset).toISOString()
							: undefined;
					const next_check = future_reset ?? NextUsagePoll(now);
					const event = yield* database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [updated] = yield* transaction
								.update(UsageInterruptions)
								.set({
									affected_model_id:
										evidence.source_affected_model_id ?? row.affected_model_id,
									alternatives_json: JSON.stringify(alternatives),
									limit_label: evidence.source_limit_label ?? row.limit_label,
									limit_scope: evidence.source_limit_scope ?? row.limit_scope,
									resets_at: future_reset ?? null,
									resume_not_before: next_check,
									revision: sql`${UsageInterruptions.revision} + 1`,
									state: "scheduled",
									updated_at: now,
								})
								.where(
									and(
										eq(UsageInterruptions.interruption_id, row.interruption_id),
										eq(UsageInterruptions.revision, row.revision),
										eq(UsageInterruptions.state, "scheduled"),
									),
								)
								.returning();
							if (updated === undefined) return undefined;
							const decoded = yield* DecodeUsageInterruptionRow(updated);
							return yield* AppendUpdated(transaction, decoded, command_id);
						}),
					);
					if (event !== undefined) yield* notifier.Publish(event.journal_sequence);
					continue;
				}
				const outcome = yield* database.client.transaction((transaction) =>
					ClaimLaunch(
						transaction,
						{
							...row,
							affected_model_id:
								evidence.source_affected_model_id ?? row.affected_model_id,
							alternatives_json: JSON.stringify(alternatives),
							limit_label: evidence.source_limit_label ?? row.limit_label,
							limit_scope: evidence.source_limit_scope ?? row.limit_scope,
						},
						{
							command_id,
							target_engine_id: row.source_engine_id,
							...(row.source_model_id === null
								? {}
								: { target_model_id: row.source_model_id }),
						},
					),
				);
				if (
					outcome._tag === "raced" ||
					outcome._tag === "discarded" ||
					outcome._tag === "source_pending"
				)
					continue;
				if (outcome._tag === "launched") launched_any = true;
				yield* notifier.Publish(outcome.event.journal_sequence);
			}
			return launched_any;
		}).pipe(
			Effect.mapError((cause) =>
				cause instanceof OrchestrationFailure ? cause : new OrchestrationFailure({ cause }),
			),
		);

		const CancelSuperseded = (thread_id: string, active_run_id: string) =>
			Effect.gen(function* () {
				const rows = yield* database.client
					.select()
					.from(UsageInterruptions)
					.where(
						and(
							eq(UsageInterruptions.thread_id, thread_id),
							ne(UsageInterruptions.source_run_id, active_run_id),
							or(
								eq(UsageInterruptions.state, "scheduled"),
								eq(UsageInterruptions.state, "awaiting_decision"),
							),
						),
					)
					.limit(32);
				for (const row of rows) {
					const now = yield* metadata.Now;
					const event = yield* database.client.transaction((transaction) =>
						Effect.gen(function* () {
							const [updated] = yield* transaction
								.update(UsageInterruptions)
								.set({
									cancelled_at: now,
									revision: sql`${UsageInterruptions.revision} + 1`,
									state: "cancelled",
									updated_at: now,
								})
								.where(
									and(
										eq(UsageInterruptions.interruption_id, row.interruption_id),
										eq(UsageInterruptions.revision, row.revision),
									),
								)
								.returning();
							if (updated === undefined) return undefined;
							const interruption = yield* DecodeUsageInterruptionRow(updated);
							return yield* AppendUpdated(
								transaction,
								interruption,
								`usage-superseded:${active_run_id}`,
							);
						}),
					);
					if (event !== undefined) yield* notifier.Publish(event.journal_sequence);
				}
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof OrchestrationFailure
						? cause
						: new OrchestrationFailure({ cause }),
				),
			);

		const RefreshPendingEvidence = Effect.gen(function* () {
			const rows = yield* database.client
				.select()
				.from(UsageInterruptions)
				.where(
					and(
						isNull(UsageInterruptions.evidence_refreshed_at),
						or(
							eq(UsageInterruptions.state, "scheduled"),
							eq(UsageInterruptions.state, "awaiting_decision"),
						),
					),
				)
				.limit(16);
			for (const row of rows) {
				const snapshot = yield* DecodeUsageInterruptionRow(row);
				const evidence = yield* RefreshEvidence(snapshot);
				const now = yield* metadata.Now;
				const verified_reset =
					evidence.matching_reset !== undefined &&
					Date.parse(evidence.matching_reset) > Date.parse(now)
						? new Date(evidence.matching_reset).toISOString()
						: undefined;
				const event = yield* database.client.transaction((transaction) =>
					Effect.gen(function* () {
						const [updated] = yield* transaction
							.update(UsageInterruptions)
							.set({
								affected_model_id:
									evidence.source_affected_model_id ?? row.affected_model_id,
								alternatives_json: JSON.stringify(evidence.alternatives),
								evidence_refreshed_at: now,
								limit_label: evidence.source_limit_label ?? row.limit_label,
								limit_scope: evidence.source_limit_scope ?? row.limit_scope,
								...(verified_reset === undefined
									? {}
									: {
											resets_at: verified_reset,
											...(row.auto_continue && row.state === "scheduled"
												? { resume_not_before: verified_reset }
												: {}),
										}),
								revision: sql`${UsageInterruptions.revision} + 1`,
								updated_at: now,
							})
							.where(
								and(
									eq(UsageInterruptions.interruption_id, row.interruption_id),
									eq(UsageInterruptions.revision, row.revision),
									isNull(UsageInterruptions.evidence_refreshed_at),
								),
							)
							.returning();
						if (updated === undefined) return undefined;
						const interruption = yield* DecodeUsageInterruptionRow(updated);
						return yield* AppendUpdated(
							transaction,
							interruption,
							`usage-evidence:${row.interruption_id}`,
						);
					}),
				);
				if (event !== undefined) yield* notifier.Publish(event.journal_sequence);
			}
		}).pipe(
			Effect.mapError((cause) =>
				cause instanceof OrchestrationFailure ? cause : new OrchestrationFailure({ cause }),
			),
		);

		return {
			CancelSuperseded,
			MarkTargetContinued: (target_run_id) => TransitionTarget(target_run_id, "continued"),
			MarkTargetFailed: (target_run_id) => TransitionTarget(target_run_id, "failed"),
			Resolve,
			RefreshPendingEvidence,
			ScanDue,
		};
	}),
);
