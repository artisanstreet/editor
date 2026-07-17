import { eq } from "drizzle-orm";
import { Effect, Layer, Schema } from "effect";

import {
	ExportControlAuditRecord,
	ExportControlDecision,
	type ExportControlAuditRecord as ExportControlAuditRecordValue,
	type ExportControlDecision as ExportControlDecisionValue,
} from "@artisan/protocol";

import { Database } from "../persistence/database";
import { ExportControlAuditDecisions } from "../persistence/schema";
import { RetrySqliteWrite } from "../persistence/sqlite-write-retry";
import {
	ExportControlAuditConflict,
	ExportControlAuditFailure,
	ExportControlAuditStore,
	type ExportControlAuditCommit,
} from "./export-control";

const DecodeDecision = Schema.decodeUnknownEffect(ExportControlDecision, {
	onExcessProperty: "error",
});

const DecodeRecord = Schema.decodeUnknownEffect(ExportControlAuditRecord, {
	onExcessProperty: "error",
});

function failure(cause: unknown) {
	return new ExportControlAuditFailure({ cause });
}

function conflict(decision_id: string) {
	return new ExportControlAuditConflict({ decision_id });
}

function string_arrays_equal(left: ReadonlyArray<string>, right: ReadonlyArray<string>) {
	return left.length === right.length && left.every((value, index) => value === right[index]);
}

const ParseJson = (value: string, context: string) =>
	Effect.try({
		try: () => JSON.parse(value) as unknown,
		catch: (cause) => failure(new Error(`${context} contains invalid JSON`, { cause })),
	});

function decisions_match_record(
	decision: ExportControlDecisionValue,
	record: ExportControlAuditRecordValue,
) {
	const policy_metadata_is_complete =
		(record.policy_id === undefined) === (record.policy_version === undefined);
	const signal_kinds_are_unique =
		new Set(record.signal_kinds).size === record.signal_kinds.length;

	return (
		policy_metadata_is_complete &&
		signal_kinds_are_unique &&
		decision.decision_id === record.decision_id &&
		decision.decision === record.decision &&
		decision.policy_id === record.policy_id &&
		decision.policy_version === record.policy_version &&
		(decision.decision === "allowed"
			? record.reason_code === "allowed"
			: decision.code === record.reason_code)
	);
}

const ValidateCommit = (input: ExportControlAuditCommit) =>
	Effect.gen(function* () {
		const decision = yield* DecodeDecision(input.decision).pipe(Effect.mapError(failure));
		const record = yield* DecodeRecord(input.record).pipe(Effect.mapError(failure));

		if (!/^[0-9a-f]{64}$/u.test(input.intent_fingerprint)) {
			return yield* failure(new Error("Export-control intent fingerprint is invalid"));
		}

		if (!decisions_match_record(decision, record)) {
			return yield* failure(
				new Error("Export-control decision and audit record are inconsistent"),
			);
		}

		return { decision, record };
	});

/** Persists privacy-bounded export-control decisions with process-safe exact replay. */
export const SQLiteExportControlAuditStoreLive = Layer.effect(
	ExportControlAuditStore,
	Effect.gen(function* () {
		const database = yield* Database;

		const Commit = (input: ExportControlAuditCommit) =>
			Effect.gen(function* () {
				const validated = yield* ValidateCommit(input);
				const decision_json = JSON.stringify(
					Schema.encodeSync(ExportControlDecision)(validated.decision),
				);
				const record_json = JSON.stringify(
					Schema.encodeSync(ExportControlAuditRecord)(validated.record),
				);
				const write = database.client.transaction((transaction) =>
					Effect.gen(function* () {
						yield* transaction
							.insert(ExportControlAuditDecisions)
							.values({
								action: validated.record.action,
								created_at: validated.record.occurred_at,
								decision_id: validated.record.decision_id,
								decision_json,
								intent_fingerprint: input.intent_fingerprint,
								record_json,
							})
							.onConflictDoNothing({
								target: ExportControlAuditDecisions.decision_id,
							});

						const [stored] = yield* transaction
							.select({
								action: ExportControlAuditDecisions.action,
								decision_json: ExportControlAuditDecisions.decision_json,
								intent_fingerprint: ExportControlAuditDecisions.intent_fingerprint,
								record_json: ExportControlAuditDecisions.record_json,
							})
							.from(ExportControlAuditDecisions)
							.where(
								eq(
									ExportControlAuditDecisions.decision_id,
									validated.record.decision_id,
								),
							)
							.limit(1);

						if (!stored) {
							return yield* failure(
								new Error("Export-control audit decision was not persisted"),
							);
						}

						if (stored.intent_fingerprint !== input.intent_fingerprint) {
							return yield* conflict(validated.record.decision_id);
						}

						const stored_decision = yield* ParseJson(
							stored.decision_json,
							`Export-control decision ${validated.record.decision_id}`,
						).pipe(Effect.flatMap(DecodeDecision), Effect.mapError(failure));
						const stored_record = yield* ParseJson(
							stored.record_json,
							`Export-control audit ${validated.record.decision_id}`,
						).pipe(Effect.flatMap(DecodeRecord), Effect.mapError(failure));

						if (
							stored.action !== validated.record.action ||
							stored_record.action !== validated.record.action ||
							stored_record.decision_id !== validated.record.decision_id ||
							!string_arrays_equal(
								stored_record.signal_kinds,
								validated.record.signal_kinds,
							) ||
							!decisions_match_record(stored_decision, stored_record)
						) {
							return yield* failure(
								new Error("Export-control audit decision is corrupt"),
							);
						}

						return stored_decision;
					}),
				);

				return yield* RetrySqliteWrite(write);
			}).pipe(
				Effect.mapError((cause) =>
					cause instanceof ExportControlAuditConflict ? cause : failure(cause),
				),
			);

		return { Commit };
	}),
);
