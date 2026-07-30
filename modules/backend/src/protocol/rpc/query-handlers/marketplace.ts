import { Effect } from "effect";

import type {
	CapabilityConnectPreviewEnvelope,
	CapabilityConnectPreviewResultEnvelope,
	CapabilityDetailQueryEnvelope,
	CapabilityDetailQueryResultEnvelope,
	CapabilityOAuthTokenStatusEnvelope,
	CapabilityOAuthTokenStatusResultEnvelope,
	CapabilityRegistryQueryEnvelope,
	CapabilityRegistryQueryResultEnvelope,
	GlobalGuidanceQueryEnvelope,
	GlobalGuidanceQueryResultEnvelope,
	MarketplaceScope,
	ModelBehaviourQueryEnvelope,
	ModelBehaviourQueryResultEnvelope,
	NpxSkillsDiscoverEnvelope,
	NpxSkillsDiscoverResultEnvelope,
	ProtocolErrorDetail,
	RoutineDetailQueryEnvelope,
	RoutineDetailQueryResultEnvelope,
	RoutineInstallPreviewEnvelope,
	RoutineInstallPreviewResultEnvelope,
	RoutineRegistryQueryEnvelope,
	RoutineRegistryQueryResultEnvelope,
	SecretReference,
} from "@artisan/protocol";

import { GlobalGuidanceService } from "../../../guidance/service";
import { CapabilityRepository } from "../../../marketplace/capabilities/repository";
import {
	CapabilityOAuthLifecycle,
	CapabilityService,
} from "../../../marketplace/capabilities/service";
import { RoutineService } from "../../../marketplace/routines/service";
import { ModelBehaviourService } from "../../../model-behaviour/service";
import { RuntimeMetadata } from "../../../runtime/metadata";

export type MarketplaceQueryEnvelope =
	| CapabilityConnectPreviewEnvelope
	| CapabilityDetailQueryEnvelope
	| CapabilityOAuthTokenStatusEnvelope
	| CapabilityRegistryQueryEnvelope
	| GlobalGuidanceQueryEnvelope
	| ModelBehaviourQueryEnvelope
	| NpxSkillsDiscoverEnvelope
	| RoutineDetailQueryEnvelope
	| RoutineInstallPreviewEnvelope
	| RoutineRegistryQueryEnvelope;

export type MarketplaceQueryResultEnvelope =
	| CapabilityConnectPreviewResultEnvelope
	| CapabilityDetailQueryResultEnvelope
	| CapabilityOAuthTokenStatusResultEnvelope
	| CapabilityRegistryQueryResultEnvelope
	| GlobalGuidanceQueryResultEnvelope
	| ModelBehaviourQueryResultEnvelope
	| NpxSkillsDiscoverResultEnvelope
	| RoutineDetailQueryResultEnvelope
	| RoutineInstallPreviewResultEnvelope
	| RoutineRegistryQueryResultEnvelope;

const ScopeMatches = (left: MarketplaceScope, right: MarketplaceScope) =>
	left.kind === right.kind &&
	(left.kind === "global" ||
		(left.kind === "workspace" &&
			right.kind === "workspace" &&
			left.workspace_id === right.workspace_id) ||
		(left.kind === "project" &&
			right.kind === "project" &&
			left.project_id === right.project_id));

const GuidanceUnavailable: ProtocolErrorDetail = {
	code: "guidance.unavailable",
	message: "Global guidance could not be read or reconciled.",
	retryable: true,
};

const ModelBehaviourUnavailable: ProtocolErrorDetail = {
	code: "model_behaviour.unavailable",
	message: "Model Behaviour settings could not be read or reconciled.",
	retryable: true,
};

const MarketplaceUnavailable: ProtocolErrorDetail = {
	code: "marketplace.unavailable",
	message: "The Marketplace operation could not be completed.",
	retryable: true,
};

const literal = <Value extends string | number>(value: Value): Value => value;

const backend_origin = literal("backend");
const protocol_version = literal(1);
const registry_version = literal(1);
const schema_version = literal(1);

type OAuthStatusPayload = CapabilityOAuthTokenStatusResultEnvelope["payload"];

const OAuthStatusPayload = (status: {
	readonly capability_id: string;
	readonly secret_reference?: SecretReference;
	readonly state: "absent" | "active" | "expired" | "revoked";
}): OAuthStatusPayload => ({
	capability_id: status.capability_id,
	...(status.secret_reference === undefined ? {} : { secret_ref: status.secret_reference }),
	status:
		status.state === "absent"
			? "not_started"
			: status.state === "active"
				? "authorized"
				: status.state,
});

export const MakeMarketplaceQueryHandler = Effect.gen(function* () {
	const capabilities = yield* CapabilityService;
	const capability_oauth = yield* CapabilityOAuthLifecycle;
	const capability_repository = yield* CapabilityRepository;
	const guidance = yield* GlobalGuidanceService;
	const metadata = yield* RuntimeMetadata;
	const model_behaviour = yield* ModelBehaviourService;
	const routines = yield* RoutineService;

	const Envelope = <Kind extends MarketplaceQueryResultEnvelope["kind"], Payload>(
		query: MarketplaceQueryEnvelope,
		kind: Kind,
		payload: Payload,
	) =>
		Effect.gen(function* () {
			return {
				correlation_id: query.message_id,
				kind,
				message_id: yield* metadata.MakeId("message"),
				origin: backend_origin,
				payload,
				protocol_version,
				schema_version,
				sent_at: yield* metadata.Now,
			};
		});

	const handlers = {
		"guidance.query": (query: GlobalGuidanceQueryEnvelope) =>
			guidance.Get.pipe(
				Effect.flatMap((payload) => Envelope(query, "guidance.query.result", payload)),
				Effect.mapError(() => GuidanceUnavailable),
			),
		"model_behaviour.query": (query: ModelBehaviourQueryEnvelope) =>
			model_behaviour.Get.pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "model_behaviour.query.result", payload),
				),
				Effect.mapError(() => ModelBehaviourUnavailable),
			),
		"marketplace.routine.list.query": (query: RoutineRegistryQueryEnvelope) =>
			routines.Browse(query.payload).pipe(
				Effect.map((routines) => ({ registry_version, routines })),
				Effect.flatMap((payload) =>
					Envelope(query, "marketplace.routine.list.query.result", payload),
				),
				Effect.mapError(() => MarketplaceUnavailable),
			),
		"marketplace.routine.detail.query": (query: RoutineDetailQueryEnvelope) =>
			routines.Detail(query.payload.routine_id).pipe(
				Effect.filterOrFail(
					(detail) => ScopeMatches(detail.scope, query.payload.scope),
					() => MarketplaceUnavailable,
				),
				Effect.flatMap((payload) =>
					Envelope(query, "marketplace.routine.detail.query.result", payload),
				),
				Effect.mapError(() => MarketplaceUnavailable),
			),
		"marketplace.routine.install.preview": (query: RoutineInstallPreviewEnvelope) =>
			routines.Preview(query.payload).pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "marketplace.routine.install.preview.result", payload),
				),
				Effect.mapError(() => MarketplaceUnavailable),
			),
		"marketplace.npx_skills.discover": (query: NpxSkillsDiscoverEnvelope) =>
			routines.DiscoverNpxSkills(query.payload).pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "marketplace.npx_skills.discover.result", payload),
				),
				Effect.mapError(() => MarketplaceUnavailable),
			),
		"marketplace.capability.list.query": (query: CapabilityRegistryQueryEnvelope) =>
			capability_repository.ReadSummaries.pipe(
				Effect.flatMap((capabilities) =>
					Effect.forEach(capabilities, (summary) =>
						capability_repository
							.ReadDetail(summary.id)
							.pipe(Effect.map((detail) => ({ detail, summary }))),
					),
				),
				Effect.map((records) => ({
					capabilities: records
						.filter(
							({ detail, summary: capability }) =>
								(query.payload.compatibility_engine_id === undefined ||
									detail.compatibility.some(
										(entry) =>
											entry.engine_id ===
											query.payload.compatibility_engine_id,
									)) &&
								(query.payload.category === undefined ||
									query.payload.category === "capability") &&
								(query.payload.enabled === undefined ||
									capability.enabled === query.payload.enabled) &&
								(query.payload.status === undefined ||
									capability.status === query.payload.status) &&
								(query.payload.scope === undefined ||
									ScopeMatches(capability.scope, query.payload.scope)) &&
								(query.payload.text === undefined ||
									capability.display_name
										.toLocaleLowerCase()
										.includes(query.payload.text.toLocaleLowerCase())),
						)
						.map(({ summary }) => summary),
					registry_version,
				})),
				Effect.flatMap((payload) =>
					Envelope(query, "marketplace.capability.list.query.result", payload),
				),
				Effect.mapError(() => MarketplaceUnavailable),
			),
		"marketplace.capability.detail.query": (query: CapabilityDetailQueryEnvelope) =>
			capability_repository.ReadDetail(query.payload.capability_id).pipe(
				Effect.filterOrFail(
					(detail) => ScopeMatches(detail.scope, query.payload.scope),
					() => MarketplaceUnavailable,
				),
				Effect.flatMap((payload) =>
					Envelope(query, "marketplace.capability.detail.query.result", payload),
				),
				Effect.mapError(() => MarketplaceUnavailable),
			),
		"marketplace.capability.connect.preview": (query: CapabilityConnectPreviewEnvelope) =>
			capabilities.Preview(query.payload).pipe(
				Effect.flatMap((payload) =>
					Envelope(query, "marketplace.capability.connect.preview.result", payload),
				),
				Effect.mapError(() => MarketplaceUnavailable),
			),
		"marketplace.capability.oauth.status.query": (query: CapabilityOAuthTokenStatusEnvelope) =>
			capability_repository.ReadDetail(query.payload.capability_id).pipe(
				Effect.filterOrFail(
					(detail) => ScopeMatches(detail.scope, query.payload.scope),
					() => MarketplaceUnavailable,
				),
				Effect.andThen(capability_oauth.Status(query.payload.capability_id)),
				Effect.map(OAuthStatusPayload),
				Effect.flatMap((payload) =>
					Envelope(query, "marketplace.capability.oauth.status.query.result", payload),
				),
				Effect.mapError(() => MarketplaceUnavailable),
			),
	};

	return (
		query: MarketplaceQueryEnvelope,
	): Effect.Effect<MarketplaceQueryResultEnvelope, ProtocolErrorDetail> => {
		switch (query.kind) {
			case "guidance.query":
				return handlers["guidance.query"](query);
			case "model_behaviour.query":
				return handlers["model_behaviour.query"](query);
			case "marketplace.routine.list.query":
				return handlers["marketplace.routine.list.query"](query);
			case "marketplace.routine.detail.query":
				return handlers["marketplace.routine.detail.query"](query);
			case "marketplace.routine.install.preview":
				return handlers["marketplace.routine.install.preview"](query);
			case "marketplace.npx_skills.discover":
				return handlers["marketplace.npx_skills.discover"](query);
			case "marketplace.capability.list.query":
				return handlers["marketplace.capability.list.query"](query);
			case "marketplace.capability.detail.query":
				return handlers["marketplace.capability.detail.query"](query);
			case "marketplace.capability.connect.preview":
				return handlers["marketplace.capability.connect.preview"](query);
			case "marketplace.capability.oauth.status.query":
				return handlers["marketplace.capability.oauth.status.query"](query);
		}
	};
});
