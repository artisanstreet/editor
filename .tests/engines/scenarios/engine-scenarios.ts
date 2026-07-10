import type { EngineCommand, EngineOpenInput } from "@artisan/engines";

/** Defines stable start and resume inputs shared by engine conformance scenarios. @since 0.2.0 */
export const EngineOpenScenarios: Readonly<Record<"resume" | "start", EngineOpenInput>> = {
	resume: {
		_tag: "resume",
		artisan_run_id: "artisan-run-resume",
		model: "test-model",
		next_text: "Continue the existing task",
		permission_metadata: { filesystem: "read-write" },
		profile: "test-profile",
		resume_token: {
			native_thread_id: "native-thread-resume",
			opaque_checkpoint: "checkpoint-1",
		},
		working_directory: "C:\\workspace",
	},
	start: {
		_tag: "start",
		artisan_run_id: "artisan-run-start",
		initial_text: "Start the task",
		model: "test-model",
		permission_metadata: { filesystem: "read-write" },
		profile: "test-profile",
		working_directory: "C:\\workspace",
	},
};

/** Defines ordered commands that exercise every non-terminal command path. @since 0.2.0 */
export const EngineCommandScenarios = [
	{ _tag: "steer", command_id: "command-steer", text: "Use a smaller change" },
	{
		_tag: "respond_approval",
		approval_id: "approval-1",
		approved: true,
		command_id: "command-approval",
	},
	{
		_tag: "respond_question",
		command_id: "command-question",
		question_id: "question-1",
		text: "Use the current workspace",
	},
] as const satisfies ReadonlyArray<EngineCommand>;

/** Defines commands used to test terminal, duplicate, and unsupported delivery. @since 0.2.0 */
export const EngineTerminalCommandScenarios = {
	cancel: { _tag: "cancel", command_id: "command-cancel" } satisfies EngineCommand,
	close: { _tag: "close", command_id: "command-close" } satisfies EngineCommand,
	second_close: { _tag: "close", command_id: "command-close-second" } satisfies EngineCommand,
	unsupported_steer: {
		_tag: "steer",
		command_id: "command-unsupported",
		text: "This command is intentionally unsupported",
	} satisfies EngineCommand,
} as const;
