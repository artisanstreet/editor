import { Effect } from "effect";
import {
	type GlobalGuidanceDriftResolutionEnvelope,
	type GlobalGuidanceQueryEnvelope,
	type GlobalGuidanceRetryEnvelope,
	type GlobalGuidanceSelectionEnvelope,
	type GlobalGuidanceUpdateEnvelope,
	type ModelBehaviourDriftResolutionEnvelope,
	type ModelBehaviourQueryEnvelope,
	type ModelBehaviourRetryEnvelope,
	type ModelBehaviourUpdateEnvelope,
} from "@artisan/protocol";
import type {
	ArtisanCommandReceipt,
	ArtisanGlobalGuidanceDriftInput,
	ArtisanGlobalGuidanceRetryInput,
	ArtisanGlobalGuidanceSelectionInput,
	ArtisanGlobalGuidanceUpdateInput,
	ArtisanModelBehaviourDriftInput,
	ArtisanModelBehaviourRetryInput,
	ArtisanModelBehaviourUpdateInput,
} from "../../client-api/service";
import { client_error } from "../client-common";
import { ClientApiContext } from "./context";

export const MakeGuidanceApi = Effect.gen(function* () {
	const context = yield* ClientApiContext;
	const get_global_guidance = Effect.gen(function* () {
		const trace = yield* context.MakeTrace;
		const envelope: GlobalGuidanceQueryEnvelope = {
			...trace,
			kind: "guidance.query",
			payload: {},
		};
		const result = yield* context.Request(envelope);

		return result.kind === "guidance.query.result"
			? result.payload
			: yield* Effect.die("global guidance response narrowed incorrectly");
	});

	type GuidanceMutationEnvelope =
		| GlobalGuidanceDriftResolutionEnvelope
		| GlobalGuidanceRetryEnvelope
		| GlobalGuidanceSelectionEnvelope
		| GlobalGuidanceUpdateEnvelope;
	const send_guidance_mutation = (envelope: GuidanceMutationEnvelope) =>
		Effect.gen(function* () {
			const result = yield* context.Request(envelope);

			if (result.kind !== "command.receipt") {
				return yield* Effect.die("global guidance receipt narrowed incorrectly");
			}

			if (result.payload.status === "rejected") {
				return yield* Effect.fail(
					client_error(
						"protocol",
						result.payload.error.message,
						result.payload.error,
						result.payload.error.retryable,
						result.payload.error.code,
					),
				);
			}

			return {
				command_id: envelope.message_id,
				journal_sequence: result.payload.journal_sequence,
				status: result.payload.status,
			} satisfies ArtisanCommandReceipt;
		});
	const update_global_guidance = (input: ArtisanGlobalGuidanceUpdateInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: GlobalGuidanceUpdateEnvelope = {
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "guidance.update",
				payload: { content: input.content },
			};

			return yield* send_guidance_mutation(envelope);
		});
	const select_global_guidance = (input: ArtisanGlobalGuidanceSelectionInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: GlobalGuidanceSelectionEnvelope = {
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "guidance.selection",
				payload: {
					content_hash: input.content_hash,
					provider: input.provider,
				},
			};

			return yield* send_guidance_mutation(envelope);
		});
	const resolve_global_guidance_drift = (input: ArtisanGlobalGuidanceDriftInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: GlobalGuidanceDriftResolutionEnvelope = {
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "guidance.drift.resolve",
				payload: {
					action: input.action,
					observed_hash: input.observed_hash,
					provider: input.provider,
				},
			};

			return yield* send_guidance_mutation(envelope);
		});
	const retry_global_guidance_sync = (input: ArtisanGlobalGuidanceRetryInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: GlobalGuidanceRetryEnvelope = {
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "guidance.sync.retry",
				payload: { provider: input.provider },
			};

			return yield* send_guidance_mutation(envelope);
		});

	const get_model_behaviour = Effect.gen(function* () {
		const trace = yield* context.MakeTrace;
		const envelope: ModelBehaviourQueryEnvelope = {
			...trace,
			kind: "model_behaviour.query",
			payload: {},
		};
		const result = yield* context.Request(envelope);

		return result.kind === "model_behaviour.query.result"
			? result.payload
			: yield* Effect.die("Model Behaviour response narrowed incorrectly");
	});

	type ModelBehaviourMutationEnvelope =
		| ModelBehaviourDriftResolutionEnvelope
		| ModelBehaviourRetryEnvelope
		| ModelBehaviourUpdateEnvelope;
	const send_model_behaviour_mutation = (envelope: ModelBehaviourMutationEnvelope) =>
		Effect.gen(function* () {
			const result = yield* context.Request(envelope);

			if (result.kind !== "command.receipt") {
				return yield* Effect.die("Model Behaviour receipt narrowed incorrectly");
			}

			if (result.payload.status === "rejected") {
				return yield* Effect.fail(
					client_error(
						"protocol",
						result.payload.error.message,
						result.payload.error,
						result.payload.error.retryable,
						result.payload.error.code,
					),
				);
			}

			return {
				command_id: envelope.message_id,
				journal_sequence: result.payload.journal_sequence,
				status: result.payload.status,
			} satisfies ArtisanCommandReceipt;
		});
	const update_model_behaviour = (input: ArtisanModelBehaviourUpdateInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: ModelBehaviourUpdateEnvelope = {
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "model_behaviour.update",
				payload: {
					setting_id: input.setting_id,
					value: input.value,
				},
			};

			return yield* send_model_behaviour_mutation(envelope);
		});
	const resolve_model_behaviour_drift = (input: ArtisanModelBehaviourDriftInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: ModelBehaviourDriftResolutionEnvelope = {
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "model_behaviour.drift.resolve",
				payload: {
					action: input.action,
					observed_hash: input.observed_hash,
					provider_id: input.provider_id,
					setting_id: input.setting_id,
				},
			};

			return yield* send_model_behaviour_mutation(envelope);
		});
	const retry_model_behaviour_sync = (input: ArtisanModelBehaviourRetryInput) =>
		Effect.gen(function* () {
			const trace = yield* context.MakeTrace;
			const envelope: ModelBehaviourRetryEnvelope = {
				...trace,
				message_id: input.command_id ?? trace.message_id,
				kind: "model_behaviour.sync.retry",
				payload: {
					provider_id: input.provider_id,
					setting_id: input.setting_id,
				},
			};

			return yield* send_model_behaviour_mutation(envelope);
		});

	return {
		get_global_guidance,
		get_model_behaviour,
		resolve_global_guidance_drift,
		resolve_model_behaviour_drift,
		retry_global_guidance_sync,
		retry_model_behaviour_sync,
		select_global_guidance,
		update_global_guidance,
		update_model_behaviour,
	};
});
