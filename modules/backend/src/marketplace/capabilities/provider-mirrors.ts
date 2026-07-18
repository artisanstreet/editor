import { createHash } from "node:crypto";

import { Context, Data, Effect, Layer } from "effect";
import type { MarketplaceScope, ProviderSyncState } from "@artisan/protocol";

import { CapabilityRepository } from "./capability-repository";
import { RuntimeMetadata } from "../../runtime/runtime-metadata";

const FailureState = (
	engine_id: string,
	error: CapabilityProviderMirrorError,
	updated_at: string,
	observed_revision?: string,
): ProviderSyncState => ({
	engine_id,
	last_error_code: error.code,
	...(observed_revision === undefined ? {} : { observed_revision }),
	status: error.code === "unsupported" ? "unsupported" : "sync_failed",
	updated_at,
});

const Fingerprint = (value: unknown) =>
	createHash("sha256").update(JSON.stringify(value)).digest("hex");

const ScopeMatches = (left: MarketplaceScope, right: MarketplaceScope) =>
	JSON.stringify(left) === JSON.stringify(right);

/** Native config synchronization is deliberately injectable: the canonical registry never writes provider files itself. */
export class CapabilityProviderMirror extends Context.Service<
	CapabilityProviderMirror,
	{
		readonly Sync: (input: {
			readonly capability_id: string;
			readonly engine_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<ProviderSyncState, CapabilityProviderMirrorError>;
		readonly ResolveDrift: (input: {
			readonly action: "ignore" | "import" | "overwrite";
			readonly approved?: boolean;
			readonly capability_id: string;
			readonly engine_id: string;
			readonly observed_revision: string;
		}) => Effect.Effect<ProviderSyncState, CapabilityProviderMirrorError>;
	}
>()("Artisan/Marketplace/CapabilityProviderMirror") {}

export class CapabilityProviderMirrorError extends Data.TaggedError(
	"CapabilityProviderMirrorError",
)<{
	readonly code: "unsupported" | "unavailable";
}> {}

/** Default behavior records no side effect and makes unsupported/runtime-only status explicit. */
export const EmptyCapabilityProviderMirrorLive = Layer.effect(
	CapabilityProviderMirror,
	Effect.gen(function* () {
		const metadata = yield* RuntimeMetadata;
		return {
			ResolveDrift: ({ action, engine_id, observed_revision }) =>
				metadata.Now.pipe(
					Effect.map((updated_at) => ({
						engine_id,
						observed_revision,
						status: action === "ignore" ? "drift_ignored" : "runtime_only",
						updated_at,
					})),
				),
			Sync: ({ engine_id }) =>
				metadata.Now.pipe(
					Effect.map((updated_at) => ({ engine_id, status: "runtime_only", updated_at })),
				),
		};
	}),
);

export class CapabilityMirrorService extends Context.Service<
	CapabilityMirrorService,
	{
		readonly Sync: (input: {
			readonly capability_id: string;
			readonly engine_id: string;
			readonly operation_id: string;
		}) => Effect.Effect<ProviderSyncState, never>;
		readonly ResolveDrift: (input: {
			readonly action: "ignore" | "import";
			readonly capability_id: string;
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly operation_id: string;
		}) => Effect.Effect<ProviderSyncState, never>;
		readonly RequestOverwrite: (input: {
			readonly approval_fingerprint: string;
			readonly approval_id: string;
			readonly capability_id: string;
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly operation_id: string;
			readonly scope: MarketplaceScope;
		}) => Effect.Effect<void, never>;
		readonly DecideOverwrite: (input: {
			readonly approval_fingerprint: string;
			readonly approval_id: string;
			readonly approved: boolean;
			readonly capability_id: string;
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly scope: MarketplaceScope;
		}) => Effect.Effect<ProviderSyncState, never>;
	}
>()("Artisan/Marketplace/CapabilityMirrorService") {}

export const CapabilityMirrorServiceLive = Layer.effect(
	CapabilityMirrorService,
	Effect.gen(function* () {
		const repository = yield* CapabilityRepository;
		const mirror = yield* CapabilityProviderMirror;
		const metadata = yield* RuntimeMetadata;
		const Sync = (input: {
			readonly capability_id: string;
			readonly engine_id: string;
			readonly operation_id: string;
		}) =>
			Effect.gen(function* () {
				/** Reject orphan mirrors before asking an adapter to write native configuration. */
				const detail = yield* repository.ReadDetail(input.capability_id);
				yield* repository.RecordProviderSync(input);
				const claim = yield* repository.ClaimProviderSync(input.operation_id);
				if (claim === "syncing")
					return yield* new CapabilityProviderMirrorError({ code: "unavailable" });
				if (claim === "completed") {
					const state = detail.sync.find((entry) => entry.engine_id === input.engine_id);
					if (state) return state;
					return yield* new CapabilityProviderMirrorError({ code: "unavailable" });
				}
				const now = yield* metadata.Now;
				const state = yield* mirror
					.Sync(input)
					.pipe(
						Effect.catch((error) =>
							Effect.succeed(FailureState(input.engine_id, error, now)),
						),
					);
				yield* repository.CompleteProviderSync({
					capability_id: input.capability_id,
					operation_id: input.operation_id,
					state,
					status: detail.status,
				});
				return state;
			}).pipe(Effect.orDie);
		const ResolveDrift = (input: {
			readonly action: "ignore" | "import";
			readonly capability_id: string;
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly operation_id: string;
		}) =>
			Effect.gen(function* () {
				const detail = yield* repository.ReadDetail(input.capability_id);
				const operation_id = input.operation_id;
				yield* repository.RecordDriftResolution({ ...input, operation_id });
				{
					/** Import and ignore only reconcile canonical metadata; neither writes provider config. */
					const updated_at = yield* metadata.Now;
					const state: ProviderSyncState = {
						engine_id: input.engine_id,
						observed_revision: input.observed_revision,
						status: input.action === "ignore" ? "drift_ignored" : "runtime_only",
						updated_at,
					};
					yield* repository.CompleteDriftResolution({
						capability_id: input.capability_id,
						operation_id,
						state,
						status: detail.status,
					});
					return state;
				}
			}).pipe(Effect.orDie);
		const RequestOverwrite = (input: {
			readonly approval_fingerprint: string;
			readonly approval_id: string;
			readonly capability_id: string;
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly operation_id: string;
			readonly scope: MarketplaceScope;
		}) =>
			Effect.gen(function* () {
				const expected = Fingerprint({
					capability_id: input.capability_id,
					engine_id: input.engine_id,
					observed_revision: input.observed_revision,
					scope: input.scope,
				});
				if (expected !== input.approval_fingerprint)
					return yield* new CapabilityProviderMirrorError({ code: "unavailable" });
				yield* repository.RecordDriftResolution({ ...input, action: "overwrite" });
			}).pipe(Effect.asVoid, Effect.orDie);
		const DecideOverwrite = (input: {
			readonly approval_fingerprint: string;
			readonly approval_id: string;
			readonly approved: boolean;
			readonly capability_id: string;
			readonly engine_id: string;
			readonly observed_revision: string;
			readonly scope: MarketplaceScope;
		}) =>
			Effect.gen(function* () {
				const intent = yield* repository.ReadDriftApproval(input.approval_id);
				const expected = Fingerprint({
					capability_id: input.capability_id,
					engine_id: input.engine_id,
					observed_revision: input.observed_revision,
					scope: input.scope,
				});
				if (
					expected !== input.approval_fingerprint ||
					intent.capability_id !== input.capability_id ||
					intent.engine_id !== input.engine_id ||
					intent.observed_revision !== input.observed_revision ||
					!ScopeMatches(intent.scope, input.scope)
				)
					return yield* new CapabilityProviderMirrorError({ code: "unavailable" });
				const decision = yield* repository.DecideDriftOverwrite(input);
				if (!input.approved || decision === "denied")
					return yield* new CapabilityProviderMirrorError({ code: "unavailable" });
				const claim = yield* repository.ClaimDriftOverwrite(intent.operation_id);
				if (claim === "writing")
					return yield* new CapabilityProviderMirrorError({ code: "unavailable" });
				if (claim === "completed") {
					const detail = yield* repository.ReadDetail(intent.capability_id);
					const state = detail.sync.find((item) => item.engine_id === intent.engine_id);
					if (state) return state;
					return yield* new CapabilityProviderMirrorError({ code: "unavailable" });
				}
				const detail = yield* repository.ReadDetail(intent.capability_id);
				const now = yield* metadata.Now;
				const state = yield* mirror
					.ResolveDrift({ ...intent, action: "overwrite", approved: true })
					.pipe(
						Effect.catch((error) =>
							Effect.succeed(
								FailureState(
									intent.engine_id,
									error,
									now,
									intent.observed_revision,
								),
							),
						),
					);
				yield* repository.CompleteDriftResolution({
					capability_id: intent.capability_id,
					operation_id: intent.operation_id,
					state,
					status: detail.status,
				});
				return state;
			}).pipe(Effect.orDie);
		return {
			ResolveDrift: (input: {
				readonly action: "ignore" | "import";
				readonly capability_id: string;
				readonly engine_id: string;
				readonly observed_revision: string;
				readonly operation_id: string;
			}) => ResolveDrift(input),
			RequestOverwrite,
			DecideOverwrite,
			Sync: (input: {
				readonly capability_id: string;
				readonly engine_id: string;
				readonly operation_id: string;
			}) => Sync(input),
		};
	}).pipe(Effect.orDie),
);
