import { and, desc, eq } from "drizzle-orm";
import { Context, Effect } from "effect";

import type { EventEnvelope, ThreadMessageRoutedEvent } from "@artisan/protocol";

import type { IntakeAssessment } from "../../orchestration/intake-policy";
import { RuntimeMetadata } from "../../runtime/metadata";
import type { DatabaseClient } from "../database";
import { AppendJournalEventInTransaction } from "../journal-store";
import {
	JournalEvents,
	JournalCommands,
	MessageImageAttachments,
	OrchestrationCoordinators,
	OrchestrationInteractions,
	OrchestrationIntake,
	OrchestrationMessages,
	OrchestrationOutbox,
	OrchestrationRuns,
} from "../tables";
import { OrchestrationFailure, OrchestrationNotFound, type OutboxKind } from "./contracts";
import { ImageAttachmentsFor, SanitisePayload } from "./message-attachments";
import type {
	AuthoritativePayload,
	InboundOrAuthoritativeCommandEnvelope,
} from "./message-command";
import {
	DecodePersistedJson,
	PersistedMentionedProjects,
	PersistedRawOrigin,
} from "./storage-codec";

const is_active_status = (status: string): status is "running" | "waiting" =>
	status === "running" || status === "waiting";

const is_projectable_status = (status: string): status is "queued" | "running" | "waiting" =>
	status === "queued" || is_active_status(status);

export class CommandTransaction extends Context.Service<
	CommandTransaction,
	{ readonly client: DatabaseClient }
>()("Artisan/Orchestration/CommandTransaction") {}

interface DispatchCommandInput {
	readonly can_steer: boolean;
	readonly command: InboundOrAuthoritativeCommandEnvelope;
	readonly intake: IntakeAssessment | undefined;
	readonly payload: AuthoritativePayload;
	readonly payload_json: string;
	readonly raw_origin_json: string | null;
	readonly routing_reason: ThreadMessageRoutedEvent["reason"] | undefined;
}

export const MakeCommandDispatcher = Effect.gen(function* () {
	const metadata = yield* RuntimeMetadata;
	const DispatchCommand = ({
		can_steer,
		command,
		intake,
		payload,
		payload_json,
		raw_origin_json,
		routing_reason,
	}: DispatchCommandInput) =>
		Effect.gen(function* () {
			const transaction = (yield* CommandTransaction).client;
			const GetJournalSequence = transaction
				.select({ journal_sequence: JournalEvents.sequence })
				.from(JournalEvents)
				.orderBy(desc(JournalEvents.sequence))
				.limit(1)
				.pipe(Effect.map(([event]) => event?.journal_sequence ?? 0));
			const AppendEvent = (input: Parameters<typeof AppendJournalEventInTransaction>[2]) =>
				AppendJournalEventInTransaction(transaction, metadata, input);
			const [coordinator] = yield* transaction
				.select()
				.from(OrchestrationCoordinators)
				.where(eq(OrchestrationCoordinators.thread_id, command.thread_id))
				.limit(1);
			const accepted_at = yield* metadata.Now;
			if (payload.type === "thread.auto_steer.update") {
				if (!coordinator) {
					return yield* new OrchestrationNotFound({
						id: command.thread_id,
						resource: "run",
					});
				}
				yield* transaction.insert(JournalCommands).values({
					accepted_at,
					agent_id: command.agent_id ?? null,
					causation_id: command.causation_id ?? null,
					message_id: command.message_id,
					origin: command.origin,
					payload_json,
					payload_type: payload.type,
					raw_origin_json,
					assigned_run_id: coordinator.active_run_id,
					run_id: command.run_id ?? null,
					schema_version: command.schema_version,
					sent_at: command.sent_at,
					status: "accepted",
					thread_id: command.thread_id,
				});
				yield* transaction
					.update(OrchestrationCoordinators)
					.set({
						auto_steer_follow_ups: payload.enabled,
						updated_at: accepted_at,
					})
					.where(eq(OrchestrationCoordinators.thread_id, command.thread_id));
				const event = yield* AppendEvent({
					agent_id: coordinator.agent_id,
					causation_id: command.message_id,
					correlation_id: command.message_id,
					payload: {
						type: "thread.auto_steer.updated",
						enabled: payload.enabled,
					},
					thread_id: command.thread_id,
				});
				return {
					events: [event],
					journal_sequence: event.journal_sequence,
					run_id: coordinator.active_run_id ?? "none",
					status: "accepted" as const,
				};
			}
			if (payload.type === "thread.session_policy.update") {
				const agent_id = coordinator?.agent_id ?? (yield* metadata.MakeId("agent"));
				yield* transaction.insert(JournalCommands).values({
					accepted_at,
					agent_id: command.agent_id ?? null,
					causation_id: command.causation_id ?? null,
					message_id: command.message_id,
					origin: command.origin,
					payload_json,
					payload_type: payload.type,
					raw_origin_json,
					assigned_run_id: coordinator?.active_run_id ?? null,
					run_id: command.run_id ?? null,
					schema_version: command.schema_version,
					sent_at: command.sent_at,
					status: "accepted",
					thread_id: command.thread_id,
				});
				const policy_columns = {
					engine_id: payload.policy.engine_id,
					policy_model: payload.policy.model ?? null,
					policy_context_window: payload.policy.context_window ?? null,
					policy_reasoning_effort: payload.policy.reasoning_effort,
					policy_permission: payload.policy.permission,
					policy_permission_mode: payload.policy.permission_mode,
					policy_sandbox_mode: payload.policy.sandbox_mode,
					policy_service_tier: payload.policy.service_tier,
					policy_web_search_enabled: payload.policy.web_search_enabled,
					policy_strict_clarification: payload.policy.strict_clarification,
					updated_at: accepted_at,
				};
				if (coordinator) {
					yield* transaction
						.update(OrchestrationCoordinators)
						.set(policy_columns)
						.where(eq(OrchestrationCoordinators.thread_id, command.thread_id));
				} else {
					yield* transaction.insert(OrchestrationCoordinators).values({
						active_run_id: null,
						agent_id,
						auto_steer_follow_ups: true,
						created_at: accepted_at,
						display_name: "Primary coordinator",
						native_resume_json: null,
						native_thread_id: null,
						role: "primary",
						thread_id: command.thread_id,
						...policy_columns,
					});
				}
				const event = yield* AppendEvent({
					agent_id,
					causation_id: command.message_id,
					correlation_id: command.message_id,
					payload: {
						type: "thread.session_policy.updated",
						policy: payload.policy,
					},
					thread_id: command.thread_id,
				});
				return {
					events: [event],
					journal_sequence: event.journal_sequence,
					run_id: coordinator?.active_run_id ?? "none",
					status: "accepted" as const,
				};
			}
			if (payload.type === "intake.respond_question") {
				const [pending] = yield* transaction
					.select()
					.from(OrchestrationIntake)
					.where(
						and(
							eq(OrchestrationIntake.thread_id, command.thread_id),
							eq(OrchestrationIntake.question_id, payload.question_id),
							eq(OrchestrationIntake.state, "pending"),
						),
					)
					.limit(1);
				if (
					!pending ||
					Object.keys(payload.answers).length !== 1 ||
					!Object.hasOwn(payload.answers, payload.question_id)
				) {
					return yield* new OrchestrationNotFound({
						id: payload.question_id,
						resource: "run",
					});
				}
				const [active_coordinator_run] = coordinator?.active_run_id
					? yield* transaction
							.select({ status: OrchestrationRuns.status })
							.from(OrchestrationRuns)
							.where(eq(OrchestrationRuns.run_id, coordinator.active_run_id))
							.limit(1)
					: [];
				if (
					active_coordinator_run &&
					is_projectable_status(active_coordinator_run.status)
				) {
					return yield* new OrchestrationFailure({
						cause: new Error("Cannot resolve intake while a coordinator run is active"),
					});
				}
				const run_id = yield* metadata.MakeId("run");
				const intake_raw_origin = pending.raw_origin_json
					? yield* DecodePersistedJson(PersistedRawOrigin, pending.raw_origin_json)
					: undefined;
				const resolved_agent_id =
					coordinator?.agent_id ?? command.agent_id ?? (yield* metadata.MakeId("agent"));
				yield* transaction.insert(JournalCommands).values({
					accepted_at,
					agent_id: command.agent_id ?? null,
					causation_id: command.causation_id ?? null,
					message_id: command.message_id,
					origin: command.origin,
					payload_json,
					payload_type: payload.type,
					raw_origin_json,
					assigned_run_id: run_id,
					run_id: command.run_id ?? null,
					schema_version: command.schema_version,
					sent_at: command.sent_at,
					status: "accepted",
					thread_id: command.thread_id,
				});
				if (!coordinator)
					yield* transaction.insert(OrchestrationCoordinators).values({
						active_run_id: run_id,
						agent_id: resolved_agent_id,
						auto_steer_follow_ups: true,
						created_at: accepted_at,
						display_name: "Primary coordinator",
						engine_id: pending.engine_id,
						native_resume_json: null,
						native_thread_id: null,
						role: "primary",
						thread_id: command.thread_id,
						updated_at: accepted_at,
					});
				else
					yield* transaction
						.update(OrchestrationCoordinators)
						.set({
							active_run_id: run_id,
							engine_id: pending.engine_id,
							updated_at: accepted_at,
						})
						.where(eq(OrchestrationCoordinators.thread_id, command.thread_id));
				yield* transaction.insert(OrchestrationRuns).values({
					agent_id: resolved_agent_id,
					created_at: accepted_at,
					engine_id: pending.engine_id,
					native_resume_json: null,
					native_thread_id: null,
					run_id,
					status: "queued",
					thread_id: command.thread_id,
					updated_at: accepted_at,
					working_directory: pending.working_directory,
				});
				yield* transaction.insert(OrchestrationMessages).values({
					agent_id: resolved_agent_id,
					command_id: command.message_id,
					created_at: accepted_at,
					delivery: "queued",
					message_id: pending.message_id,
					run_id,
					text: pending.text,
					thread_id: command.thread_id,
				});
				yield* transaction
					.update(OrchestrationIntake)
					.set({ state: "resolved", updated_at: accepted_at })
					.where(eq(OrchestrationIntake.message_id, pending.message_id));
				const start_payload = {
					type: "thread.send_message",
					engine_id: pending.engine_id,
					text: pending.text,
					working_directory: pending.working_directory,
					...(pending.mentioned_projects_json
						? {
								mentioned_projects: yield* DecodePersistedJson(
									PersistedMentionedProjects,
									pending.mentioned_projects_json,
								),
							}
						: {}),
				};
				yield* transaction.insert(OrchestrationOutbox).values({
					agent_id: resolved_agent_id,
					command_id: command.message_id,
					created_at: accepted_at,
					kind: "start",
					payload_json: JSON.stringify(start_payload),
					run_id,
					status: "pending",
					thread_id: command.thread_id,
					updated_at: accepted_at,
				});
				const events = [
					yield* AppendEvent({
						agent_id: resolved_agent_id,
						causation_id: command.message_id,
						correlation_id: command.message_id,
						payload: {
							type: "interaction.question",
							question_id: payload.question_id,
							answers: payload.answers,
							state: "resolved",
							text: pending.question ?? "Clarified",
							source: "intake",
						},
						...(intake_raw_origin === undefined
							? {}
							: { raw_origin: intake_raw_origin }),
						run_id,
						thread_id: command.thread_id,
					}),
					yield* AppendEvent({
						agent_id: resolved_agent_id,
						causation_id: command.message_id,
						correlation_id: command.message_id,
						payload: {
							type: "thread.message_queued",
							message_id: pending.message_id,
							reason: "no_active_run",
							text: pending.text,
							working_directory: pending.working_directory,
						},
						...(intake_raw_origin === undefined
							? {}
							: { raw_origin: intake_raw_origin }),
						run_id,
						thread_id: command.thread_id,
					}),
					yield* AppendEvent({
						agent_id: resolved_agent_id,
						causation_id: command.message_id,
						correlation_id: command.message_id,
						payload: {
							message_id: pending.message_id,
							outcome: "queued",
							reason: "no_active_run",
							run_id,
							type: "thread.message_routed",
						},
						...(intake_raw_origin === undefined
							? {}
							: { raw_origin: intake_raw_origin }),
						run_id,
						thread_id: command.thread_id,
					}),
					yield* AppendEvent({
						agent_id: resolved_agent_id,
						causation_id: command.message_id,
						correlation_id: command.message_id,
						payload: {
							type: "run.lifecycle",
							state: "queued",
							working_directory: pending.working_directory,
						},
						...(intake_raw_origin === undefined
							? {}
							: { raw_origin: intake_raw_origin }),
						run_id,
						thread_id: command.thread_id,
					}),
				];
				return {
					events,
					journal_sequence:
						events.at(-1)?.journal_sequence ?? (yield* GetJournalSequence),
					run_id,
					status: "accepted" as const,
				};
			}
			const agent_id =
				coordinator?.agent_id ?? command.agent_id ?? (yield* metadata.MakeId("agent"));
			const active_run = coordinator?.active_run_id
				? yield* transaction
						.select()
						.from(OrchestrationRuns)
						.where(eq(OrchestrationRuns.run_id, coordinator.active_run_id))
						.limit(1)
				: [];
			const current_run = active_run[0];

			if (command.run_id && current_run?.run_id !== command.run_id) {
				return yield* new OrchestrationNotFound({
					id: command.run_id,
					resource: "run",
				});
			}

			const send_message = payload.type === "thread.send_message";
			if (send_message && intake?.resolution === "question") {
				const question_id = yield* metadata.MakeId("message");
				const intake_id = yield* metadata.MakeId("run");
				yield* transaction.insert(JournalCommands).values({
					accepted_at,
					agent_id: command.agent_id ?? null,
					causation_id: command.causation_id ?? null,
					message_id: command.message_id,
					origin: command.origin,
					payload_json,
					payload_type: payload.type,
					raw_origin_json,
					assigned_run_id: intake_id,
					run_id: command.run_id ?? null,
					schema_version: command.schema_version,
					sent_at: command.sent_at,
					status: "accepted",
					thread_id: command.thread_id,
				});
				yield* transaction.insert(OrchestrationIntake).values({
					assumptions_json: JSON.stringify(intake.assumptions),
					created_at: accepted_at,
					engine_id: payload.engine_id,
					message_id: command.message_id,
					question: intake.question ?? "Please clarify the request.",
					question_id,
					risk: intake.risk,
					state: "pending",
					text: payload.text,
					mentioned_projects_json:
						payload.mentioned_projects === undefined
							? null
							: JSON.stringify(payload.mentioned_projects),
					thread_id: command.thread_id,
					raw_origin_json,
					updated_at: accepted_at,
					working_directory: payload.working_directory,
				});
				const events = [
					yield* AppendEvent({
						agent_id: command.agent_id ?? "intake",
						causation_id: command.message_id,
						correlation_id: command.message_id,
						payload: {
							type: "intake.assessed",
							message_id: command.message_id,
							risk: intake.risk,
							resolution: "question",
						},
						...(command.raw_origin === undefined
							? {}
							: { raw_origin: command.raw_origin }),
						thread_id: command.thread_id,
					}),
					yield* AppendEvent({
						agent_id: command.agent_id ?? "intake",
						causation_id: command.message_id,
						correlation_id: command.message_id,
						payload: {
							type: "interaction.question",
							question_id,
							state: "requested",
							text: intake.question ?? "Please clarify the request.",
							source: "intake",
						},
						...(command.raw_origin === undefined
							? {}
							: { raw_origin: command.raw_origin }),
						thread_id: command.thread_id,
					}),
				];
				return {
					events,
					journal_sequence:
						events.at(-1)?.journal_sequence ?? (yield* GetJournalSequence),
					run_id: intake_id,
					status: "accepted" as const,
				};
			}
			const requested_engine_id = "engine_id" in payload ? payload.engine_id : undefined;
			const steer =
				send_message &&
				current_run &&
				is_active_status(current_run.status) &&
				requested_engine_id === current_run.engine_id &&
				can_steer;
			const engine_id = steer
				? current_run.engine_id
				: send_message
					? requested_engine_id
					: (current_run?.engine_id ?? coordinator?.engine_id);

			if (!engine_id) {
				return yield* new OrchestrationNotFound({
					id: command.thread_id,
					resource: "run",
				});
			}

			const run_id =
				steer || !send_message
					? (command.run_id ?? current_run?.run_id)
					: yield* metadata.MakeId("run");

			if (!run_id) {
				return yield* new OrchestrationNotFound({
					id: command.thread_id,
					resource: "run",
				});
			}

			yield* transaction.insert(JournalCommands).values({
				accepted_at,
				agent_id: command.agent_id ?? null,
				causation_id: command.causation_id ?? null,
				message_id: command.message_id,
				origin: command.origin,
				payload_json,
				payload_type: payload.type,
				raw_origin_json,
				assigned_run_id: run_id,
				run_id: command.run_id ?? null,
				schema_version: command.schema_version,
				sent_at: command.sent_at,
				status: "accepted",
				thread_id: command.thread_id,
			});
			if (
				payload.type === "thread.send_message" &&
				payload.attachments !== undefined &&
				payload.attachments.length > 0
			) {
				const durable_attachments = yield* Effect.forEach(
					payload.attachments,
					(attachment) =>
						attachment.bytes === undefined
							? new OrchestrationFailure({
									cause: new Error(
										`Attachment ${attachment.id} has no durable bytes`,
									),
								})
							: Effect.succeed({ ...attachment, bytes: attachment.bytes }),
				);
				yield* transaction.insert(MessageImageAttachments).values(
					durable_attachments.map((attachment, position) => ({
						attachment_id: attachment.id,
						content: Buffer.from(attachment.bytes),
						media_type: attachment.media_type,
						message_id: command.message_id,
						name: attachment.name,
						position,
						size_bytes: attachment.bytes.byteLength,
					})),
				);
			}

			if (!coordinator) {
				yield* transaction.insert(OrchestrationCoordinators).values({
					active_run_id: send_message && !steer ? run_id : null,
					agent_id,
					created_at: accepted_at,
					display_name: "Primary coordinator",
					engine_id,
					native_resume_json: null,
					native_thread_id: null,
					role: "primary",
					thread_id: command.thread_id,
					updated_at: accepted_at,
				});
			}

			const events: EventEnvelope[] = [];

			if (send_message && !steer) {
				yield* transaction.insert(OrchestrationRuns).values({
					agent_id,
					created_at: accepted_at,
					engine_id,
					native_resume_json: null,
					native_thread_id: null,
					run_id,
					status: "queued",
					thread_id: command.thread_id,
					updated_at: accepted_at,
					working_directory: payload.working_directory,
				});
				yield* transaction
					.update(OrchestrationCoordinators)
					.set({ active_run_id: run_id, engine_id, updated_at: accepted_at })
					.where(eq(OrchestrationCoordinators.thread_id, command.thread_id));
				yield* transaction.insert(OrchestrationMessages).values({
					agent_id,
					command_id: command.message_id,
					created_at: accepted_at,
					delivery: "queued",
					message_id: command.message_id,
					run_id,
					text: payload.text,
					thread_id: command.thread_id,
				});
				events.push(
					yield* AppendEvent({
						agent_id,
						causation_id: command.message_id,
						correlation_id: command.message_id,
						payload: {
							message_id: command.message_id,
							...(payload.attachments === undefined
								? {}
								: { attachments: ImageAttachmentsFor(payload) }),
							...(payload.content === undefined ? {} : { content: payload.content }),
							...(payload.mentioned_projects === undefined
								? {}
								: { mentioned_projects: payload.mentioned_projects }),
							reason:
								routing_reason ??
								(current_run && is_active_status(current_run.status)
									? "unsupported"
									: "no_active_run"),
							text: payload.text,
							type: "thread.message_queued",
							working_directory: payload.working_directory,
						},
						...(command.raw_origin ? { raw_origin: command.raw_origin } : {}),
						run_id,
						thread_id: command.thread_id,
					}),
				);
				events.push(
					yield* AppendEvent({
						agent_id,
						causation_id: command.message_id,
						correlation_id: command.message_id,
						payload: {
							message_id: command.message_id,
							outcome: "queued",
							reason:
								routing_reason ??
								(current_run && is_active_status(current_run.status)
									? "unsupported"
									: "no_active_run"),
							run_id,
							type: "thread.message_routed",
						},
						run_id,
						thread_id: command.thread_id,
					}),
				);
				events.push(
					yield* AppendEvent({
						agent_id,
						causation_id: command.message_id,
						correlation_id: command.message_id,
						payload: {
							state: "queued",
							type: "run.lifecycle",
							working_directory: payload.working_directory,
						},
						...(command.raw_origin ? { raw_origin: command.raw_origin } : {}),
						run_id,
						thread_id: command.thread_id,
					}),
				);
			} else if (send_message) {
				yield* transaction.insert(OrchestrationMessages).values({
					agent_id,
					command_id: command.message_id,
					created_at: accepted_at,
					delivery: "steering",
					message_id: command.message_id,
					run_id,
					text: payload.text,
					thread_id: command.thread_id,
				});
				events.push(
					yield* AppendEvent({
						agent_id,
						causation_id: command.message_id,
						correlation_id: command.message_id,
						payload: {
							message_id: command.message_id,
							...(payload.attachments === undefined
								? {}
								: { attachments: ImageAttachmentsFor(payload) }),
							...(payload.content === undefined ? {} : { content: payload.content }),
							...(payload.mentioned_projects === undefined
								? {}
								: { mentioned_projects: payload.mentioned_projects }),
							text: payload.text,
							type: "thread.message_steering",
							working_directory: payload.working_directory,
						},
						...(command.raw_origin ? { raw_origin: command.raw_origin } : {}),
						run_id,
						thread_id: command.thread_id,
					}),
				);
			}

			if (payload.type === "run.respond_approval") {
				const [interaction] = yield* transaction
					.select()
					.from(OrchestrationInteractions)
					.where(
						and(
							eq(OrchestrationInteractions.interaction_id, payload.approval_id),
							eq(OrchestrationInteractions.kind, "approval"),
							eq(OrchestrationInteractions.run_id, run_id),
							eq(OrchestrationInteractions.state, "requested"),
						),
					)
					.limit(1);

				if (!interaction) {
					return yield* new OrchestrationNotFound({
						id: payload.approval_id,
						resource: "run",
					});
				}
			}

			if (payload.type === "run.respond_question") {
				for (const question_id of Object.keys(payload.answers)) {
					const [interaction] = yield* transaction
						.select()
						.from(OrchestrationInteractions)
						.where(
							and(
								eq(OrchestrationInteractions.interaction_id, question_id),
								eq(OrchestrationInteractions.kind, "question"),
								eq(OrchestrationInteractions.run_id, run_id),
								eq(OrchestrationInteractions.state, "requested"),
							),
						)
						.limit(1);

					if (!interaction) {
						return yield* new OrchestrationNotFound({
							id: question_id,
							resource: "run",
						});
					}
				}
			}

			if (send_message && intake) {
				yield* transaction.insert(OrchestrationIntake).values({
					assumptions_json: JSON.stringify(intake.assumptions),
					created_at: accepted_at,
					engine_id: payload.engine_id,
					message_id: command.message_id,
					mentioned_projects_json:
						payload.mentioned_projects === undefined
							? null
							: JSON.stringify(payload.mentioned_projects),
					question: null,
					question_id: null,
					raw_origin_json,
					risk: intake.risk,
					state: "resolved",
					text: payload.text,
					thread_id: command.thread_id,
					updated_at: accepted_at,
					working_directory: payload.working_directory,
				});
				events.push(
					yield* AppendEvent({
						agent_id,
						causation_id: command.message_id,
						correlation_id: command.message_id,
						payload: {
							type: "intake.assessed",
							message_id: command.message_id,
							risk: intake.risk,
							resolution: intake.resolution,
						},
						...(command.raw_origin === undefined
							? {}
							: { raw_origin: command.raw_origin }),
						thread_id: command.thread_id,
					}),
				);
				for (const assumption of intake.assumptions) {
					events.push(
						yield* AppendEvent({
							agent_id,
							causation_id: command.message_id,
							correlation_id: command.message_id,
							payload: {
								type: "intake.assumption_recorded",
								message_id: command.message_id,
								assumption,
							},
							...(command.raw_origin === undefined
								? {}
								: { raw_origin: command.raw_origin }),
							thread_id: command.thread_id,
						}),
					);
				}
			}

			const kind: OutboxKind =
				payload.type === "thread.send_message"
					? steer
						? "steer"
						: "start"
					: (payload.type.replace("run.", "") as OutboxKind);
			yield* transaction.insert(OrchestrationOutbox).values({
				agent_id,
				command_id: command.message_id,
				created_at: accepted_at,
				kind,
				payload_json:
					payload.type === "thread.send_message"
						? JSON.stringify(SanitisePayload(payload))
						: payload_json,
				run_id,
				status: "pending",
				thread_id: command.thread_id,
				updated_at: accepted_at,
			});

			return {
				events,
				journal_sequence: events.at(-1)?.journal_sequence ?? (yield* GetJournalSequence),
				run_id,
				status: "accepted" as const,
			};
		});

	return DispatchCommand;
});
