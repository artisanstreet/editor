import { MessageChannel, type MessagePort } from "node:worker_threads";

import { Effect, Exit, Layer, ManagedRuntime, Stream } from "effect";

import { ProtocolServer } from "@artisan/backend";
import {
	DecodeInboundControlEnvelope,
	DecodeOutboundControlEnvelope,
	type OutboundEnvelope,
	type PreviewBrowserLifecycleQueryEnvelope,
	type RichLinkMetadataQueryEnvelope,
	type ToolApprovalDecideEnvelope,
	type ToolInvokeEnvelope,
} from "@artisan/protocol";
import {
	ArtisanClient,
	MessagePortConnector,
	MessagePortConnectorError,
	TransportRuntime,
	TransportRuntimeLive,
	DecodeTransportFrame,
	make_artisan_client_layer,
	type ArtisanClientOptions,
	type MessagePortConnection,
	type MessagePortLike,
} from "@artisan/transport";
import { adapt_node_message_port } from "@artisan/transport/node";
import {
	BinaryStreamSource,
	BinaryStreamSourceError,
	MessagePortTransportServer,
	make_message_port_transport_server_layer,
	type MessagePortTransportServerOptions,
} from "@artisan/transport/server";

import {
	make_fake_protocol_server,
	type FakeProtocolOptions,
	type FakeProtocolSnapshot,
} from "./fake-protocol-server";

interface NativeSession {
	readonly control_client: MessagePort;
	readonly control_server: MessagePort;
	readonly stream_client: MessagePort;
	readonly stream_server: MessagePort;
}

/** Configures one real-MessageChannel integration harness. */
export interface TransportHarnessOptions {
	readonly binary_stream_source?: typeof BinaryStreamSource.Service;
	readonly binary_streams?: Readonly<Record<string, ReadonlyArray<Uint8Array>>>;
	readonly client?: ArtisanClientOptions;
	readonly drop_first_command_receipt?: boolean;
	readonly drop_first_preview_browser_lifecycle_result?: boolean;
	readonly drop_first_rich_link_metadata_result?: boolean;
	readonly drop_first_tool_approval_decide_result?: boolean;
	readonly drop_first_tool_invoke_result?: boolean;
	readonly protocol?: FakeProtocolOptions;
	readonly server?: MessagePortTransportServerOptions;
	readonly transport_runtime?: Layer.Layer<TransportRuntime>;
}

/** Reports connector ownership and injected-fault observations. */
export interface MessageChannelConnectorSnapshot {
	readonly active_sessions: number;
	readonly connections: number;
	readonly dropped_command_receipts: number;
	readonly dropped_preview_browser_lifecycle_results: number;
	readonly dropped_rich_link_metadata_results: number;
	readonly dropped_tool_approval_decide_results: number;
	readonly dropped_tool_invoke_results: number;
	readonly preview_browser_lifecycle_query_attempts: ReadonlyArray<PreviewBrowserLifecycleQueryEnvelope>;
	readonly rich_link_metadata_query_attempts: ReadonlyArray<RichLinkMetadataQueryEnvelope>;
	readonly tool_approval_decide_attempts: ReadonlyArray<ToolApprovalDecideEnvelope>;
	readonly tool_invoke_attempts: ReadonlyArray<ToolInvokeEnvelope>;
	readonly server_failures: number;
}

/** Owns one client, server, fake backend, and their managed scopes. */
export interface TransportTestHarness {
	readonly client: typeof ArtisanClient.Service;
	readonly close_current_connection: () => void;
	readonly connector_snapshot: () => MessageChannelConnectorSnapshot;
	readonly dispose: () => Promise<void>;
	readonly erase_thread: (thread_id: string) => Effect.Effect<void>;
	readonly protocol_snapshot: () => FakeProtocolSnapshot;
	readonly server: typeof MessagePortTransportServer.Service;
}

/** Owns the real MessageChannel stack around a supplied protocol server. */
export type ProtocolTransportTestHarness = Omit<
	TransportTestHarness,
	"erase_thread" | "protocol_snapshot"
>;

function close_native_session(session: NativeSession) {
	session.control_client.close();
	session.control_server.close();
	session.stream_client.close();
	session.stream_server.close();
}

const has_outbound_kind = (message: unknown, kind: OutboundEnvelope["kind"]) =>
	DecodeTransportFrame(message).pipe(
		Effect.flatMap((frame) =>
			frame.kind === "transport.control"
				? DecodeOutboundControlEnvelope(frame.payload).pipe(
						Effect.map((envelope) => envelope.kind === kind),
					)
				: Effect.succeed(false),
		),
		Effect.catch(() => Effect.succeed(false)),
	);

const get_rich_link_metadata_query = (message: unknown) =>
	DecodeTransportFrame(message).pipe(
		Effect.flatMap((frame) =>
			frame.kind === "transport.control"
				? DecodeInboundControlEnvelope(frame.payload).pipe(
						Effect.map((envelope) =>
							envelope.kind === "rich-link.metadata.query" ? envelope : undefined,
						),
					)
				: Effect.succeed(undefined),
		),
		Effect.catch(() => Effect.succeed(undefined)),
	);

const get_preview_browser_lifecycle_query = (message: unknown) =>
	DecodeTransportFrame(message).pipe(
		Effect.flatMap((frame) =>
			frame.kind === "transport.control"
				? DecodeInboundControlEnvelope(frame.payload).pipe(
						Effect.map((envelope) =>
							envelope.kind === "preview.browser.lifecycle.query"
								? envelope
								: undefined,
						),
					)
				: Effect.succeed(undefined),
		),
		Effect.catch(() => Effect.succeed(undefined)),
	);

const get_tool_invoke = (message: unknown) =>
	DecodeTransportFrame(message).pipe(
		Effect.flatMap((frame) =>
			frame.kind === "transport.control"
				? DecodeInboundControlEnvelope(frame.payload).pipe(
						Effect.map((envelope) =>
							envelope.kind === "tool.invoke" ? envelope : undefined,
						),
					)
				: Effect.succeed(undefined),
		),
		Effect.catch(() => Effect.succeed(undefined)),
	);

const get_tool_approval_decide = (message: unknown) =>
	DecodeTransportFrame(message).pipe(
		Effect.flatMap((frame) =>
			frame.kind === "transport.control"
				? DecodeInboundControlEnvelope(frame.payload).pipe(
						Effect.map((envelope) =>
							envelope.kind === "tool.approval.decide" ? envelope : undefined,
						),
					)
				: Effect.succeed(undefined),
		),
		Effect.catch(() => Effect.succeed(undefined)),
	);

function make_binary_source_layer(
	binary_streams: Readonly<Record<string, ReadonlyArray<Uint8Array>>>,
) {
	return Layer.succeed(BinaryStreamSource, {
		Open: (stream_id) => {
			const chunks = binary_streams[stream_id];

			return chunks
				? Effect.succeed(
						Stream.fromIterable(chunks).pipe(Stream.map((chunk) => chunk.slice())),
					)
				: Effect.fail(
						new BinaryStreamSourceError({
							cause: new Error("test stream not found"),
							code: "not_found",
							stream_id,
						}),
					);
		},
	});
}

function make_message_channel_connector(
	server: typeof MessagePortTransportServer.Service,
	drop_first_command_receipt: boolean,
	drop_first_preview_browser_lifecycle_result: boolean,
	drop_first_rich_link_metadata_result: boolean,
	drop_first_tool_approval_decide_result: boolean,
	drop_first_tool_invoke_result: boolean,
) {
	const active_sessions = new Set<NativeSession>();
	const preview_browser_lifecycle_query_attempts: Array<PreviewBrowserLifecycleQueryEnvelope> =
		[];
	const rich_link_metadata_query_attempts: Array<RichLinkMetadataQueryEnvelope> = [];
	const tool_approval_decide_attempts: Array<ToolApprovalDecideEnvelope> = [];
	const tool_invoke_attempts: Array<ToolInvokeEnvelope> = [];
	let connections = 0;
	let dropped_command_receipts = 0;
	let dropped_preview_browser_lifecycle_results = 0;
	let dropped_rich_link_metadata_results = 0;
	let dropped_tool_approval_decide_results = 0;
	let dropped_tool_invoke_results = 0;
	let server_failures = 0;

	const Connect = Effect.gen(function* () {
		const control = new MessageChannel();
		const stream = new MessageChannel();
		const native_session: NativeSession = {
			control_client: control.port1,
			control_server: control.port2,
			stream_client: stream.port1,
			stream_server: stream.port2,
		};

		connections += 1;
		active_sessions.add(native_session);

		const adapt = (port: MessagePort) =>
			adapt_node_message_port(port).pipe(
				Effect.mapError((cause) => new MessagePortConnectorError({ cause })),
			);
		const raw_client_control = yield* adapt(control.port1);
		const client_stream = yield* adapt(stream.port1);
		const raw_server_control = yield* adapt(control.port2);
		const server_stream = yield* adapt(stream.port2);
		const client_control: MessagePortLike = {
			...raw_client_control,
			Send: (message, transfer) =>
				Effect.gen(function* () {
					const browser_attempt = yield* get_preview_browser_lifecycle_query(message);
					const attempt = yield* get_rich_link_metadata_query(message);
					const tool_approval_decide_attempt = yield* get_tool_approval_decide(message);
					const tool_invoke_attempt = yield* get_tool_invoke(message);

					if (browser_attempt) {
						preview_browser_lifecycle_query_attempts.push(browser_attempt);
					}

					if (attempt) {
						rich_link_metadata_query_attempts.push(attempt);
					}

					if (tool_approval_decide_attempt) {
						tool_approval_decide_attempts.push(tool_approval_decide_attempt);
					}

					if (tool_invoke_attempt) {
						tool_invoke_attempts.push(tool_invoke_attempt);
					}

					yield* raw_client_control.Send(message, transfer);
				}),
		};
		const server_control: MessagePortLike = {
			...raw_server_control,
			Send: (message, transfer) =>
				Effect.gen(function* () {
					const should_drop_command_receipt =
						drop_first_command_receipt && dropped_command_receipts === 0
							? yield* has_outbound_kind(message, "command.receipt")
							: false;
					const should_drop_preview_browser_lifecycle_result =
						drop_first_preview_browser_lifecycle_result &&
						dropped_preview_browser_lifecycle_results === 0
							? yield* has_outbound_kind(
									message,
									"preview.browser.lifecycle.query.result",
								)
							: false;
					const should_drop_rich_link_metadata_result =
						drop_first_rich_link_metadata_result &&
						dropped_rich_link_metadata_results === 0
							? yield* has_outbound_kind(message, "rich-link.metadata.query.result")
							: false;
					const should_drop_tool_approval_decide_result =
						drop_first_tool_approval_decide_result &&
						dropped_tool_approval_decide_results === 0
							? yield* has_outbound_kind(message, "tool.approval.decide.result")
							: false;
					const should_drop_tool_invoke_result =
						drop_first_tool_invoke_result && dropped_tool_invoke_results === 0
							? yield* has_outbound_kind(message, "tool.invoke.result")
							: false;

					if (should_drop_command_receipt) {
						dropped_command_receipts += 1;
						close_native_session(native_session);

						return;
					}

					if (should_drop_preview_browser_lifecycle_result) {
						dropped_preview_browser_lifecycle_results += 1;
						close_native_session(native_session);

						return;
					}

					if (should_drop_rich_link_metadata_result) {
						dropped_rich_link_metadata_results += 1;
						close_native_session(native_session);

						return;
					}

					if (should_drop_tool_approval_decide_result) {
						dropped_tool_approval_decide_results += 1;
						close_native_session(native_session);

						return;
					}

					if (should_drop_tool_invoke_result) {
						dropped_tool_invoke_results += 1;
						close_native_session(native_session);

						return;
					}

					yield* raw_server_control.Send(message, transfer);
				}),
		};
		const server_ports: MessagePortConnection = {
			control_port: server_control,
			stream_port: server_stream,
		};

		yield* server.Serve(server_ports).pipe(
			Effect.exit,
			Effect.tap((exit) =>
				Effect.sync(() => {
					if (Exit.isFailure(exit)) {
						server_failures += 1;
					}
				}),
			),
			Effect.ensuring(
				Effect.sync(() => {
					active_sessions.delete(native_session);
				}),
			),
			Effect.forkScoped,
		);
		yield* Effect.addFinalizer(() =>
			Effect.sync(() => {
				active_sessions.delete(native_session);
				close_native_session(native_session);
			}),
		);

		return {
			control_port: client_control,
			stream_port: client_stream,
		} satisfies MessagePortConnection;
	});
	const layer = Layer.succeed(MessagePortConnector, { Connect });
	const close_current_connection = () => {
		const current = [...active_sessions].at(-1);

		if (current) {
			close_native_session(current);
		}
	};
	const snapshot = (): MessageChannelConnectorSnapshot => ({
		active_sessions: active_sessions.size,
		connections,
		dropped_command_receipts,
		dropped_preview_browser_lifecycle_results,
		dropped_rich_link_metadata_results,
		dropped_tool_approval_decide_results,
		dropped_tool_invoke_results,
		preview_browser_lifecycle_query_attempts: [...preview_browser_lifecycle_query_attempts],
		rich_link_metadata_query_attempts: [...rich_link_metadata_query_attempts],
		tool_approval_decide_attempts: [...tool_approval_decide_attempts],
		tool_invoke_attempts: [...tool_invoke_attempts],
		server_failures,
	});

	return { close_current_connection, layer, snapshot };
}

async function make_transport_stack(
	protocol_layer: Layer.Layer<ProtocolServer>,
	options: TransportHarnessOptions,
): Promise<ProtocolTransportTestHarness> {
	const binary_streams = options.binary_streams ?? {
		"asset:asset_1": [Uint8Array.of(1, 2), Uint8Array.of(3, 4, 5)],
	};
	const binary_stream_source = options.binary_stream_source
		? Layer.succeed(BinaryStreamSource, options.binary_stream_source)
		: make_binary_source_layer(binary_streams);
	const transport_runtime = options.transport_runtime ?? TransportRuntimeLive;
	const server_layer = make_message_port_transport_server_layer(options.server).pipe(
		Layer.provide(protocol_layer),
		Layer.provide(binary_stream_source),
		Layer.provide(transport_runtime),
	);
	const server_runtime = ManagedRuntime.make(server_layer);
	const server = await server_runtime.runPromise(MessagePortTransportServer);
	const connector = make_message_channel_connector(
		server,
		options.drop_first_command_receipt ?? false,
		options.drop_first_preview_browser_lifecycle_result ?? false,
		options.drop_first_rich_link_metadata_result ?? false,
		options.drop_first_tool_approval_decide_result ?? false,
		options.drop_first_tool_invoke_result ?? false,
	);
	const client_layer = make_artisan_client_layer(options.client).pipe(
		Layer.provide(connector.layer),
		Layer.provide(transport_runtime),
	);
	const client_runtime = ManagedRuntime.make(client_layer);
	const client = await client_runtime.runPromise(ArtisanClient);
	let disposed = false;

	const dispose = async () => {
		if (disposed) {
			return;
		}

		disposed = true;
		await Effect.runPromise(client.Dispose);
		await client_runtime.dispose();
		await server_runtime.dispose();
	};

	return {
		client,
		close_current_connection: connector.close_current_connection,
		connector_snapshot: connector.snapshot,
		dispose,
		server,
	};
}

/** Creates a scoped transport stack while every ordinary test remains offline. */
export async function make_transport_test_harness(
	options: TransportHarnessOptions = {},
): Promise<TransportTestHarness> {
	const fake_protocol = make_fake_protocol_server(options.protocol);
	const harness = await make_transport_stack(fake_protocol.layer, options);

	return {
		...harness,
		erase_thread: fake_protocol.EraseThread,
		protocol_snapshot: fake_protocol.snapshot,
	};
}

/** Wraps an actual backend ProtocolServer in the real MessagePort transport stack. */
export function make_transport_test_harness_with_protocol_server(
	protocol_server: typeof ProtocolServer.Service,
	options: Omit<TransportHarnessOptions, "protocol"> = {},
): Promise<ProtocolTransportTestHarness> {
	return make_transport_stack(Layer.succeed(ProtocolServer, protocol_server), options);
}

/** Waits for one asynchronous lifecycle observation with a strict deadline. */
export async function wait_for(predicate: () => boolean, timeout_ms = 2_000): Promise<void> {
	const deadline = Date.now() + timeout_ms;

	while (!predicate()) {
		if (Date.now() >= deadline) {
			throw new Error("Timed out waiting for transport test condition");
		}

		await new Promise((resolve) => setTimeout(resolve, 5));
	}
}
