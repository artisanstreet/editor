import { Effect, Schema } from "effect";
import { Tool } from "effect/unstable/ai";
import { describe, expect, it } from "vitest";

import { ToolInputSchema, WorkspaceGitSessionQueryResult } from "@artisan/protocol";

import { make_effect_tool_adapter } from "../../modules/backend/src/tool-control/internal/effect-tool-adapter";
import {
	ToolRegistry,
	ToolRegistryError,
	ToolIneligible,
	type ToolRegistration,
	make_tool_registry_layer,
} from "../../modules/backend/src/tool-control/tool-registry";

const context = { agent_id: "agent", run_id: "run", thread_id: "thread" };

function registration(input: {
	readonly approval_policy?: "automatic" | "required";
	readonly effect?: ToolRegistration["descriptor"]["effect"];
	readonly handler?: () => Effect.Effect<unknown, unknown>;
	readonly IsEligible?: ToolRegistration["IsEligible"];
	readonly recovery_policy?: ToolRegistration["recovery_policy"];
	readonly revision?: number;
	readonly tool_id: string;
}): ToolRegistration {
	const adapter = make_effect_tool_adapter({
		handler: () => input.handler?.() ?? Effect.succeed({ journal_sequence: 1 }),
		parameters: Tool.EmptyParams,
		success: WorkspaceGitSessionQueryResult,
	});
	const descriptor = {
		approval_policy: input.approval_policy ?? "automatic",
		effect: input.effect ?? "read",
		input_schema: Schema.decodeUnknownSync(ToolInputSchema)(adapter.input_schema),
		label: `Label ${input.tool_id}`,
		revision: input.revision ?? 1,
		source: "artisan",
		summary: `Summary ${input.tool_id}`,
		tool_id: input.tool_id,
	} satisfies ToolRegistration["descriptor"];

	return {
		adapter,
		descriptor,
		IsEligible: input.IsEligible ?? (() => Effect.void),
		recovery_policy: input.recovery_policy ?? "retry",
	};
}

function registry(registrations: ReadonlyArray<unknown>) {
	return Effect.service(ToolRegistry).pipe(
		Effect.provide(make_tool_registry_layer(registrations)),
	);
}

describe("ToolRegistry", () => {
	it("rejects duplicate IDs and descriptors that disagree with their Effect Tool schema", async () => {
		const duplicate = await Effect.runPromise(
			registry([
				registration({ tool_id: "duplicate" }),
				registration({ tool_id: "duplicate" }),
			]).pipe(Effect.flip),
		);
		const original = registration({ tool_id: "mismatch" });
		const mismatch = {
			...original,
			descriptor: { ...original.descriptor, input_schema: { type: "object" } },
		};
		const invalid = await Effect.runPromise(registry([mismatch]).pipe(Effect.flip));
		const missing_invoke = {
			...original,
			adapter: { input_schema: original.adapter.input_schema },
		};
		const missing_eligibility = {
			adapter: original.adapter,
			descriptor: original.descriptor,
			recovery_policy: original.recovery_policy,
		};
		const missing_recovery = {
			adapter: original.adapter,
			descriptor: original.descriptor,
			IsEligible: original.IsEligible,
		};
		const invalid_adapter = await Effect.runPromise(
			registry([missing_invoke]).pipe(Effect.flip),
		);
		const invalid_eligibility = await Effect.runPromise(
			registry([missing_eligibility]).pipe(Effect.flip),
		);
		const invalid_recovery = await Effect.runPromise(
			registry([missing_recovery]).pipe(Effect.flip),
		);

		expect(duplicate).toBeInstanceOf(ToolRegistryError);
		expect(duplicate.reason_code).toBe("duplicate_registration");
		expect(invalid).toBeInstanceOf(ToolRegistryError);
		expect(invalid.reason_code).toBe("invalid_registration");
		expect(invalid_adapter.reason_code).toBe("invalid_registration");
		expect(invalid_eligibility.reason_code).toBe("invalid_registration");
		expect(invalid_recovery.reason_code).toBe("invalid_registration");
	});

	it("lists deterministic eligibility, preserves approval metadata, and resolves exact revisions", async () => {
		const alpha = registration({ tool_id: "alpha" });
		const service = await Effect.runPromise(
			registry([registration({ approval_policy: "required", tool_id: "zeta" }), alpha]),
		);
		const listed = await Effect.runPromise(service.List(context));
		const resolved = await Effect.runPromise(
			service.Resolve({ revision: 1, tool_id: "alpha" }),
		);
		const recovery_policy = await Effect.runPromise(
			service.RecoveryPolicy({ revision: 1, tool_id: "alpha" }),
		);
		const stale = await Effect.runPromise(
			service.Resolve({ revision: 2, tool_id: "alpha" }).pipe(Effect.flip),
		);

		expect(listed.tools.map(({ descriptor }) => descriptor.tool_id)).toEqual(["alpha", "zeta"]);
		expect(listed.tools[1]?.descriptor.approval_policy).toBe("required");
		expect(resolved.tool_id).toBe("alpha");
		expect(recovery_policy).toBe("retry");
		expect(stale.reason_code).toBe("revision_mismatch");

		Reflect.set(resolved, "revision", 99);
		Reflect.set(Object(resolved.input_schema), "type", "string");
		Reflect.set(alpha.adapter, "Invoke", () => Effect.succeed("mutated"));

		const resolved_again = await Effect.runPromise(
			service.Resolve({ revision: 1, tool_id: "alpha" }),
		);
		const invoked = await Effect.runPromise(
			service.Invoke({ revision: 1, tool_id: "alpha" }, context, {}),
		);

		expect(resolved_again.revision).toBe(1);
		expect(resolved_again.input_schema).toMatchObject({ type: "object" });
		expect(invoked).toEqual({ journal_sequence: 1 });
	});

	it("authorizes only eligible exact revisions and certifies retries only for reads", async () => {
		const ineligible = registration({
			IsEligible: () =>
				Effect.fail(new ToolIneligible({ reason_code: "workspace.unavailable" })),
			tool_id: "ineligible",
		});
		const invalid_retry = registration({
			effect: "workspace_mutation",
			recovery_policy: "retry",
			tool_id: "invalid.retry",
		});
		const mutation = registration({
			effect: "workspace_mutation",
			recovery_policy: "outcome_unknown",
			tool_id: "mutation",
		});
		const invalid = await Effect.runPromise(registry([invalid_retry]).pipe(Effect.flip));
		const service = await Effect.runPromise(registry([ineligible, mutation]));
		const denied = await Effect.runPromise(
			service.Authorize({ revision: 1, tool_id: "ineligible" }, context).pipe(Effect.flip),
		);
		const authorized = await Effect.runPromise(
			service.Authorize({ revision: 1, tool_id: "mutation" }, context),
		);

		expect(invalid.reason_code).toBe("invalid_registration");
		expect(denied.reason_code).toBe("context_ineligible");
		expect(authorized).toMatchObject({
			descriptor: { effect: "workspace_mutation", tool_id: "mutation" },
			recovery_policy: "outcome_unknown",
		});
	});

	it("compares input schemas structurally and encodes transformed success values", async () => {
		const adapter = make_effect_tool_adapter({
			handler: () => Effect.succeed(42),
			parameters: Tool.EmptyParams,
			success: Schema.NumberFromString,
		});
		const reordered_input_schema = Schema.decodeUnknownSync(ToolInputSchema)(
			Object.fromEntries(Object.entries(Object(adapter.input_schema)).reverse()),
		);
		const transformed = {
			adapter,
			descriptor: {
				approval_policy: "automatic",
				effect: "read",
				input_schema: reordered_input_schema,
				label: "Transformed result",
				revision: 1,
				source: "artisan",
				summary: "Encodes a transformed success value.",
				tool_id: "transformed.result",
			},
			IsEligible: () => Effect.void,
			recovery_policy: "retry" as const,
		};
		const service = await Effect.runPromise(registry([transformed]));
		const result = await Effect.runPromise(
			service.Invoke({ revision: 1, tool_id: "transformed.result" }, context, {}),
		);

		expect(result).toBe("42");
	});

	it("rejects hostile arguments before the handler and never reflects hostile data into metadata or errors", async () => {
		let calls = 0;
		const hostile = "C:\\private\\workspace\\provider diagnostic";
		const valid = registration({
			handler: () => {
				calls += 1;

				return Effect.succeed({ journal_sequence: 1 });
			},
			tool_id: "hostile.arguments",
		});
		const invalid_result = registration({
			handler: () => Effect.succeed({ diagnostic: hostile }),
			tool_id: "hostile.result",
		});
		const service = await Effect.runPromise(registry([valid, invalid_result]));
		const before = await Effect.runPromise(service.List(context));
		const invalid_arguments = await Effect.runPromise(
			service
				.Invoke({ revision: 1, tool_id: "hostile.arguments" }, context, {
					workspace_id: hostile,
				})
				.pipe(Effect.flip),
		);
		const invalid_output = await Effect.runPromise(
			service
				.Invoke({ revision: 1, tool_id: "hostile.result" }, context, {})
				.pipe(Effect.flip),
		);
		const after = await Effect.runPromise(service.List(context));

		expect(calls).toBe(0);
		expect(invalid_arguments.reason_code).toBe("invalid_arguments");
		expect(invalid_output.reason_code).toBe("invalid_result");
		expect(after).toEqual(before);
		expect(JSON.stringify({ after, invalid_arguments, invalid_output })).not.toContain(hostile);
	});

	it("returns bounded failures for malformed contexts and eligibility errors", async () => {
		const private_diagnostic = "C:\\private\\eligibility diagnostic";
		const malformed_eligibility = {
			...registration({ tool_id: "malformed.eligibility" }),
			IsEligible: () => Effect.fail({ reason_code: private_diagnostic }),
		};
		const throwing_eligibility = {
			...registration({ tool_id: "throwing.eligibility" }),
			IsEligible: () => {
				throw new Error(private_diagnostic);
			},
		};
		const defect_eligibility = {
			...registration({ tool_id: "defect.eligibility" }),
			IsEligible: () => Effect.die(private_diagnostic),
		};
		const non_effect_eligibility = {
			...registration({ tool_id: "non_effect.eligibility" }),
			IsEligible: () => undefined,
		};
		const service = await Effect.runPromise(
			registry([
				malformed_eligibility,
				throwing_eligibility,
				defect_eligibility,
				non_effect_eligibility,
			]),
		);
		const invalid_context = await Effect.runPromise(
			service.List({ agent_id: "agent" }).pipe(Effect.flip),
		);
		const listed = await Effect.runPromise(service.List(context));

		expect(invalid_context.reason_code).toBe("context_ineligible");
		expect(listed.tools).toHaveLength(4);
		expect(listed.tools).toEqual(
			expect.arrayContaining([
				expect.objectContaining({
					reason_code: "tool_unavailable",
					state: "unavailable",
				}),
			]),
		);
		expect(listed.tools.every((tool) => tool.state === "unavailable")).toBe(true);
		expect(JSON.stringify({ invalid_context, listed })).not.toContain(private_diagnostic);
	});

	it("collapses handler throws and defects into source-safe execution failures", async () => {
		const private_diagnostic = "C:\\private\\handler diagnostic";
		const throwing = registration({
			handler: () => {
				throw new Error(private_diagnostic);
			},
			tool_id: "throwing.handler",
		});
		const defect = registration({
			handler: () => Effect.die(private_diagnostic),
			tool_id: "defect.handler",
		});
		const service = await Effect.runPromise(registry([throwing, defect]));
		const failures = await Effect.runPromise(
			Effect.forEach(["throwing.handler", "defect.handler"], (tool_id) =>
				service.Invoke({ revision: 1, tool_id }, context, {}).pipe(Effect.flip),
			),
		);

		expect(failures.map((failure) => failure.reason_code)).toEqual([
			"execution_failed",
			"execution_failed",
		]);
		expect(JSON.stringify(failures)).not.toContain(private_diagnostic);
	});
});
