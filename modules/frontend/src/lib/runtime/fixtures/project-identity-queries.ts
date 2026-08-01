import { Effect } from "effect";
import { ArtisanClient } from "@artisan/transport/client";

import {
	fixture_engine_usage_session_reset_at,
	fixture_project,
	fixture_project_head_committed_at,
	fixture_timestamp,
} from "./support";

/** Project presentation and host/provider identity fixture projections. */
export const FixtureProjectIdentityQueries = {
	GetProjectRepositories: () =>
		Effect.gen(function* () {
			return {
				repositories: [
					{
						project_id: fixture_project.project_id,
						repository: {
							branch: { name: "master", type: "attached" as const },
							default_remote: "origin",
							head: "0f1e2d3c4b5a69788796a5b4c3d2e1f00f1e2d3c",
							remotes: [
								{
									host: "github" as const,
									name: "origin",
									url: "git@github.com:sandersonstabo/artisan-editor.git",
									web_url: "https://github.com/sandersonstabo/artisan-editor",
								},
							],
							state: "repository" as const,
						},
					},
				],
			};
		}),
	GetProjectDiffs: () =>
		Effect.gen(function* () {
			return {
				diffs: [
					{
						diff: {
							comparisons: [
								{
									ahead: 3,
									behind: 1,
									counts: {
										binary_file_count: 0,
										file_count: 12,
										lines_added: 486,
										lines_deleted: 121,
									},
									kind: "upstream" as const,
									ref: "origin/feature",
								},
								{
									ahead: 9,
									behind: 4,
									counts: {
										binary_file_count: 1,
										file_count: 34,
										lines_added: 1_204,
										lines_deleted: 388,
									},
									kind: "default_branch" as const,
									ref: "origin/master",
								},
							],
							head_committed_at: fixture_project_head_committed_at,
							staged: {
								binary_file_count: 0,
								file_count: 3,
								lines_added: 96,
								lines_deleted: 12,
							},
							state: "repository" as const,
							stash_count: 1,
							truncated: false,
							unstaged: {
								binary_file_count: 0,
								file_count: 4,
								lines_added: 118,
								lines_deleted: 26,
							},
							untracked_file_count: 2,
							working: {
								binary_file_count: 0,
								file_count: 7,
								lines_added: 214,
								lines_deleted: 38,
							},
						},
						project_id: fixture_project.project_id,
					},
				],
			};
		}),
	GetHostIdentity: Effect.gen(function* () {
		return {
			display_name: "Sander Sonstabo",
			hostname: "DESKTOP-FIXTURE",
			platform: "win32" as const,
			username: "sander",
		};
	}),
	GetEngineUsage: (input) =>
		Effect.gen(function* () {
			return {
				engines: [
					{
						authentication: "authenticated" as const,
						display_name: "Claude",
						engine_id: "claude",
						windows: [
							{
								id: "session",
								kind: "session" as const,
								percent_used: 17,
								resets_at: fixture_engine_usage_session_reset_at,
							},
							{
								id: "claude_weekly",
								kind: "weekly" as const,
								percent_used: 3,
							},
							{
								id: "claude_weekly_fable",
								kind: "weekly" as const,
								label: "Fable",
								percent_used: 5,
							},
						],
					},
					{
						authentication: "authenticated" as const,
						display_name: "Codex",
						engine_id: "codex",
						windows: [
							{
								id: "codex",
								kind: "weekly" as const,
								percent_used: 12,
							},
							{
								id: "codex_weekly_gpt_5_3_spark",
								kind: "weekly" as const,
								label: "GPT-5.3-Codex-Spark",
								percent_used: 2,
							},
						],
					},
					{
						authentication: "unauthenticated" as const,
						display_name: "Grok",
						engine_id: "grok",
						windows: [],
					},
				].filter(
					(report) =>
						input?.engine_id === undefined || report.engine_id === input.engine_id,
				),
				fetched_at: fixture_timestamp,
			};
		}),
} satisfies Pick<
	typeof ArtisanClient.Service,
	"GetProjectRepositories" | "GetProjectDiffs" | "GetHostIdentity" | "GetEngineUsage"
>;
