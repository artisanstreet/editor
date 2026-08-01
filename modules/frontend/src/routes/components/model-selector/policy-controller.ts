import { Data, Effect, Ref, Semaphore } from "effect";

import type { ThreadSessionPolicy } from "@artisan/protocol";

import { ApplyPolicyPatch } from "./presentation";

type PolicyPersistence = (
	policy: ThreadSessionPolicy,
) => Effect.Effect<ThreadSessionPolicy, { readonly message: string }>;

interface PolicyControllerState {
	readonly authoritative: ThreadSessionPolicy | undefined;
	readonly desired: ThreadSessionPolicy | undefined;
	readonly in_flight: ThreadSessionPolicy | undefined;
	readonly repair_key: string | undefined;
}

export interface PolicyFlushResult {
	readonly confirmed: ReadonlyArray<ThreadSessionPolicy>;
	readonly current: ThreadSessionPolicy | undefined;
}

export class ModelPolicyMutationError extends Data.TaggedError("ModelPolicyMutationError")<{
	readonly cause: unknown;
	readonly message: string;
}> {}

export interface ModelPolicyController {
	readonly Current: Effect.Effect<ThreadSessionPolicy | undefined>;
	readonly Flush: (
		persist: PolicyPersistence,
	) => Effect.Effect<PolicyFlushResult, ModelPolicyMutationError>;
	readonly Patch: (
		patch: Partial<ThreadSessionPolicy>,
	) => Effect.Effect<ThreadSessionPolicy | undefined>;
	readonly Replace: (policy: ThreadSessionPolicy) => Effect.Effect<ThreadSessionPolicy>;
	readonly RequestRepair: (policy: ThreadSessionPolicy) => Effect.Effect<boolean>;
	readonly SetAuthoritative: (policy: ThreadSessionPolicy) => Effect.Effect<ThreadSessionPolicy>;
}

const PolicyKey = (policy: ThreadSessionPolicy): string => JSON.stringify(policy);

const CurrentPolicy = (state: PolicyControllerState): ThreadSessionPolicy | undefined =>
	state.desired ?? state.in_flight ?? state.authoritative;

/**
 * Makes one component-scoped policy controller. Effect Ref owns desired and
 * authoritative state while the semaphore serializes mutation/reconciliation.
 */
export const MakeModelPolicyController = Effect.gen(function* () {
	const state = yield* Ref.make<PolicyControllerState>({
		authoritative: undefined,
		desired: undefined,
		in_flight: undefined,
		repair_key: undefined,
	});
	const mutation_lock = yield* Semaphore.make(1);

	const Current = Effect.gen(function* () {
		const current = yield* Ref.get(state);
		return CurrentPolicy(current);
	});

	const SetAuthoritative = (policy: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			return yield* Ref.modify(state, (current) => {
				const changed =
					current.authoritative === undefined ||
					PolicyKey(current.authoritative) !== PolicyKey(policy);
				const next: PolicyControllerState = {
					...current,
					authoritative: policy,
					repair_key: changed ? undefined : current.repair_key,
				};
				return [CurrentPolicy(next) ?? policy, next] as const;
			});
		});

	const Replace = (policy: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			yield* Ref.update(state, (current) => ({ ...current, desired: policy }));
			return policy;
		});

	const Patch = (patch: Partial<ThreadSessionPolicy>) =>
		Effect.gen(function* () {
			return yield* Ref.modify(state, (current) => {
				const base = CurrentPolicy(current);
				if (base === undefined) return [undefined, current] as const;
				const desired = ApplyPolicyPatch(base, patch);
				return [desired, { ...current, desired }] as const;
			});
		});

	const RequestRepair = (policy: ThreadSessionPolicy) =>
		Effect.gen(function* () {
			return yield* Ref.modify(state, (current) => {
				const repair_key = PolicyKey(policy);
				const current_policy = CurrentPolicy(current);
				if (
					current.repair_key === repair_key ||
					(current_policy !== undefined && PolicyKey(current_policy) === repair_key)
				) {
					return [false, current] as const;
				}
				return [true, { ...current, desired: policy, repair_key }] as const;
			});
		});

	const FlushUnlocked = (persist: PolicyPersistence) =>
		Effect.gen(function* () {
			const confirmed: Array<ThreadSessionPolicy> = [];
			while (true) {
				const desired = yield* Ref.modify(state, (current) => {
					if (current.desired === undefined) return [undefined, current] as const;
					return [
						current.desired,
						{ ...current, desired: undefined, in_flight: current.desired },
					] as const;
				});
				if (desired === undefined) break;

				const authoritative = yield* persist(desired).pipe(
					Effect.mapError(
						(cause) =>
							new ModelPolicyMutationError({
								cause,
								message: cause.message,
							}),
					),
					Effect.tapError(() =>
						Effect.gen(function* () {
							yield* Ref.update(state, (current) => ({
								...current,
								desired: undefined,
								in_flight: undefined,
							}));
						}),
					),
				);

				confirmed.push(authoritative);
				yield* Ref.update(state, (current) => ({
					...current,
					authoritative,
					in_flight: undefined,
				}));
			}

			return { confirmed, current: yield* Current } satisfies PolicyFlushResult;
		});

	const Flush = (persist: PolicyPersistence) =>
		Effect.gen(function* () {
			return yield* mutation_lock.withPermit(FlushUnlocked(persist));
		});

	return {
		Current,
		Flush,
		Patch,
		Replace,
		RequestRepair,
		SetAuthoritative,
	} satisfies ModelPolicyController;
});
