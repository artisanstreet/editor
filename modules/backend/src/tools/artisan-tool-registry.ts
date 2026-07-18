import { Context, Data, Effect, Layer, Schema } from "effect";

import {
	ArtisanToolAvailability,
	ArtisanToolDeclaration,
	ArtisanToolDescriptor,
	ArtisanToolId,
	ArtisanToolPermissionPolicy,
	type ArtisanToolAvailability as ArtisanToolAvailabilityValue,
	type ArtisanToolDeclaration as ArtisanToolDeclarationValue,
	type ArtisanToolId as ArtisanToolIdValue,
	type ArtisanToolPermissionPolicy as ArtisanToolPermissionPolicyValue,
} from "@artisan/protocol";

import { ArtisanToolApprovalPolicy } from "./approval-policy";

const CapabilityState = Schema.Struct({
	state: Schema.Literals(["available", "unavailable"]),
	tool_id: ArtisanToolId,
	unavailable_reason: Schema.optional(Schema.NonEmptyString),
}).check(
	Schema.makeFilter<typeof CapabilityState.Type>((state) =>
		state.state === "unavailable" && state.unavailable_reason === undefined
			? "Expected unavailable capabilities to explain why they are unavailable"
			: state.state === "available" && state.unavailable_reason !== undefined
				? "Expected available capabilities not to have an unavailable reason"
				: undefined,
	),
);

const RegistryRegistration = Schema.Struct({
	declaration: ArtisanToolDeclaration,
	tool_id: ArtisanToolId,
});

export type ArtisanToolCapabilityStateValue = typeof CapabilityState.Type;

export interface ArtisanToolRegistryRegistration {
	readonly declaration: ArtisanToolDeclarationValue;
	readonly tool_id: ArtisanToolIdValue;
}

/** Reports malformed, duplicate, or inconsistent static control-plane registration. */
export class ArtisanToolRegistryFailure extends Data.TaggedError("ArtisanToolRegistryFailure")<{
	readonly cause?: unknown;
	readonly reason: "duplicate_id" | "invalid" | "mismatched_id";
	readonly tool_id?: string;
}> {}

/** Reports an exact built-in tool identity that is outside this Artisan-owned registry. */
export class ArtisanToolNotFound extends Data.TaggedError("ArtisanToolNotFound")<{
	readonly tool_id: string;
}> {}

/** Supplies runtime capability facts without coupling the static catalog to implementation services. */
export class ArtisanToolCapabilityState extends Context.Service<
	ArtisanToolCapabilityState,
	{
		readonly Get: (
			tool_id: ArtisanToolIdValue,
			workspace_id?: string,
		) => Effect.Effect<ArtisanToolCapabilityStateValue>;
	}
>()("Artisan/ArtisanToolCapabilityState") {}

/** Owns the static, policy-aware Artisan built-in tool catalog. */
export class ArtisanToolRegistry extends Context.Service<
	ArtisanToolRegistry,
	{
		readonly Declarations: ReadonlyArray<ArtisanToolDeclarationValue>;
		readonly Availability: (input: {
			readonly policy: ArtisanToolPermissionPolicyValue;
			readonly workspace_id?: string;
		}) => Effect.Effect<
			ReadonlyArray<ArtisanToolAvailabilityValue>,
			ArtisanToolRegistryFailure
		>;
		readonly Find: (
			tool_id: ArtisanToolIdValue,
		) => Effect.Effect<ArtisanToolDeclarationValue, ArtisanToolNotFound>;
	}
>()("Artisan/ArtisanToolRegistry") {}

const descriptor = (
	id: ArtisanToolIdValue,
	kind: (typeof ArtisanToolDescriptor.Type)["kind"],
	title: string,
	description: string,
	permission_requirements: (typeof ArtisanToolDescriptor.Type)["permission_requirements"],
	approval_behavior: (typeof ArtisanToolDescriptor.Type)["approval_behavior"],
): ArtisanToolRegistryRegistration => ({
	declaration: {
		descriptor: {
			approval_behavior,
			description,
			id,
			kind,
			permission_requirements,
			schema_version: 1,
			title,
		},
		input_schema_version: 1,
		output_schema_version: 1,
	},
	tool_id: id,
});

/** The full engine-facing V1 catalog; engine.native_action only records observation. */
export const ArtisanBuiltInToolRegistrations: ReadonlyArray<ArtisanToolRegistryRegistration> = [
	descriptor(
		"approval.request",
		"approval",
		"Request approval",
		"Request a durable user decision before sensitive work.",
		["user_interaction"],
		"never",
	),
	descriptor(
		"assumption.record",
		"assumption",
		"Record assumption",
		"Record a safe, reviewable assumption.",
		["none"],
		"never",
	),
	descriptor(
		"engine.native_action.record",
		"native_action",
		"Record native action",
		"Observe an engine-owned action; this never dispatches an engine action.",
		["engine_observation"],
		"never",
	),
	descriptor(
		"git.diff.read",
		"git",
		"Read Git diff",
		"Read the current workspace Git diff.",
		["git_read"],
		"never",
	),
	descriptor(
		"git.index.stage",
		"git",
		"Stage Git paths",
		"Stage selected paths in the Git index.",
		["git_index_write"],
		"on_request",
	),
	descriptor(
		"git.index.unstage",
		"git",
		"Unstage Git paths",
		"Unstage selected paths in the Git index.",
		["git_index_write"],
		"on_request",
	),
	descriptor(
		"git.status.read",
		"git",
		"Read Git status",
		"Read current workspace Git status.",
		["git_read"],
		"never",
	),
	descriptor(
		"preview.inspect",
		"preview",
		"Inspect preview",
		"Inspect a controlled preview target.",
		["preview_control"],
		"never",
	),
	descriptor(
		"preview.open",
		"preview",
		"Open preview",
		"Open a controlled local preview target.",
		["preview_control"],
		"on_request",
	),
	descriptor(
		"preview.stop",
		"preview",
		"Stop preview",
		"Stop a controlled preview target.",
		["preview_control"],
		"on_request",
	),
	descriptor(
		"question.ask",
		"question",
		"Ask question",
		"Ask the user a structured clarification question.",
		["user_interaction"],
		"never",
	),
	descriptor(
		"terminal.open",
		"terminal",
		"Open terminal",
		"Open a controlled workspace terminal.",
		["process_control"],
		"on_request",
	),
	descriptor(
		"terminal.read",
		"terminal",
		"Read terminal",
		"Read output from an owned terminal.",
		["none"],
		"never",
	),
	descriptor(
		"terminal.restart",
		"terminal",
		"Restart terminal",
		"Restart an owned terminal.",
		["process_control"],
		"on_request",
	),
	descriptor(
		"terminal.stop",
		"terminal",
		"Stop terminal",
		"Stop an owned terminal.",
		["process_control"],
		"on_request",
	),
	descriptor(
		"terminal.write",
		"terminal",
		"Write terminal",
		"Write bounded input to an owned terminal.",
		["process_control"],
		"on_request",
	),
	descriptor(
		"workspace.file.read",
		"workspace_file",
		"Read workspace file",
		"Read a controlled workspace file.",
		["workspace_read"],
		"never",
	),
	descriptor(
		"workspace.file.list",
		"workspace_file",
		"Discover workspace files",
		"Discover controlled workspace files without reading arbitrary host paths.",
		["workspace_read"],
		"never",
	),
	descriptor(
		"workspace.file.write",
		"workspace_file",
		"Write workspace file",
		"Create a reviewable controlled workspace file change.",
		["workspace_write"],
		"on_request",
	),
	descriptor(
		"workspace.language.status",
		"workspace_file",
		"Inspect language capabilities",
		"Report configured language and diagnostics capabilities without inventing a provider.",
		["workspace_read"],
		"never",
	),
].toSorted((left, right) => left.tool_id.localeCompare(right.tool_id));

function RegistryFailure(
	reason: ArtisanToolRegistryFailure["reason"],
	cause?: unknown,
	tool_id?: string,
) {
	return new ArtisanToolRegistryFailure({
		...(cause === undefined ? {} : { cause }),
		...(tool_id === undefined ? {} : { tool_id }),
		reason,
	});
}

/** Builds a fail-closed capability-state service from explicitly supplied runtime facts. */
export function make_artisan_tool_capability_state_layer(states: ReadonlyArray<unknown>) {
	return Layer.effect(
		ArtisanToolCapabilityState,
		Effect.gen(function* () {
			const decoded = yield* Effect.forEach(states, (state) =>
				Schema.decodeUnknownEffect(CapabilityState, { onExcessProperty: "error" })(
					state,
				).pipe(Effect.mapError((cause) => RegistryFailure("invalid", cause))),
			);
			const ids = decoded.map((state) => state.tool_id);

			if (new Set(ids).size !== ids.length) {
				return yield* Effect.fail(RegistryFailure("duplicate_id"));
			}

			const by_tool_id = new Map(decoded.map((state) => [state.tool_id, state] as const));
			const Get = (tool_id: ArtisanToolIdValue, _workspace_id?: string) =>
				Effect.succeed(
					by_tool_id.get(tool_id) ?? {
						state: "unavailable" as const,
						tool_id,
						unavailable_reason: "Capability is not configured",
					},
				);

			return { Get };
		}),
	);
}

/** Builds the validated static catalog and resolves policy plus runtime availability deterministically. */
export function make_artisan_tool_registry_layer(
	registrations: ReadonlyArray<unknown> = ArtisanBuiltInToolRegistrations,
) {
	return Layer.effect(
		ArtisanToolRegistry,
		Effect.gen(function* () {
			const approval_policy = yield* ArtisanToolApprovalPolicy;
			const capability_state = yield* ArtisanToolCapabilityState;
			const decoded = yield* Effect.forEach(registrations, (registration) =>
				Schema.decodeUnknownEffect(RegistryRegistration, { onExcessProperty: "error" })(
					registration,
				).pipe(Effect.mapError((cause) => RegistryFailure("invalid", cause))),
			);
			const mismatched = decoded.find(
				(registration) => registration.declaration.descriptor.id !== registration.tool_id,
			);

			if (mismatched !== undefined) {
				return yield* Effect.fail(
					RegistryFailure("mismatched_id", undefined, mismatched.tool_id),
				);
			}
			const ids = decoded.map((registration) => registration.tool_id);

			if (new Set(ids).size !== ids.length) {
				return yield* Effect.fail(RegistryFailure("duplicate_id"));
			}

			const Declarations = decoded
				.map((registration) => registration.declaration)
				.toSorted((left, right) => left.descriptor.id.localeCompare(right.descriptor.id));
			const by_tool_id = new Map(
				Declarations.map(
					(declaration) => [declaration.descriptor.id, declaration] as const,
				),
			);
			const Find = (tool_id: ArtisanToolIdValue) => {
				const declaration = by_tool_id.get(tool_id);

				return declaration === undefined
					? Effect.fail(new ArtisanToolNotFound({ tool_id }))
					: Effect.succeed(declaration);
			};
			const Availability = (input: {
				readonly policy: ArtisanToolPermissionPolicyValue;
				readonly workspace_id?: string;
			}) =>
				Schema.decodeUnknownEffect(ArtisanToolPermissionPolicy, {
					onExcessProperty: "error",
				})(input.policy).pipe(
					Effect.mapError((cause) => RegistryFailure("invalid", cause)),
					Effect.flatMap((policy) =>
						Effect.forEach(Declarations, (declaration) =>
							Effect.all([
								approval_policy.Decide(declaration.descriptor, policy),
								capability_state.Get(declaration.descriptor.id, input.workspace_id),
							]).pipe(
								Effect.map(([decision, capability]) => {
									if (capability.state === "unavailable") {
										return {
											state: "unavailable" as const,
											tool_id: declaration.descriptor.id,
											unavailable_reason: capability.unavailable_reason,
										} satisfies ArtisanToolAvailabilityValue;
									}
									if (decision.decision === "denied") {
										return {
											state: "unavailable" as const,
											tool_id: declaration.descriptor.id,
											unavailable_reason:
												decision.reason ??
												"Session policy denies this tool",
										} satisfies ArtisanToolAvailabilityValue;
									}
									return decision.decision === "approval_required"
										? ({
												state: "approval_required" as const,
												tool_id: declaration.descriptor.id,
											} satisfies ArtisanToolAvailabilityValue)
										: ({
												state: "available" as const,
												tool_id: declaration.descriptor.id,
											} satisfies ArtisanToolAvailabilityValue);
								}),
								Effect.mapError((cause) => RegistryFailure("invalid", cause)),
							),
						),
					),
				);

			return { Availability, Declarations, Find };
		}),
	);
}
