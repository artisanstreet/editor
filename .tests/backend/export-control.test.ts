import { createHash } from "node:crypto";

import { Effect, Exit, Layer, Redacted } from "effect";
import { TestClock } from "effect/testing";
import { describe, expect, it } from "vitest";

import type { ExportControlPolicy } from "@artisan/protocol";

import {
	ExportControlAuditConflict,
	ExportControlAuditFailure,
	ExportControlAuditStore,
	ExportControlGate,
	ExportControlGateLive,
	ExportControlInputInvalid,
	ExportControlIntentCommitment,
	ExportControlIntentCommitmentFailure,
	ExportControlPolicySource,
	ExportControlPolicySourceFailure,
	ExportControlRestricted,
	ExportControlUnavailable,
	type ExportControlAuditCommit,
} from "../../modules/backend/src/compliance/export-control";
import { make_node_export_control_intent_commitment_layer } from "../../modules/backend/src/compliance/node-export-control-intent-commitment";

const now = Date.parse("2026-07-17T12:00:00.000Z");

function policy(denied_country_codes: ReadonlyArray<string> = ["RU"]): ExportControlPolicy {
	return {
		action_requirements: [
			{
				action: "marketplace_delivery",
				required_signal_kinds: ["account_country", "network_country"],
			},
			{ action: "release", required_signal_kinds: ["account_country"] },
		],
		denied_country_codes: denied_country_codes as [string, ...Array<string>],
		effective_at: "2026-01-01T00:00:00.000Z",
		expires_at: "2027-01-01T00:00:00.000Z",
		legal_review: {
			approved_at: "2026-01-01T00:00:00.000Z",
			expires_at: "2027-01-01T00:00:00.000Z",
			reference: "legal_review_1",
			status: "approved",
		},
		policy_id: "policy_export_1",
		schema_version: 1,
		support_url: "https://artisan.example/support/export-control",
		version: 1,
	};
}

function make_audit_store() {
	const entries = new Map<string, ExportControlAuditCommit>();
	const layer = Layer.succeed(ExportControlAuditStore, {
		Commit: (input) =>
			Effect.gen(function* () {
				const existing = entries.get(input.record.decision_id);

				if (existing) {
					if (existing.intent_fingerprint !== input.intent_fingerprint) {
						return yield* new ExportControlAuditConflict({
							decision_id: input.record.decision_id,
						});
					}

					return existing.decision;
				}

				entries.set(input.record.decision_id, input);

				return input.decision;
			}),
	});

	return { entries, layer };
}

function make_gate_layer(
	Load: Effect.Effect<unknown, ExportControlPolicySourceFailure>,
	audit_layer: Layer.Layer<ExportControlAuditStore>,
) {
	return ExportControlGateLive.pipe(
		Layer.provideMerge(Layer.succeed(ExportControlPolicySource, { Load })),
		Layer.provideMerge(audit_layer),
		Layer.provideMerge(
			make_node_export_control_intent_commitment_layer(
				Redacted.make(new Uint8Array(32).fill(17)),
			),
		),
		Layer.provideMerge(TestClock.layer()),
	);
}

describe("ExportControlGate", () => {
	it("uses stable key-separated commitments and rejects undersized keys", async () => {
		const Fingerprint = (fill: number) =>
			ExportControlIntentCommitment.pipe(
				Effect.flatMap((commitment) => commitment.Fingerprint("release:account-country")),
				Effect.provide(
					make_node_export_control_intent_commitment_layer(
						Redacted.make(new Uint8Array(32).fill(fill)),
					),
				),
				Effect.runPromise,
			);
		const first = await Fingerprint(7);
		const replay = await Fingerprint(7);
		const separated = await Fingerprint(8);
		const invalid = await Effect.runPromise(
			ExportControlIntentCommitment.pipe(
				Effect.provide(
					make_node_export_control_intent_commitment_layer(
						Redacted.make(new Uint8Array(31)),
					),
				),
				Effect.exit,
			),
		);

		expect(first).toMatch(/^[0-9a-f]{64}$/u);
		expect(replay).toBe(first);
		expect(separated).not.toBe(first);
		expect(Exit.isFailure(invalid)).toBe(true);
	});

	it("denies a restricted signal, audits no country data, and exact-replays across policy changes", async () => {
		let current_policy = policy();
		const audit = make_audit_store();
		const layer = make_gate_layer(
			Effect.sync(() => current_policy),
			audit.layer,
		);
		const request = {
			action: "marketplace_delivery",
			decision_id: "decision_restricted_1",
			signals: [
				{ country_code: "RU", kind: "account_country" },
				{ country_code: "NO", kind: "network_country" },
			],
		};
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				yield* TestClock.setTime(now);

				const gate = yield* ExportControlGate;
				const first = yield* gate.Check(request);

				current_policy = policy(["BY"]);

				const replay = yield* gate.Check(request);

				return { first, replay };
			}).pipe(Effect.provide(layer)),
		);

		expect(result.first).toMatchObject({
			code: "restricted_region",
			decision: "restricted",
		});
		expect(result.replay).toEqual(result.first);
		expect(audit.entries).toHaveLength(1);
		expect(
			JSON.stringify([...audit.entries.values()].map(({ record }) => record)),
		).not.toContain("RU");
		expect(audit.entries.get("decision_restricted_1")?.record.signal_kinds).toEqual([
			"account_country",
			"network_country",
		]);
		const plain_fingerprint = createHash("sha256")
			.update(
				JSON.stringify([
					"marketplace_delivery",
					[
						["account_country", "RU"],
						["network_country", "NO"],
					],
				]),
			)
			.digest("hex");

		expect(audit.entries.get("decision_restricted_1")?.intent_fingerprint).not.toBe(
			plain_fingerprint,
		);
	});

	it("allows only when every policy-required reliable signal is present", async () => {
		const audit = make_audit_store();
		const layer = make_gate_layer(Effect.succeed(policy()), audit.layer);
		const result = await Effect.runPromise(
			Effect.gen(function* () {
				yield* TestClock.setTime(now);

				const gate = yield* ExportControlGate;
				const insufficient = yield* gate.Check({
					action: "marketplace_delivery",
					decision_id: "decision_missing_signal",
					signals: [{ country_code: "NO", kind: "account_country" }],
				});
				const allowed = yield* gate.Check({
					action: "marketplace_delivery",
					decision_id: "decision_allowed",
					signals: [
						{ country_code: "NO", kind: "account_country" },
						{ country_code: "NO", kind: "network_country" },
					],
				});

				return { allowed, insufficient };
			}).pipe(Effect.provide(layer)),
		);

		expect(result.insufficient).toMatchObject({
			code: "insufficient_signals",
			decision: "unavailable",
		});
		expect(result.allowed).toMatchObject({ decision: "allowed" });
	});

	it("rejects duplicate signal kinds and ignores no locale surrogate", async () => {
		const audit = make_audit_store();
		const layer = make_gate_layer(Effect.succeed(policy()), audit.layer);
		const failures = await Effect.runPromise(
			Effect.gen(function* () {
				yield* TestClock.setTime(now);

				const gate = yield* ExportControlGate;
				const duplicate = yield* Effect.flip(
					gate.Check({
						action: "marketplace_delivery",
						decision_id: "decision_duplicate_signal",
						signals: [
							{ country_code: "NO", kind: "account_country" },
							{ country_code: "RU", kind: "account_country" },
						],
					}),
				);
				const locale = yield* Effect.flip(
					gate.Check({
						action: "release",
						decision_id: "decision_locale",
						locale: "ru-RU",
						signals: [{ country_code: "NO", kind: "account_country" }],
					}),
				);

				return { duplicate, locale };
			}).pipe(Effect.provide(layer)),
		);

		expect(failures.duplicate).toBeInstanceOf(ExportControlInputInvalid);
		expect(failures.locale).toBeInstanceOf(ExportControlInputInvalid);
		expect(audit.entries).toHaveLength(0);
	});

	it("fails closed for unavailable policy, stale legal review, and audit failure", async () => {
		const audit = make_audit_store();
		const unavailable_layer = make_gate_layer(
			Effect.fail(new ExportControlPolicySourceFailure({})),
			audit.layer,
		);
		const stale_policy = {
			...policy(),
			legal_review: { ...policy().legal_review, expires_at: "2026-07-17T11:59:59.000Z" },
		};
		const stale_layer = make_gate_layer(Effect.succeed(stale_policy), audit.layer);
		const failing_audit = Layer.succeed(ExportControlAuditStore, {
			Commit: () => Effect.fail(new ExportControlAuditFailure({})),
		});
		const audit_failure_layer = make_gate_layer(Effect.succeed(policy()), failing_audit);
		const request = {
			action: "release",
			signals: [{ country_code: "NO", kind: "account_country" }],
		};
		const Check = (
			layer: Layer.Layer<ExportControlGate, ExportControlIntentCommitmentFailure>,
			decision_id: string,
		) =>
			Effect.gen(function* () {
				yield* TestClock.setTime(now);

				return yield* ExportControlGate.pipe(
					Effect.flatMap((gate) => gate.Check({ ...request, decision_id })),
				);
			}).pipe(Effect.provide(layer), Effect.runPromise);
		const unavailable = await Check(unavailable_layer, "decision_policy_unavailable");
		const stale = await Check(stale_layer, "decision_stale_policy");
		const audit_failure = await Check(audit_failure_layer, "decision_audit_failure");

		expect(unavailable).toMatchObject({ code: "policy_unavailable", decision: "unavailable" });
		expect(stale).toMatchObject({ code: "invalid_policy", decision: "unavailable" });
		expect(audit_failure).toMatchObject({ code: "audit_unavailable", decision: "unavailable" });
	});

	it("turns non-allowed decisions into explicit Require failures", async () => {
		const audit = make_audit_store();
		const layer = make_gate_layer(Effect.succeed(policy()), audit.layer);
		const failures = await Effect.runPromise(
			Effect.gen(function* () {
				yield* TestClock.setTime(now);

				const gate = yield* ExportControlGate;
				const restricted = yield* Effect.flip(
					gate.Require({
						action: "release",
						decision_id: "decision_require_restricted",
						signals: [{ country_code: "RU", kind: "account_country" }],
					}),
				);
				const unavailable = yield* Effect.flip(
					gate.Require({
						action: "marketplace_delivery",
						decision_id: "decision_require_unavailable",
						signals: [{ country_code: "NO", kind: "account_country" }],
					}),
				);

				return { restricted, unavailable };
			}).pipe(Effect.provide(layer)),
		);

		expect(failures.restricted).toBeInstanceOf(ExportControlRestricted);
		expect(failures.unavailable).toBeInstanceOf(ExportControlUnavailable);
	});
});
