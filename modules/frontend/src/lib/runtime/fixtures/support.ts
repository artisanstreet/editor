import { Effect } from "effect";
import type { PreviewTargetGetQuery } from "@artisan/protocol";
import { ArtisanClientError } from "@artisan/transport/client";

import {
	FixtureConversation,
	fixture_artisan_client_data,
	fixture_engine_usage_session_reset_at,
	fixture_project,
	fixture_project_head_committed_at,
	fixture_timestamp,
} from "./data";

export {
	FixtureConversation,
	fixture_artisan_client_data,
	fixture_engine_usage_session_reset_at,
	fixture_project,
	fixture_project_head_committed_at,
	fixture_timestamp,
};

export const FixtureFailure = (message: string) =>
	Effect.gen(function* () {
		return yield* Effect.fail(
			new ArtisanClientError({
				cause: undefined,
				code: "protocol",
				message,
				protocol_code: "fixture_not_found",
				retryable: false,
			}),
		);
	});

export const FixtureReceipt = (command_id: string, journal_sequence = 48) =>
	Effect.gen(function* () {
		return yield* Effect.succeed({
			command_id,
			journal_sequence,
			status: "accepted" as const,
		});
	});

export const FixturePreviewTarget = (input: PreviewTargetGetQuery) =>
	Effect.gen(function* () {
		const target = fixture_artisan_client_data.preview_targets.find(
			(candidate) => candidate.id === input.target_id,
		);

		return target === undefined
			? yield* FixtureFailure(`Unknown fixture preview target: ${input.target_id}`)
			: target;
	});
