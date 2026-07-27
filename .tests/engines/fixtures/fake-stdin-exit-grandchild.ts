import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";

const grandchild = spawn(process.execPath, ["-e", "setInterval(() => undefined, 1000)"], {
	stdio: "ignore",
	windowsHide: true,
});

if (process.env.FAKE_GRANDCHILD_PID_FILE) {
	writeFileSync(process.env.FAKE_GRANDCHILD_PID_FILE, String(grandchild.pid));
}

process.stdin.on("end", () => {
	process.exit(0);
});

setInterval(() => undefined, 1_000);
