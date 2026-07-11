import { Effect, Schema } from "effect";
import { describe, expect, it } from "vitest";

import {
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	ModelBehaviourCapability,
	ModelBehaviourSnapshot,
	ModelBehaviourUpdateRequest,
} from "@artisan/protocol";

const hash_a = "a".repeat(64);
const timestamp = "2026-07-11T12:00:00.000Z";

function frontend_envelope(kind: string, payload: unknown) {
	return {
		kind,
		message_id: `message_${kind}`,
		origin: "frontend",
		payload,
		protocol_version: 1,
		schema_version: 1,
		sent_at: timestamp,
	};
}

const decode_capability = Schema.decodeUnknownSync(ModelBehaviourCapability, {
	onExcessProperty: "error",
});
const decode_snapshot = Schema.decodeUnknownSync(ModelBehaviourSnapshot, {
	onExcessProperty: "error",
});
const decode_update = Schema.decodeUnknownSync(ModelBehaviourUpdateRequest, {
	onExcessProperty: "error",
});

describe("model behaviour protocol", () => {
	it("projects a provider-neutral compaction trigger with truthful provider support", () => {
		const capability = decode_capability({
			control: {
				kind: "integer",
				maximum: 2_000_000,
				minimum: 16_384,
				step: 128,
				unit: "tokens",
			},
			description: "Token threshold that triggers automatic history compaction.",
			display_name: "Auto-compaction trigger",
			provider_support: [
				{
					activation_timing: "new_threads",
					details: "Codex reads this global value when a thread starts.",
					minimum_version: "0.142.5",
					native_key: "model_auto_compact_token_limit",
					provider_id: "codex",
					state: "supported",
				},
				{
					activation_timing: "new_threads",
					details: "Claude Code has no equivalent supported mapping.",
					provider_id: "claude",
					state: "unsupported",
				},
			],
			scope: "global_default",
			setting_id: "auto_compaction_trigger_tokens",
		});

		expect(capability.provider_support.map((provider) => provider.state)).toEqual([
			"supported",
			"unsupported",
		]);
	});

	it("accepts explicit tokens or provider default and rejects context-sized nonsense", () => {
		expect(
			decode_update({
				setting_id: "auto_compaction_trigger_tokens",
				value: { type: "integer", value: 250_000 },
			}),
		).toMatchObject({ value: { value: 250_000 } });
		expect(
			decode_update({
				setting_id: "auto_compaction_trigger_tokens",
				value: { type: "provider_default" },
			}),
		).toMatchObject({ value: { type: "provider_default" } });
		expect(() =>
			decode_update({
				setting_id: "auto_compaction_trigger_tokens",
				value: { type: "integer", value: 20_000_000 },
			}),
		).toThrow();
	});

	it("rejects reversed and step-incompatible integer control ranges", () => {
		const base = {
			kind: "integer",
			maximum: 2_000_000,
			minimum: 16_384,
			step: 128,
			unit: "tokens",
		};
		const decode_control = Schema.decodeUnknownSync(ModelBehaviourCapability.fields.control);

		expect(() => decode_control({ ...base, maximum: 16_384, minimum: 2_000_000 })).toThrow();
		expect(() => decode_control({ ...base, step: 1_024 })).toThrow();
	});

	it("requires complete versioned snapshots and rejects raw provider config blobs", () => {
		const snapshot = decode_snapshot({
			capabilities: [],
			providers: [],
			registry_version: 1,
			settings: [
				{
					setting_id: "auto_compaction_trigger_tokens",
					updated_at: "2026-07-11T12:00:00.000Z",
					value: { type: "provider_default" },
					version: 1,
				},
			],
		});

		expect(snapshot.registry_version).toBe(1);
		expect(() =>
			decode_snapshot({
				...snapshot,
				raw_provider_config: "api_key = 'secret'",
			}),
		).toThrow();
	});

	it("decodes every Model Behaviour request envelope", async () => {
		const envelopes = [
			frontend_envelope("model_behaviour.query", {}),
			frontend_envelope("model_behaviour.update", {
				setting_id: "auto_compaction_trigger_tokens",
				value: { type: "integer", value: 250_000 },
			}),
			frontend_envelope("model_behaviour.drift.resolve", {
				action: "ignore",
				observed_hash: hash_a,
				provider_id: "codex",
				setting_id: "auto_compaction_trigger_tokens",
			}),
			frontend_envelope("model_behaviour.sync.retry", {
				provider_id: "codex",
				setting_id: "auto_compaction_trigger_tokens",
			}),
		];

		const decoded = await Promise.all(
			envelopes.map((envelope) => Effect.runPromise(DecodeInboundControlEnvelope(envelope))),
		);

		expect(decoded).toEqual(envelopes);
	});

	it("decodes content-free snapshots and the acknowledged drift state", async () => {
		const envelope = {
			correlation_id: "model_behaviour_query",
			kind: "model_behaviour.query.result",
			message_id: "model_behaviour_result",
			origin: "backend",
			payload: {
				capabilities: [],
				providers: [
					{
						ignored_drift_hash: hash_a,
						observed_hash: hash_a,
						provider_id: "codex",
						setting_id: "auto_compaction_trigger_tokens",
						status: "drift_ignored",
						updated_at: timestamp,
					},
				],
				registry_version: 1,
				settings: [
					{
						setting_id: "auto_compaction_trigger_tokens",
						updated_at: timestamp,
						value: { type: "provider_default" },
						version: 1,
					},
				],
			},
			protocol_version: 1,
			schema_version: 1,
			sent_at: timestamp,
		};

		await expect(Effect.runPromise(DecodeOutboundControlEnvelope(envelope))).resolves.toEqual(
			envelope,
		);
		expect(() =>
			decode_snapshot({
				...envelope.payload,
				providers: [{ ...envelope.payload.providers[0], raw_config: "api_key='secret'" }],
			}),
		).toThrow();
	});
});
