import { Clock, Effect, Match, Schema } from "effect";

import { InstallationStore, type InstallationState } from "@artisan/distribution";

import { MakeNpmCleanupPlan } from "./cleanup";
import {
	BootstrapCleanup,
	BootstrapContractInvalid,
	BootstrapFinalizationFailure,
	ForgeRunningStatus,
	BootstrapHandoff,
	BootstrapInstallationMalformed,
	BootstrapInstaller,
	BootstrapInvocation,
	BootstrapInvocationInvalid,
	BootstrapOutcome,
	PermanentAe,
	PermanentAeCommandFailed,
	PermanentAeStatusInvalid,
	type BootstrapHandoff as BootstrapHandoffValue,
} from "./contract";

type BootstrapRoute = BootstrapOutcome["route"];

const ResolveHandoff = (
	state: InstallationState,
	invocation: BootstrapInvocation,
	installer: BootstrapInstaller["Service"],
) =>
	Match.value(state).pipe(
		Match.tagsExhaustive({
			Absent: () =>
				installer
					.InstallFirstTime(invocation)
					.pipe(Effect.map((handoff) => ["installed", handoff] as const)),
			Healthy: ({ manifest }) =>
				Effect.succeed([
					"delegated",
					{
						permanent_ae_path: manifest.permanent_ae_path,
					},
				] as const),
			Malformed: ({ cause, manifest_path }) =>
				Effect.fail(
					new BootstrapInstallationMalformed({
						cause,
						manifest_path,
					}),
				),
			Partial: ({ manifest }) =>
				installer
					.Resume(manifest, invocation)
					.pipe(Effect.map((handoff) => ["resumed", handoff] as const)),
		}),
	);

/**
 * Runs the disposable bootstrap without relying on PATH resolution or shell
 * command strings. Cleanup is attempted only after the permanent CLI passes
 * its health boundary and is deliberately nonfatal.
 */
export const RunBootstrap = (input: unknown) =>
	Effect.gen(function* () {
		const invocation = yield* Schema.decodeUnknownEffect(BootstrapInvocation)(input).pipe(
			Effect.mapError((cause) => new BootstrapInvocationInvalid({ cause })),
		);
		const installation_store = yield* InstallationStore;
		const installer = yield* BootstrapInstaller;
		const permanent_ae = yield* PermanentAe;
		const cleanup = yield* BootstrapCleanup;

		const state = yield* installation_store.Inspect();
		const [route, raw_handoff]: readonly [BootstrapRoute, BootstrapHandoffValue] =
			yield* ResolveHandoff(state, invocation, installer);
		const handoff = yield* Schema.decodeUnknownEffect(BootstrapHandoff)(raw_handoff).pipe(
			Effect.mapError(
				(cause) =>
					new BootstrapContractInvalid({
						boundary: "handoff",
						cause,
					}),
			),
		);

		yield* permanent_ae.VerifyHandoff(handoff.permanent_ae_path);

		const RequireSuccess = (
			operation: PermanentAeCommandFailed["operation"],
			result: {
				readonly exit_code: number;
				readonly stderr?: string;
				readonly stdout?: string;
			},
		) =>
			result.exit_code === 0
				? Effect.void
				: Effect.fail(
						new PermanentAeCommandFailed({
							exit_code: result.exit_code,
							message: [
								`Permanent ae ${operation} exited with code ${result.exit_code}`,
								result.stderr?.trim() || result.stdout?.trim(),
							]
								.filter((value) => value !== undefined && value.length > 0)
								.join(": "),
							operation,
							permanent_ae_path: handoff.permanent_ae_path,
							...(result.stderr === undefined ? {} : { stderr: result.stderr }),
						}),
					);

		if (route !== "delegated") {
			for (const [operation, argv] of [
				["setup", ["setup"]],
				["start", ["start"]],
			] as const) {
				const result = yield* permanent_ae.Execute(
					handoff.permanent_ae_path,
					operation,
					argv,
				);
				yield* RequireSuccess(operation, result);
			}

			const status = yield* permanent_ae.Execute(handoff.permanent_ae_path, "status", [
				"status",
				"--json",
			]);
			yield* RequireSuccess("status", status);
			if (status.stdout_truncated)
				return yield* new PermanentAeStatusInvalid({
					cause: new Error("Forge status output exceeded the bootstrap limit"),
					permanent_ae_path: handoff.permanent_ae_path,
					stdout: status.stdout,
					stdout_truncated: true,
				});
			const raw_status = yield* Schema.decodeUnknownEffect(Schema.UnknownFromJsonString)(
				status.stdout,
			).pipe(
				Effect.mapError(
					(cause) =>
						new PermanentAeStatusInvalid({
							cause,
							permanent_ae_path: handoff.permanent_ae_path,
							stdout: status.stdout,
							stdout_truncated: false,
						}),
				),
			);
			yield* Schema.decodeUnknownEffect(ForgeRunningStatus)(raw_status).pipe(
				Effect.mapError(
					(cause) =>
						new PermanentAeStatusInvalid({
							cause,
							permanent_ae_path: handoff.permanent_ae_path,
							stdout: status.stdout,
							stdout_truncated: false,
						}),
				),
			);

			const finalized_state = yield* installation_store.Inspect();
			if (
				finalized_state._tag !== "Partial" ||
				finalized_state.manifest.activation_state !== "active" ||
				finalized_state.manifest.transaction.state !== "idle" ||
				finalized_state.manifest.permanent_ae_path !== handoff.permanent_ae_path
			)
				return yield* new BootstrapFinalizationFailure({
					cause: new Error(
						"Installed manifest is not an idle active installation at the verified handoff",
					),
				});
			const updated_at = new Date(yield* Clock.currentTimeMillis).toISOString();
			yield* installation_store
				.WriteAtomic({
					...finalized_state.manifest,
					finalization_state: "complete",
					updated_at,
				})
				.pipe(
					Effect.mapError(
						(cause) =>
							new BootstrapFinalizationFailure({
								cause,
							}),
					),
				);
		}

		const exit_code = yield* permanent_ae.Delegate(handoff.permanent_ae_path, invocation.argv);
		yield* RequireSuccess("delegate", { exit_code });

		const cleanup_plan = MakeNpmCleanupPlan(invocation);
		const cleanup_outcome = yield* cleanup.ScheduleDetached(cleanup_plan).pipe(
			Effect.as({ state: "scheduled" } as const),
			Effect.catch(() =>
				Effect.succeed({
					state: "manual",
					command: cleanup_plan.manual_command,
				} as const),
			),
		);

		return yield* Schema.decodeUnknownEffect(BootstrapOutcome)({
			route,
			permanent_ae_path: handoff.permanent_ae_path,
			exit_code,
			cleanup: cleanup_outcome,
		}).pipe(
			Effect.mapError(
				(cause) =>
					new BootstrapContractInvalid({
						boundary: "outcome",
						cause,
					}),
			),
		);
	});
