import { readFile, writeFile } from "node:fs/promises";

import { make_release_plan } from "./planner.ts";

const arguments_map = new Map<string, string>();
for (let index = 2; index < process.argv.length; index += 2) {
	const name = process.argv[index];
	const value = process.argv[index + 1];
	if (!name?.startsWith("--") || value === undefined)
		throw new Error("Expected --name value pairs.");
	arguments_map.set(name.slice(2), value);
}

const required = (name: string): string => {
	const value = arguments_map.get(name);
	if (!value) throw new Error(`Missing --${name}.`);
	return value;
};

const cargo_toml = await readFile("Cargo.toml", "utf8");
const workspace_version = cargo_toml.match(
	/^\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m,
)?.[1];
if (!workspace_version) throw new Error("Cargo.toml does not declare workspace.package.version.");

const plan = make_release_plan({
	event: required("event") as "pull_request" | "push" | "workflow_dispatch",
	ref: required("ref"),
	commit: required("commit"),
	version: required("version"),
	workspace_version,
	run_id: required("run-id"),
	...(arguments_map.has("mode")
		? { mode: required("mode") as "dry-run" | "release" | "resume" }
		: {}),
	...(arguments_map.has("resume-commit") ? { resume_commit: required("resume-commit") } : {}),
	...(arguments_map.has("resume-version") ? { resume_version: required("resume-version") } : {}),
	...(arguments_map.has("resume-run-id") ? { resume_run_id: required("resume-run-id") } : {}),
});
const output = `${JSON.stringify(plan, undefined, "\t")}\n`;
await writeFile(required("output"), output);

const github_output = arguments_map.get("github-output");
if (github_output) {
	const lines = [
		`version=${plan.version}`,
		`commit=${plan.commit}`,
		`source_run_id=${plan.run_id}`,
		`tag=${plan.tag}`,
		`candidate_artifact=${plan.candidate_artifact}`,
		`construct_candidate=${plan.construct_candidate}`,
		`publish=${plan.publish}`,
		`assets=${JSON.stringify(plan.assets)}`,
	];
	await writeFile(github_output, `${lines.join("\n")}\n`, { flag: "a" });
}
