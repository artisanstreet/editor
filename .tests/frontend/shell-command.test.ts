import { describe, expect, it } from "vitest";

import { PresentShellCommand } from "../../modules/frontend/src/lib/conversation/shell-command";

describe("shell command presentation", () => {
	/** The exact shape Codex reports on Windows, path and all. */
	it("recovers the command from an absolute PowerShell invocation", () => {
		expect(
			PresentShellCommand(
				'"C:\\Program Files\\WindowsApps\\Microsoft.PowerShell_7.6.4.0_x64__8wekyb3d8bbwe\\pwsh.exe" -Command "Get-Content -Raw \'A-SECOND-SIGNAL.md\'; git status --short"',
			),
		).toBe("Get-Content -Raw 'A-SECOND-SIGNAL.md'; git status --short");
	});

	it("unwraps a single-quoted body as readily as a double-quoted one", () => {
		expect(
			PresentShellCommand(
				"\"C:\\Program Files\\pwsh.exe\" -Command 'Get-ChildItem -Force | Select-Object Name, Length'",
			),
		).toBe("Get-ChildItem -Force | Select-Object Name, Length");
	});

	it("unwraps the POSIX shells and cmd", () => {
		expect(PresentShellCommand('bash -lc "pnpm vitest run"')).toBe("pnpm vitest run");
		expect(PresentShellCommand("/bin/sh -c 'git status --short'")).toBe("git status --short");
		expect(PresentShellCommand('cmd.exe /c "dir /b"')).toBe("dir /b");
	});

	/** A command that is already what it says stays exactly what it says. */
	it("leaves an unwrapped command alone", () => {
		expect(PresentShellCommand("git status --short")).toBe("git status --short");
	});

	it("keeps a shell invoked for something other than a command", () => {
		expect(PresentShellCommand("pwsh.exe -File ./script.ps1")).toBe(
			"pwsh.exe -File ./script.ps1",
		);
	});

	/** A row is one line, and the wrapper's own newlines are not part of the work. */
	it("collapses whitespace so a multi-line body reads as one line", () => {
		expect(PresentShellCommand('bash -lc "git add .\n  git commit -m one"')).toBe(
			"git add . git commit -m one",
		);
	});

	it("returns the invocation whole when the body is empty", () => {
		expect(PresentShellCommand('bash -lc ""')).toBe('bash -lc ""');
	});
});
