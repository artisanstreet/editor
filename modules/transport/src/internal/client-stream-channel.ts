import { Cause, Deferred, Effect, Option, Queue, Ref, Scope, Stream } from "effect";

import type { ArtisanClientError } from "../client-api/service";
import type { MessagePortStreamFrame, TransportStreamFrame } from "../wire";
import {
	client_error,
	map_port_error,
	type ActiveClientSession,
	type AwaitActive,
} from "./client-common";

interface BinaryChannel {
	readonly connection_id: string;
	readonly expected_sequence: number;
	readonly queue: Queue.Queue<Uint8Array, ArtisanClientError | Cause.Done<void>>;
	readonly ready: Deferred.Deferred<void, ArtisanClientError>;
	readonly stream_id: string;
}

interface BinaryChannelState {
	readonly channels: ReadonlyMap<string, BinaryChannel>;
	readonly disposed: boolean;
	readonly ignored_channels: ReadonlySet<string>;
}

/** Owns logical stream continuity independently from the control connection. */
export interface ClientStreamChannel {
	readonly Disconnect: (connection_id: string) => Effect.Effect<void>;
	readonly Dispose: (error: Option.Option<ArtisanClientError>) => Effect.Effect<void>;
	readonly Handle: (
		active: ActiveClientSession,
		frame: Exclude<MessagePortStreamFrame, { kind: "stream.bind" }>,
	) => Effect.Effect<void, ArtisanClientError>;
	readonly Open: (
		stream_id: string,
	) => Effect.Effect<
		Stream.Stream<Uint8Array, ArtisanClientError>,
		ArtisanClientError,
		Scope.Scope
	>;
}

/** Builds bounded terminal and asset stream delivery with explicit gap closure. */
export const make_client_stream_channel = (
	stream_capacity: number,
	make_id: (prefix: string) => Effect.Effect<string>,
	await_active: AwaitActive,
	current_active: Effect.Effect<Option.Option<ActiveClientSession>>,
) =>
	Effect.gen(function* () {
		const state = yield* Ref.make<BinaryChannelState>({
			channels: new Map(),
			disposed: false,
			ignored_channels: new Set(),
		});

		const send_stream = (active: ActiveClientSession, frame: MessagePortStreamFrame) =>
			active.ports.stream_port
				.Send({
					connection_id: active.connection_id,
					frame,
					kind: "transport.stream",
					transport_version: 1,
				} satisfies TransportStreamFrame)
				.pipe(Effect.mapError(map_port_error));

		const take_channel = (channel_id: string, remember: boolean) =>
			Ref.modify(
				state,
				(current): readonly [Option.Option<BinaryChannel>, BinaryChannelState] => {
					const channel = current.channels.get(channel_id);

					if (!channel) {
						return [Option.none(), current];
					}

					const channels = new Map(current.channels);
					const ignored_channels = new Set(current.ignored_channels);

					channels.delete(channel_id);

					if (remember) {
						ignored_channels.add(channel_id);
					}

					return [Option.some(channel), { ...current, channels, ignored_channels }];
				},
			);

		const fail_channel = (channel_id: string, error: ArtisanClientError, remember = false) =>
			Effect.gen(function* () {
				const channel = yield* take_channel(channel_id, remember);

				if (Option.isSome(channel)) {
					yield* Deferred.fail(channel.value.ready, error);
					yield* Queue.fail(channel.value.queue, error);
				}
			});

		const update_sequence = (channel_id: string, sequence: number) =>
			Ref.update(state, (current) => {
				const channel = current.channels.get(channel_id);

				return channel
					? {
							...current,
							channels: new Map(current.channels).set(channel_id, {
								...channel,
								expected_sequence: sequence,
							}),
						}
					: current;
			});

		const handle = (
			active: ActiveClientSession,
			frame: Exclude<MessagePortStreamFrame, { kind: "stream.bind" }>,
		) =>
			Effect.gen(function* () {
				const current = yield* Ref.get(state);
				const channel = current.channels.get(frame.channel_id);

				if (!channel) {
					if (current.ignored_channels.has(frame.channel_id)) {
						if (frame.kind === "stream.end") {
							yield* Ref.update(state, (value) => {
								const ignored_channels = new Set(value.ignored_channels);

								ignored_channels.delete(frame.channel_id);

								return { ...value, ignored_channels };
							});
						}

						return;
					}

					return yield* Effect.fail(
						client_error(
							"correlation_conflict",
							"The stream channel id is unknown.",
							new Error("unknown binary stream channel"),
						),
					);
				}

				if (
					channel.connection_id !== active.connection_id ||
					channel.stream_id !== frame.stream_id
				) {
					return yield* Effect.fail(
						client_error(
							"correlation_conflict",
							"The stream channel belongs to another connection or stream.",
							new Error("binary stream channel mismatch"),
						),
					);
				}

				if (frame.kind === "stream.ready") {
					if (frame.channel_sequence !== 0 || channel.expected_sequence !== -1) {
						const error = client_error(
							"stream_gap",
							"The binary stream readiness sequence is invalid.",
							new Error("invalid stream.ready sequence"),
						);

						yield* fail_channel(frame.channel_id, error, true);
						yield* send_stream(active, {
							channel_id: frame.channel_id,
							channel_sequence: 1,
							kind: "stream.end",
							reason: "gap",
							stream_id: frame.stream_id,
						}).pipe(Effect.ignore);

						return;
					}

					yield* update_sequence(frame.channel_id, 0);
					yield* Deferred.succeed(channel.ready, undefined);

					return;
				}

				if (frame.kind === "stream.chunk") {
					if (frame.channel_sequence !== channel.expected_sequence + 1) {
						const error = client_error(
							"stream_gap",
							"The binary stream sequence contained a gap.",
							new Error("binary stream sequence gap"),
						);

						yield* fail_channel(frame.channel_id, error, true);
						yield* send_stream(active, {
							channel_id: frame.channel_id,
							channel_sequence: 1,
							kind: "stream.end",
							reason: "gap",
							stream_id: frame.stream_id,
						}).pipe(Effect.ignore);

						return;
					}

					if (!Queue.offerUnsafe(channel.queue, frame.data.slice())) {
						const error = client_error(
							"stream_overflow",
							"The binary stream consumer queue overflowed.",
							new Error("binary stream queue overflow"),
						);

						yield* fail_channel(frame.channel_id, error, true);
						yield* send_stream(active, {
							channel_id: frame.channel_id,
							channel_sequence: 1,
							kind: "stream.end",
							reason: "cancelled",
							stream_id: frame.stream_id,
						}).pipe(Effect.ignore);

						return;
					}

					yield* update_sequence(frame.channel_id, frame.channel_sequence);

					return;
				}

				const exact_end = frame.channel_sequence === channel.expected_sequence + 1;

				if (frame.reason === "completed" && exact_end) {
					yield* take_channel(frame.channel_id, false);
					yield* Queue.end(channel.queue);

					return;
				}

				const missing_asset = frame.reason === "not_found";
				const source_unavailable = frame.reason === "source_error";
				const code =
					frame.reason === "overflow"
						? "stream_overflow"
						: missing_asset && exact_end
							? "stream_not_found"
							: exact_end
								? "stream_closed"
								: "stream_gap";

				yield* fail_channel(
					frame.channel_id,
					client_error(
						code,
						frame.reason === "overflow"
							? "The backend stream queue overflowed."
							: missing_asset && exact_end
								? "The requested binary stream was not found."
								: source_unavailable && exact_end
									? "The binary stream source is temporarily unavailable."
									: "The binary stream closed before clean completion.",
						new Error(`binary stream ended with ${frame.reason}`),
						source_unavailable && exact_end,
					),
				);
			});

		const remove = (channel_id: string) =>
			Effect.gen(function* () {
				const channel = yield* take_channel(channel_id, true);

				if (Option.isNone(channel)) {
					return;
				}

				yield* Queue.end(channel.value.queue);
				const active = yield* current_active;

				if (
					Option.isSome(active) &&
					active.value.connection_id === channel.value.connection_id
				) {
					yield* send_stream(active.value, {
						channel_id,
						channel_sequence: 1,
						kind: "stream.end",
						reason: "cancelled",
						stream_id: channel.value.stream_id,
					}).pipe(Effect.ignore);
				}
			});

		const open = (stream_id: string) =>
			Effect.gen(function* () {
				const active = yield* await_active;
				const channel_id = yield* make_id("binary_stream");
				const queue = yield* Effect.acquireRelease(
					Queue.dropping<Uint8Array, ArtisanClientError | Cause.Done<void>>(
						stream_capacity,
					),
					Queue.shutdown,
				);
				const ready = yield* Deferred.make<void, ArtisanClientError>();
				const channel: BinaryChannel = {
					connection_id: active.connection_id,
					expected_sequence: -1,
					queue,
					ready,
					stream_id,
				};
				const inserted = yield* Ref.modify(
					state,
					(current): readonly [boolean, BinaryChannelState] =>
						current.disposed
							? [false, current]
							: [
									true,
									{
										...current,
										channels: new Map(current.channels).set(
											channel_id,
											channel,
										),
									},
								],
				);

				if (!inserted) {
					return yield* Effect.fail(
						client_error(
							"disposed",
							"The Artisan client was disposed.",
							new Error("client disposed"),
						),
					);
				}

				yield* Effect.addFinalizer(() => remove(channel_id));
				yield* send_stream(active, {
					channel_id,
					channel_sequence: 0,
					kind: "stream.bind",
					stream_id,
					stream_ticket: active.stream_ticket,
				});
				yield* Deferred.await(ready).pipe(Effect.onInterrupt(() => remove(channel_id)));

				return Stream.fromQueue(queue);
			});

		const disconnect = (connection_id: string) =>
			Effect.gen(function* () {
				const channels = yield* Ref.modify(
					state,
					(current): readonly [ReadonlyArray<BinaryChannel>, BinaryChannelState] => {
						const closed = [...current.channels.values()].filter(
							(channel) => channel.connection_id === connection_id,
						);
						const retained = new Map(
							[...current.channels].filter(
								([, channel]) => channel.connection_id !== connection_id,
							),
						);

						return [
							closed,
							{ ...current, channels: retained, ignored_channels: new Set() },
						];
					},
				);
				const error = client_error(
					"stream_closed",
					"The transient binary stream connection closed.",
					new Error("transport session closed"),
					true,
				);

				yield* Effect.forEach(
					channels,
					(channel) =>
						Effect.all(
							[Deferred.fail(channel.ready, error), Queue.fail(channel.queue, error)],
							{ discard: true },
						),
					{ discard: true },
				);
			});

		const dispose = (error: Option.Option<ArtisanClientError>) =>
			Effect.gen(function* () {
				const current = yield* Ref.getAndSet(state, {
					channels: new Map(),
					disposed: true,
					ignored_channels: new Set<string>(),
				});

				yield* Effect.forEach(
					current.channels.values(),
					(channel) =>
						Option.match(error, {
							onNone: () => Queue.end(channel.queue),
							onSome: (failure) =>
								Effect.all(
									[
										Deferred.fail(channel.ready, failure),
										Queue.fail(channel.queue, failure),
									],
									{ discard: true },
								),
						}),
					{ discard: true },
				);
			});

		return {
			Disconnect: disconnect,
			Dispose: dispose,
			Handle: handle,
			Open: open,
		} satisfies ClientStreamChannel;
	});
