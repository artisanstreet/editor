import { eq } from "drizzle-orm";
import { Effect } from "effect";

import type { EngineErrorRef } from "@artisan/engines";
import type { EventEnvelope, UsageInterruption } from "@artisan/protocol";

import type { DatabaseClient } from "../database";
import { AppendJournalEventInTransaction } from "../journal-store";
import { OrchestrationRuns, SessionDefaults, UsageInterruptions } from "../tables";
import type { RuntimeMetadata } from "../../runtime/metadata";
import { SanitiseUsageEvidenceText } from "./model";

const ValidFutureReset = (value: string | undefined, now: string) => {
	if (value === undefined) return undefined;
	const milliseconds = Date.parse(value);
	return Number.isFinite(milliseconds) && milliseconds > Date.parse(now)
		? new Date(milliseconds).toISOString()
		: undefined;
};

const NextUsagePoll = (now: string) => new Date(Date.parse(now) + 5 * 60 * 1_000).toISOString();

/** Atomically creates and projects the first exact usage-limit failure for one run. */
export const RecordUsageInterruptionInTransaction = (
	transaction: DatabaseClient,
	metadata: typeof RuntimeMetadata.Service,
	run: typeof OrchestrationRuns.$inferSelect,
	error_ref: EngineErrorRef,
): Effect.Effect<EventEnvelope | undefined, unknown> =>
	Effect.gen(function* () {
		if (error_ref.artisan_code !== "AE-PROVIDER-201") return undefined;
		const now = yield* metadata.Now;
		const [defaults] = yield* transaction
			.select({ auto_continue: SessionDefaults.auto_continue_usage_limits })
			.from(SessionDefaults)
			.where(eq(SessionDefaults.defaults_id, 1))
			.limit(1);
		const auto_continue = defaults?.auto_continue ?? true;
		const resets_at = ValidFutureReset(error_ref.resets_at, now);
		const interruption_id = `usage-interruption:${run.run_id}`;
		const inserted = yield* transaction
			.insert(UsageInterruptions)
			.values({
				affected_model_id: SanitiseUsageEvidenceText(error_ref.affected_model_id) ?? null,
				alternatives_json: "[]",
				auto_continue,
				cancelled_at: null,
				continuation_command_id: null,
				continued_at: null,
				created_at: now,
				evidence_refreshed_at: null,
				failed_at: null,
				interruption_id,
				limit_id: SanitiseUsageEvidenceText(error_ref.limit_id) ?? null,
				limit_label: SanitiseUsageEvidenceText(error_ref.limit_label) ?? null,
				limit_scope:
					error_ref.limit_scope === "shared" || error_ref.limit_scope === "model"
						? error_ref.limit_scope
						: "unknown",
				provider_code: SanitiseUsageEvidenceText(error_ref.provider_code) ?? null,
				resets_at: resets_at ?? null,
				resume_not_before: auto_continue ? (resets_at ?? NextUsagePoll(now)) : null,
				revision: 0,
				source_agent_id: run.agent_id,
				source_engine_id: run.engine_id,
				source_model_id: run.model_id,
				source_run_id: run.run_id,
				state: auto_continue ? "scheduled" : "awaiting_decision",
				target_engine_id: null,
				target_model_id: null,
				target_run_id: null,
				thread_id: run.thread_id,
				updated_at: now,
			})
			.onConflictDoNothing({ target: UsageInterruptions.source_run_id })
			.returning();
		const row = inserted.at(0);
		if (row === undefined) return undefined;
		const interruption = {
			...(row.affected_model_id === null ? {} : { affected_model_id: row.affected_model_id }),
			alternatives: [],
			auto_continue: row.auto_continue,
			created_at: row.created_at,
			interruption_id: row.interruption_id,
			...(row.limit_id === null ? {} : { limit_id: row.limit_id }),
			...(row.limit_label === null ? {} : { limit_label: row.limit_label }),
			limit_scope: row.limit_scope as UsageInterruption["limit_scope"],
			...(row.provider_code === null ? {} : { provider_code: row.provider_code }),
			...(row.resets_at === null ? {} : { resets_at: row.resets_at }),
			...(row.resume_not_before === null ? {} : { resume_not_before: row.resume_not_before }),
			revision: row.revision,
			source_agent_id: row.source_agent_id,
			source_engine_id: row.source_engine_id,
			...(row.source_model_id === null ? {} : { source_model_id: row.source_model_id }),
			source_run_id: row.source_run_id,
			state: row.state as UsageInterruption["state"],
			thread_id: row.thread_id,
			updated_at: row.updated_at,
		} satisfies UsageInterruption;
		return yield* AppendJournalEventInTransaction(transaction, metadata, {
			agent_id: run.agent_id,
			causation_id: interruption_id,
			correlation_id: run.run_id,
			idempotency_key: interruption_id,
			payload: { interruption, type: "usage.interruption.updated" },
			run_id: run.run_id,
			thread_id: run.thread_id,
		});
	});
