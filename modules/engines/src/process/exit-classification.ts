import type { EngineProcessExit } from "./process";

/**
 * Decides whether a child process was ended from outside rather than failing.
 *
 * A signal, or an exit with no code at all, means the process never chose to
 * stop: something killed it. That is a host restart, a shutdown, an OOM killer
 * or an operator — none of which say anything went wrong with the work, and
 * all of which leave a run that can be picked back up.
 *
 * Known limit, and it is the platform's rather than ours: Windows has no
 * signals. A process ended by `TerminateProcess` during shutdown surfaces as an
 * ordinary non-zero exit, indistinguishable here from a CLI that crashed. The
 * durable recovery path covers that case instead — a run still `running` when
 * Forge restarts is marked `interrupted` on the way back up, which is the
 * evidence this function cannot see.
 */
export const engine_exit_is_interruption = (exit: EngineProcessExit) =>
	exit.signal !== null || exit.code === null;
