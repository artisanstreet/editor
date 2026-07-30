import { Context, Effect, Layer, Option, Schema } from "effect";

import {
	PreviewRepository,
	PreviewRepositoryError,
	PreviewTargetProjection,
	type PreviewInspectionCommand,
	type PreviewInspectionProjection,
	type PreviewDispatchLease,
	type PreviewDispatchLeaseInput,
	type PreviewRegisterCommand,
	type PreviewTargetUpdateCommand,
	ValidateLocalPreviewUrl,
} from "./repository";

const Identifier = Schema.String.pipe(Schema.check(Schema.isMinLength(1), Schema.isMaxLength(256)));

export class PreviewService extends Context.Service<
	PreviewService,
	{
		readonly Get: (
			target_id: string,
		) => Effect.Effect<PreviewTargetProjection, PreviewRepositoryError>;
		readonly List: (
			workspace_id?: string,
		) => Effect.Effect<ReadonlyArray<PreviewTargetProjection>, PreviewRepositoryError>;
		readonly ValidateTargetUrl: (url: string) => Effect.Effect<string, PreviewRepositoryError>;
		readonly Register: (
			input: PreviewRegisterCommand,
		) => Effect.Effect<PreviewTargetProjection, PreviewRepositoryError>;
		readonly ReplayTargetUpdate: (
			input: PreviewTargetUpdateCommand,
		) => Effect.Effect<Option.Option<PreviewTargetProjection>, PreviewRepositoryError>;
		readonly UpdateTarget: (
			input: PreviewTargetUpdateCommand,
			dispatch_lease_id?: string,
		) => Effect.Effect<PreviewTargetProjection, PreviewRepositoryError>;
		readonly UpdateInspection: (
			input: PreviewInspectionCommand,
			dispatch_lease_id?: string,
		) => Effect.Effect<PreviewInspectionProjection, PreviewRepositoryError>;
		readonly RecoverInspections: () => Effect.Effect<
			ReadonlyArray<PreviewInspectionProjection>,
			PreviewRepositoryError
		>;
		readonly AcquireDispatchLease: (
			input: PreviewDispatchLeaseInput,
		) => Effect.Effect<PreviewDispatchLease, PreviewRepositoryError>;
		readonly ReleaseDispatchLease: (lease: PreviewDispatchLease) => Effect.Effect<void>;
		readonly RenewDispatchLease: (
			lease: PreviewDispatchLease,
		) => Effect.Effect<PreviewDispatchLease, PreviewRepositoryError>;
		readonly RecoverDispatchLeases: () => Effect.Effect<
			ReadonlyArray<PreviewDispatchLease>,
			PreviewRepositoryError
		>;
	}
>()("Artisan/PreviewService") {}

/** Composition boundary for durable preview reads; side-effect commands remain explicitly owned by the runtime coordinator. */
export const PreviewServiceLive = Layer.effect(
	PreviewService,
	Effect.gen(function* () {
		const repository = yield* PreviewRepository;
		const Get = (target_id: string) =>
			Schema.decodeUnknownEffect(Identifier)(target_id).pipe(
				Effect.mapError(
					() =>
						new PreviewRepositoryError({
							code: "invalid",
							message: "Preview target ID is invalid",
						}),
				),
				Effect.flatMap(repository.GetTarget),
			);
		return {
			Get,
			AcquireDispatchLease: repository.AcquireDispatchLease,
			List: repository.ListTargets,
			RecoverInspections: repository.RecoverInspections,
			RecoverDispatchLeases: repository.RecoverDispatchLeases,
			ReleaseDispatchLease: repository.ReleaseDispatchLease,
			RenewDispatchLease: repository.RenewDispatchLease,
			Register: repository.Register,
			ReplayTargetUpdate: repository.ReplayTargetUpdate,
			UpdateInspection: repository.UpdateInspection,
			UpdateTarget: repository.UpdateTarget,
			ValidateTargetUrl: ValidateLocalPreviewUrl,
		};
	}),
);
