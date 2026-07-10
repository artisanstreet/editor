import { Deferred, Effect, Exit, Queue, Ref, Scope, Semaphore, Stream } from "effect";

import type {
	Engine,
	EngineCapability,
	EngineCapabilityName,
	EngineCapabilities,
	EngineCommand,
	EngineCommandFailure,
	EngineDescriptor,
	EngineFailure,
	EngineObservation,
	EngineOpenInput,
	EngineProbe,
	EngineRun,
	EngineRunTerminalState,
} from "@artisan/engines";
import {
	EngineBackpressureError,
	EngineCommandIdConflictError,
	EngineRunClosedError,
	EngineUnsupportedCommandError,
} from "@artisan/engines";

/** Configures the deterministic test-only engine adapter. @since 0.2.0 */
export interface FakeEngineOptions {
	readonly engine_id?: string;
	readonly event_capacity?: number;
	readonly on_cleanup?: () => void;
	readonly unsupported_commands?: ReadonlyArray<EngineCommand["_tag"]>;
}

/** Marks the end of a fake event stream without leaking into the engine contract. */
interface FakeStreamEnd {
	readonly _tag: "fake_stream_end";
}

/** Captures private lifecycle state for a deterministic fake run. */
interface FakeRunState {
	readonly command_intents: ReadonlyMap<string, string>;
	readonly closed: boolean;
	readonly next_observation_id: number;
	readonly terminal_state?: EngineRunTerminalState;
}

function capability(state: EngineCapability["state"], reason?: string): EngineCapability {
	return reason ? { reason, state } : { state };
}

function command_capability(command: EngineCommand): EngineCapabilityName {
	if (command._tag === "respond_approval") {
		return "approval";
	}

	if (command._tag === "respond_question") {
		return "question";
	}

	return command._tag;
}

function command_intent(command: EngineCommand) {
	switch (command._tag) {
		case "steer":
			return JSON.stringify([command._tag, command.text]);
		case "respond_approval":
			return JSON.stringify([command._tag, command.approval_id, command.approved]);
		case "respond_question":
			return JSON.stringify([
				command._tag,
				Object.entries(command.answers).sort(([left], [right]) =>
					left.localeCompare(right),
				),
			]);
		case "cancel":
		case "close":
			return JSON.stringify([command._tag]);
	}
}

function make_capabilities(
	unsupported_commands: ReadonlySet<EngineCommand["_tag"]>,
): EngineCapabilities {
	const command_capability_state = (command: EngineCommand["_tag"]) =>
		unsupported_commands.has(command)
			? capability("unsupported", "Disabled by the deterministic scenario")
			: capability("supported");

	return {
		approval: command_capability_state("respond_approval"),
		auth: capability("supported"),
		cancel: command_capability_state("cancel"),
		close: command_capability_state("close"),
		events: capability("supported"),
		model_selection: capability("supported"),
		native_tools: capability("unsupported", "The fake has no provider-native tools"),
		probe: capability("supported"),
		question: command_capability_state("respond_question"),
		raw_frames: capability("supported"),
		resume: capability("supported"),
		start: capability("supported"),
		steer: command_capability_state("steer"),
		subagents: capability("experimental", "Scenarios do not model subagent execution"),
	};
}

function is_observation(event: EngineObservation | FakeStreamEnd): event is EngineObservation {
	return event._tag !== "fake_stream_end";
}

function make_descriptor(options: FakeEngineOptions): EngineDescriptor {
	const unsupported_commands = new Set(options.unsupported_commands ?? []);

	return {
		capabilities: make_capabilities(unsupported_commands),
		display_name: "Deterministic fake engine",
		id: options.engine_id ?? "fake",
		transport: "deterministic-test",
	};
}

function make_observation(
	state: Ref.Ref<FakeRunState>,
	artisan_run_id: string,
	engine_id: string,
	transport: string,
	frame: unknown,
) {
	return Effect.gen(function* () {
		const observation_id = yield* Ref.modify(state, (current) => {
			const next_observation_id = current.next_observation_id + 1;

			return [next_observation_id, { ...current, next_observation_id }];
		});

		return {
			artisan_run_id,
			observation_id: `${artisan_run_id}:${observation_id}`,
			raw: { engine_id, frame, frame_sequence: observation_id, transport },
			sequence: observation_id,
		};
	});
}

function open_fake_run(
	descriptor: EngineDescriptor,
	input: EngineOpenInput,
	event_capacity: number,
	on_cleanup: (() => void) | undefined,
): Effect.Effect<EngineRun, EngineFailure, Scope.Scope> {
	return Effect.gen(function* () {
		const run_scope = yield* Effect.acquireRelease(Scope.make(), (scope) =>
			Scope.close(scope, Exit.succeed(undefined)),
		);
		const queue = yield* Queue.dropping<EngineObservation | FakeStreamEnd>(event_capacity + 2);
		const closed = yield* Deferred.make<EngineRunTerminalState>();
		const lock = yield* Semaphore.make(1);
		const state = yield* Ref.make<FakeRunState>({
			command_intents: new Map(),
			closed: false,
			next_observation_id: 0,
		});
		const native_thread_id =
			input._tag === "resume"
				? input.resume_token.native_thread_id
				: `native:${input.artisan_run_id}`;

		const EnsureEventCapacity = Effect.gen(function* () {
			const size = yield* Queue.size(queue);

			if (size >= event_capacity) {
				return yield* Effect.fail(
					new EngineBackpressureError({
						artisan_run_id: input.artisan_run_id,
						capacity: event_capacity,
					}),
				);
			}
		});
		const Offer = (observation: EngineObservation) => Queue.offer(queue, observation);
		const terminal = (terminal_state: EngineRunTerminalState) =>
			Effect.gen(function* () {
				const current = yield* Ref.get(state);

				if (current.closed) {
					return;
				}

				const base = yield* make_observation(
					state,
					input.artisan_run_id,
					descriptor.id,
					descriptor.transport,
					{ source: "scope" },
				);

				yield* Ref.update(state, (next) => ({
					...next,
					closed: true,
					terminal_state,
				}));
				yield* Queue.offer(queue, {
					...base,
					_tag: "run_terminal",
					state: terminal_state,
				});
				yield* Queue.offer(queue, { _tag: "fake_stream_end" });
				yield* Deferred.succeed(closed, terminal_state);
				yield* Effect.sync(() => on_cleanup?.());
			});
		const finalize = Semaphore.withPermit(lock)(
			Effect.gen(function* () {
				const current = yield* Ref.get(state);

				yield* terminal(current.terminal_state ?? "closed");
			}),
		);

		yield* Scope.addFinalizer(run_scope, finalize);

		const emit_run_state = (state_name: "opening" | "running" | "waiting", frame: unknown) =>
			Effect.gen(function* () {
				yield* EnsureEventCapacity;

				const base = yield* make_observation(
					state,
					input.artisan_run_id,
					descriptor.id,
					descriptor.transport,
					frame,
				);

				yield* Offer({ ...base, _tag: "run_state", state: state_name });
			});
		const Send = (command: EngineCommand): Effect.Effect<void, EngineCommandFailure> =>
			Semaphore.withPermit(lock)(
				Effect.gen(function* () {
					const current = yield* Ref.get(state);
					const intent = command_intent(command);
					const accepted_intent = current.command_intents.get(command.command_id);

					if (accepted_intent) {
						if (accepted_intent === intent) {
							return false;
						}

						return yield* Effect.fail(
							new EngineCommandIdConflictError({
								artisan_run_id: input.artisan_run_id,
								command_id: command.command_id,
							}),
						);
					}

					if (current.closed && command._tag !== "close") {
						return yield* Effect.fail(
							new EngineRunClosedError({
								artisan_run_id: input.artisan_run_id,
								command_id: command.command_id,
							}),
						);
					}

					const capability_name = command_capability(command);
					const capability_state = descriptor.capabilities[capability_name];

					if (capability_state.state === "unsupported") {
						return yield* Effect.fail(
							new EngineUnsupportedCommandError({
								command: command._tag,
								command_id: command.command_id,
								engine_id: descriptor.id,
							}),
						);
					}

					if (command._tag === "close") {
						yield* Ref.update(state, (next) => ({
							...next,
							command_intents: new Map(next.command_intents).set(
								command.command_id,
								intent,
							),
						}));

						return !current.closed;
					}

					if (command._tag === "cancel") {
						yield* Ref.update(state, (next) => ({
							...next,
							command_intents: new Map(next.command_intents).set(
								command.command_id,
								intent,
							),
							terminal_state: "cancelled" as const,
						}));
						return true;
					}

					yield* EnsureEventCapacity;

					const base = yield* make_observation(
						state,
						input.artisan_run_id,
						descriptor.id,
						descriptor.transport,
						{ command_id: command.command_id, command: command._tag },
					);
					const observation =
						command._tag === "steer"
							? {
									...base,
									_tag: "agent_message_delta" as const,
									delta: command.text,
									turn_id: command.command_id,
								}
							: command._tag === "respond_approval"
								? {
										...base,
										_tag: "turn_state" as const,
										state: "started" as const,
										turn_id: command.approval_id,
									}
								: {
										...base,
										_tag: "agent_message_completed" as const,
										message: Object.values(command.answers).flat().join("\n"),
										turn_id:
											Object.keys(command.answers)[0] ?? command.command_id,
									};

					yield* Offer(observation);
					yield* Ref.update(state, (next) => ({
						...next,
						command_intents: new Map(next.command_intents).set(
							command.command_id,
							intent,
						),
					}));

					return false;
				}),
			).pipe(
				Effect.flatMap((should_close) =>
					should_close ? Scope.close(run_scope, Exit.succeed(undefined)) : Effect.void,
				),
			);

		yield* emit_run_state("opening", { input: input._tag });
		yield* emit_run_state("running", { native_thread_id });

		return {
			artisan_run_id: input.artisan_run_id,
			Closed: Deferred.await(closed),
			Events: Stream.fromQueue(queue).pipe(
				Stream.takeUntil((event) => event._tag === "fake_stream_end"),
				Stream.filter(is_observation),
			),
			native_thread_id,
			resume_token: {
				native_thread_id,
				...(input._tag === "resume" && input.resume_token.opaque_checkpoint
					? { opaque_checkpoint: input.resume_token.opaque_checkpoint }
					: {}),
			},
			Send,
		};
	});
}

/**
 * Creates a deterministic scoped engine adapter for conformance tests.
 *
 * @since 0.2.0
 * @param options - Scenario-specific fake behavior and observation capacity.
 * @returns An adapter that exercises the production Engine seam without I/O.
 */
export function make_fake_engine(options: FakeEngineOptions = {}): Engine {
	const descriptor = make_descriptor(options);
	const event_capacity = options.event_capacity ?? 16;
	const Probe = (): Effect.Effect<EngineProbe> =>
		Effect.succeed({
			authentication: { state: "authenticated" },
			capabilities: descriptor.capabilities,
			descriptor,
			metadata: { adapter: "deterministic" },
			ready: true,
			version: "test-1.0.0",
		});

	return {
		Descriptor: descriptor,
		Open: (input) => open_fake_run(descriptor, input, event_capacity, options.on_cleanup),
		Probe,
	};
}
