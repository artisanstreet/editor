import { Effect } from "effect";

import type {
	HostMachineConnectRequestEnvelope,
	HostMachineConnectResultEnvelope,
	HostMachinesQueryEnvelope,
	HostMachinesQueryResultEnvelope,
	ProtocolErrorDetail,
} from "@artisan/protocol";

import { HostMachineBrokerService } from "../../../runtime/host-machine-broker";
import { HostMachinesService } from "../../../runtime/host-machines";
import { RuntimeMetadata } from "../../../runtime/metadata";

type HostMachinesEnvelope = HostMachinesQueryEnvelope | HostMachineConnectRequestEnvelope;

export const MakeHostMachinesQueryHandler = Effect.gen(function* () {
	const host_machines = yield* HostMachinesService;
	const broker = yield* HostMachineBrokerService;
	const metadata = yield* RuntimeMetadata;

	const Envelope = <Kind extends string, Payload>(
		request: HostMachinesEnvelope,
		kind: Kind,
		payload: Payload,
	) =>
		Effect.gen(function* () {
			const message_id = yield* metadata.MakeId("message");
			const sent_at = yield* metadata.Now;

			return {
				correlation_id: request.message_id,
				kind,
				message_id,
				origin: "backend" as const,
				payload,
				protocol_version: 1 as const,
				schema_version: 1 as const,
				sent_at,
			};
		});

	const HandleQuery = (
		query: HostMachinesQueryEnvelope,
	): Effect.Effect<HostMachinesQueryResultEnvelope, ProtocolErrorDetail> =>
		host_machines.List.pipe(
			Effect.flatMap((snapshot) =>
				Envelope(query, "host.machines.query.result" as const, snapshot),
			),
		);

	const HandleConnect = (
		request: HostMachineConnectRequestEnvelope,
	): Effect.Effect<HostMachineConnectResultEnvelope, ProtocolErrorDetail> =>
		broker
			.Connect(request.payload.machine_id)
			.pipe(
				Effect.flatMap((outcome) =>
					Envelope(request, "host.machines.connect.result" as const, outcome),
				),
			);

	return (
		request: HostMachinesEnvelope,
	): Effect.Effect<
		HostMachinesQueryResultEnvelope | HostMachineConnectResultEnvelope,
		ProtocolErrorDetail
	> => {
		switch (request.kind) {
			case "host.machines.query":
				return HandleQuery(request);
			case "host.machines.connect.request":
				return HandleConnect(request);
		}
	};
});
