import process from "node:process";

const decoder = new TextDecoder();
const keep_alive = setInterval(() => undefined, 1_000);

process.stdin.on("data", (chunk) => {
	const lines = decoder
		.decode(typeof chunk === "string" ? Buffer.from(chunk) : chunk, { stream: true })
		.split("\n")
		.filter(Boolean);

	for (const line of lines) {
		const instruction = JSON.parse(line);
		const chunks = instruction.chunks ?? [];

		for (const chunk_instruction of chunks) {
			setTimeout(() => {
				const target =
					chunk_instruction.stream === "stderr" ? process.stderr : process.stdout;

				target.write(Buffer.from(chunk_instruction.chunk_base64, "base64"));
			}, chunk_instruction.at_ms);
		}

		if (instruction.exit) {
			setTimeout(() => process.exit(instruction.exit.code), instruction.exit.at_ms);
		}
	}
});

process.on("SIGTERM", () => {
	clearInterval(keep_alive);
	process.exit(143);
});
