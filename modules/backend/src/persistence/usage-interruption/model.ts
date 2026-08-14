import { Effect, Schema } from "effect";

import {
	UsageInterruption,
	type UsageInterruption as UsageInterruptionSnapshot,
} from "@artisan/protocol";

/** Decode a database row without allowing malformed JSON to escape persistence. */
export const DecodeUsageInterruptionRow = (row: {
	readonly affected_model_id: string | null;
	readonly alternatives_json: string;
	readonly auto_continue: boolean;
	readonly cancelled_at: string | null;
	readonly continuation_command_id: string | null;
	readonly continued_at: string | null;
	readonly created_at: string;
	readonly failed_at: string | null;
	readonly interruption_id: string;
	readonly limit_id: string | null;
	readonly limit_label: string | null;
	readonly limit_scope: string;
	readonly provider_code: string | null;
	readonly resets_at: string | null;
	readonly resume_not_before: string | null;
	readonly revision: number;
	readonly source_agent_id: string;
	readonly source_engine_id: string;
	readonly source_model_id: string | null;
	readonly source_run_id: string;
	readonly state: string;
	readonly target_engine_id: string | null;
	readonly target_model_id: string | null;
	readonly target_run_id: string | null;
	readonly thread_id: string;
	readonly updated_at: string;
}) =>
	Effect.try({
		try: () => JSON.parse(row.alternatives_json) as unknown,
		catch: (cause) => cause,
	}).pipe(
		Effect.flatMap((alternatives) =>
			Schema.decodeUnknownEffect(UsageInterruption)({
				affected_model_id: row.affected_model_id ?? undefined,
				alternatives,
				auto_continue: row.auto_continue,
				cancelled_at: row.cancelled_at ?? undefined,
				continuation_command_id: row.continuation_command_id ?? undefined,
				continued_at: row.continued_at ?? undefined,
				created_at: row.created_at,
				failed_at: row.failed_at ?? undefined,
				interruption_id: row.interruption_id,
				limit_id: row.limit_id ?? undefined,
				limit_label: row.limit_label ?? undefined,
				limit_scope: row.limit_scope,
				provider_code: row.provider_code ?? undefined,
				resets_at: row.resets_at ?? undefined,
				resume_not_before: row.resume_not_before ?? undefined,
				revision: row.revision,
				source_agent_id: row.source_agent_id,
				source_engine_id: row.source_engine_id,
				source_model_id: row.source_model_id ?? undefined,
				source_run_id: row.source_run_id,
				state: row.state,
				target_engine_id: row.target_engine_id ?? undefined,
				target_model_id: row.target_model_id ?? undefined,
				target_run_id: row.target_run_id ?? undefined,
				thread_id: row.thread_id,
				updated_at: row.updated_at,
			}),
		),
	) as Effect.Effect<UsageInterruptionSnapshot, unknown>;

/** Keeps provider evidence bounded, printable, and free of raw diagnostics. */
export const SanitiseUsageEvidenceText = (value: string | undefined) => {
	const cleaned = value
		?.split("")
		.map((character) => {
			const code = character.charCodeAt(0);
			return code <= 0x1f || code === 0x7f ? " " : character;
		})
		.join("")
		.trim()
		.slice(0, 256);
	return cleaned ? cleaned : undefined;
};
