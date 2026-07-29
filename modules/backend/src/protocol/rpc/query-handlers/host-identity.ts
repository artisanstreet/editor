import { Effect } from "effect";

import type {
	HostIdentityQueryEnvelope,
	HostIdentityQueryResultEnvelope,
	ProtocolErrorDetail,
} from "@artisan/protocol";

import { HostIdentityService } from "../../../runtime/host-identity";
import { RuntimeMetadata } from "../../../runtime/runtime-metadata";

export const MakeHostIdentityQueryHandler = Effect.gen(function* () {
	const host_identity = yield* HostIdentityService;
	const metadata = yield* RuntimeMetadata;

	const Envelope = <Kind extends HostIdentityQueryResultEnvelope["kind"], Payload>(
		query: HostIdentityQueryEnvelope,
		kind: Kind,
		payload: Payload,
	) =>
		Effect.gen(function* () {
			const message_id = yield* metadata.MakeId("message");
			const sent_at = yield* metadata.Now;

			return {
				correlation_id: query.message_id,
				kind,
				message_id,
				origin: "backend" as const,
				payload,
				protocol_version: 1 as const,
				schema_version: 1 as const,
				sent_at,
			};
		});

	const handlers = {
		"host.identity.query": (query: HostIdentityQueryEnvelope) =>
			host_identity.Get.pipe(
				Effect.flatMap((snapshot) =>
					Envelope(query, "host.identity.query.result", snapshot),
				),
			),
	};

	return (
		query: HostIdentityQueryEnvelope,
	): Effect.Effect<HostIdentityQueryResultEnvelope, ProtocolErrorDetail> => {
		switch (query.kind) {
			case "host.identity.query":
				return handlers["host.identity.query"](query);
		}
	};
});
