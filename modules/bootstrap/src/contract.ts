import { Context, Data, Effect, Schema } from "effect";

import {
	AbsolutePath,
	type InstallationManifest,
	type UnactivatedInstallationManifest,
} from "@artisan/distribution";

const BootstrapArgument = Schema.String.check(Schema.isMaxLength(4_096));

export const BootstrapInvocation = Schema.Struct({
	argv: Schema.Array(BootstrapArgument).check(Schema.isMaxLength(256)),
	bootstrap_pid: Schema.Int.check(
		Schema.isBetween({ minimum: 1, maximum: Number.MAX_SAFE_INTEGER }),
	),
	npm_executable: AbsolutePath,
	npm_prefix: AbsolutePath,
	package_name: Schema.Literal("artisan-editor"),
});
export type BootstrapInvocation = typeof BootstrapInvocation.Type;

export const BootstrapHandoff = Schema.Struct({
	permanent_ae_path: AbsolutePath,
});
export type BootstrapHandoff = typeof BootstrapHandoff.Type;

const PermanentAeOutput = Schema.String.check(Schema.isMaxLength(64 * 1_024));

export const PermanentAeCommandResult = Schema.Struct({
	exit_code: Schema.Int,
	stdout: PermanentAeOutput,
	stderr: PermanentAeOutput,
	stdout_truncated: Schema.Boolean,
	stderr_truncated: Schema.Boolean,
});
export type PermanentAeCommandResult = typeof PermanentAeCommandResult.Type;

export const ForgeRunningStatus = Schema.Struct({
	state: Schema.Literal("running"),
});
export type ForgeRunningStatus = typeof ForgeRunningStatus.Type;

export const NpmCleanupPlan = Schema.Struct({
	executable: AbsolutePath,
	argv: Schema.Tuple([
		Schema.Literal("uninstall"),
		Schema.Literal("--global"),
		Schema.Literal("--prefix"),
		AbsolutePath,
		Schema.Literal("artisan-editor"),
	]),
	bootstrap_pid: Schema.Int.check(
		Schema.isBetween({ minimum: 1, maximum: Number.MAX_SAFE_INTEGER }),
	),
	manual_command: Schema.NonEmptyString,
});
export type NpmCleanupPlan = typeof NpmCleanupPlan.Type;

export const BootstrapOutcome = Schema.Struct({
	route: Schema.Literals(["installed", "resumed", "delegated"]),
	permanent_ae_path: AbsolutePath,
	exit_code: Schema.Int,
	cleanup: Schema.Union([
		Schema.Struct({ state: Schema.Literal("scheduled") }),
		Schema.Struct({
			state: Schema.Literal("manual"),
			command: Schema.NonEmptyString,
		}),
	]),
});
export type BootstrapOutcome = typeof BootstrapOutcome.Type;

export class BootstrapInvocationInvalid extends Data.TaggedError("BootstrapInvocationInvalid")<{
	readonly cause: unknown;
}> {}

export class BootstrapInstallationMalformed extends Data.TaggedError(
	"BootstrapInstallationMalformed",
)<{
	readonly cause: unknown;
	readonly manifest_path: string;
}> {}

export class BootstrapContractInvalid extends Data.TaggedError("BootstrapContractInvalid")<{
	readonly boundary: "handoff" | "outcome";
	readonly cause: unknown;
}> {}

export class BootstrapInstallFailure extends Data.TaggedError("BootstrapInstallFailure")<{
	readonly cause: unknown;
	readonly operation: "install" | "resume";
}> {}

export class BootstrapFinalizationFailure extends Data.TaggedError("BootstrapFinalizationFailure")<{
	readonly cause: unknown;
}> {}

export class PermanentAeFailure extends Data.TaggedError("PermanentAeFailure")<{
	readonly cause: unknown;
	readonly operation: "delegate" | "setup" | "start" | "status" | "verify";
	readonly permanent_ae_path: string;
}> {}

export class PermanentAeCommandFailed extends Data.TaggedError("PermanentAeCommandFailed")<{
	readonly exit_code: number;
	readonly message: string;
	readonly operation: "delegate" | "setup" | "start" | "status";
	readonly permanent_ae_path: string;
	readonly stderr?: string;
}> {}

export class PermanentAeStatusInvalid extends Data.TaggedError("PermanentAeStatusInvalid")<{
	readonly cause: unknown;
	readonly permanent_ae_path: string;
	readonly stdout: string;
	readonly stdout_truncated: boolean;
}> {}

export class BootstrapCleanupFailure extends Data.TaggedError("BootstrapCleanupFailure")<{
	readonly cause: unknown;
	readonly plan: NpmCleanupPlan;
}> {}

export class BootstrapInstaller extends Context.Service<
	BootstrapInstaller,
	{
		readonly InstallFirstTime: (
			invocation: BootstrapInvocation,
		) => Effect.Effect<BootstrapHandoff, BootstrapInstallFailure>;
		readonly Resume: (
			manifest: UnactivatedInstallationManifest | InstallationManifest,
			invocation: BootstrapInvocation,
		) => Effect.Effect<BootstrapHandoff, BootstrapInstallFailure>;
	}
>()("Artisan/Bootstrap/BootstrapInstaller") {}

export class PermanentAe extends Context.Service<
	PermanentAe,
	{
		readonly Delegate: (
			permanent_ae_path: string,
			argv: ReadonlyArray<string>,
		) => Effect.Effect<number, PermanentAeFailure>;
		readonly Execute: (
			permanent_ae_path: string,
			operation: "setup" | "start" | "status",
			argv: ReadonlyArray<string>,
		) => Effect.Effect<PermanentAeCommandResult, PermanentAeFailure>;
		readonly VerifyHandoff: (
			permanent_ae_path: string,
		) => Effect.Effect<void, PermanentAeFailure>;
	}
>()("Artisan/Bootstrap/PermanentAe") {}

export class BootstrapCleanup extends Context.Service<
	BootstrapCleanup,
	{
		readonly ScheduleDetached: (
			plan: NpmCleanupPlan,
		) => Effect.Effect<void, BootstrapCleanupFailure>;
	}
>()("Artisan/Bootstrap/BootstrapCleanup") {}
