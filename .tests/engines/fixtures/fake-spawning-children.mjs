import { spawn } from "node:child_process";
import { appendFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const child_path = fileURLToPath(new URL("./fake-grandchild.mjs", import.meta.url));
const pid_path = process.env.FAKE_CHILD_PID_FILE;

function spawn_child() {
	const child = spawn(process.execPath, [child_path], {
		stdio: "ignore",
		windowsHide: true,
	});

	if (pid_path && child.pid !== undefined) {
		appendFileSync(pid_path, `${String(child.pid)}\n`);
	}
}

spawn_child();
setInterval(spawn_child, 10);
