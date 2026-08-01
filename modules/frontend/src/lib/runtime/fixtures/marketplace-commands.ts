import { Effect } from "effect";
import { ArtisanClient } from "@artisan/transport/client";

import { FixtureFailure, FixtureReceipt } from "./support";

/** Deterministic marketplace failures and receipts for the browser fixture. */
export const FixtureMarketplaceCommands = {
	PreviewRoutineInstall: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace routine fixtures are unavailable.");
		}),
	RequestRoutineInstall: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-routine-install");
		}),
	DecideRoutineInstall: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-routine-decision");
		}),
	EnableRoutine: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-routine-enable");
		}),
	DisableRoutine: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-routine-disable");
		}),
	RemoveRoutine: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-routine-remove");
		}),
	RollbackRoutine: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-routine-rollback");
		}),
	SyncRoutine: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-routine-sync");
		}),
	ResolveRoutineDrift: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-routine-drift");
		}),
	RequestRoutineDriftOverwrite: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(
				input.command_id ?? "fixture-routine-drift-overwrite-request",
			);
		}),
	DecideRoutineDriftOverwrite: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(
				input.command_id ?? "fixture-routine-drift-overwrite-decision",
			);
		}),
	InvokeRoutine: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace routine fixtures are unavailable.");
		}),
	DiscoverNpxSkills: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace npx-skills fixtures are unavailable.");
		}),
	ImportNpxSkills: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-npx-skills-import");
		}),
	PreviewCapabilityConnect: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace capability fixtures are unavailable.");
		}),
	RequestCapabilityConnect: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-connect");
		}),
	DecideCapabilityConnect: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-decision");
		}),
	StartCapability: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-start");
		}),
	ReconnectCapability: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-reconnect");
		}),
	CheckCapabilityHealth: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-health");
		}),
	DisconnectCapability: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-disconnect");
		}),
	RestartCapability: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-restart");
		}),
	UninstallCapability: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-uninstall");
		}),
	EnableCapability: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-enable");
		}),
	DisableCapability: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-disable");
		}),
	RemoveCapability: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-remove");
		}),
	SyncCapability: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-sync");
		}),
	ResolveCapabilityDrift: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-drift");
		}),
	RequestCapabilityDriftOverwrite: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(
				input.command_id ?? "fixture-capability-drift-overwrite-request",
			);
		}),
	DecideCapabilityDriftOverwrite: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(
				input.command_id ?? "fixture-capability-drift-overwrite-decision",
			);
		}),
	RequestCapabilityInvocation: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace capability fixtures are unavailable.");
		}),
	DecideCapabilityInvocation: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace capability fixtures are unavailable.");
		}),
	InvokeCapability: () =>
		Effect.gen(function* () {
			return yield* FixtureFailure("Marketplace capability fixtures are unavailable.");
		}),
	BeginCapabilityOAuth: () =>
		Effect.gen(function* () {
			return {
				authorization_url: "https://fixture.invalid/oauth/authorize",
				continuation_reference: "fixture-oauth-continuation",
			};
		}),
	CompleteCapabilityOAuth: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-oauth-complete");
		}),
	RefreshCapabilityOAuth: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-oauth-refresh");
		}),
	RevokeCapabilityOAuth: (input) =>
		Effect.gen(function* () {
			return yield* FixtureReceipt(input.command_id ?? "fixture-capability-oauth-revoke");
		}),
} satisfies Pick<
	typeof ArtisanClient.Service,
	| "PreviewRoutineInstall"
	| "RequestRoutineInstall"
	| "DecideRoutineInstall"
	| "EnableRoutine"
	| "DisableRoutine"
	| "RemoveRoutine"
	| "RollbackRoutine"
	| "SyncRoutine"
	| "ResolveRoutineDrift"
	| "RequestRoutineDriftOverwrite"
	| "DecideRoutineDriftOverwrite"
	| "InvokeRoutine"
	| "DiscoverNpxSkills"
	| "ImportNpxSkills"
	| "PreviewCapabilityConnect"
	| "RequestCapabilityConnect"
	| "DecideCapabilityConnect"
	| "StartCapability"
	| "ReconnectCapability"
	| "CheckCapabilityHealth"
	| "DisconnectCapability"
	| "RestartCapability"
	| "UninstallCapability"
	| "EnableCapability"
	| "DisableCapability"
	| "RemoveCapability"
	| "SyncCapability"
	| "ResolveCapabilityDrift"
	| "RequestCapabilityDriftOverwrite"
	| "DecideCapabilityDriftOverwrite"
	| "RequestCapabilityInvocation"
	| "DecideCapabilityInvocation"
	| "InvokeCapability"
	| "BeginCapabilityOAuth"
	| "CompleteCapabilityOAuth"
	| "RefreshCapabilityOAuth"
	| "RevokeCapabilityOAuth"
>;
