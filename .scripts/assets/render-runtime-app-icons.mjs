import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { promisify } from "node:util";

const workspace_root = resolve(import.meta.dirname, "../..");
const asset_root = resolve(workspace_root, "modules/frontend/src/lib/assets/barekey");
const output_root = resolve(asset_root, "runtime-app-icons");
const Run = promisify(execFile);

const chrome_candidates = [
	process.env.ARTISAN_CHROME_PATH,
	process.env.ProgramFiles === undefined
		? undefined
		: resolve(process.env.ProgramFiles, "Google/Chrome/Application/chrome.exe"),
	process.env["ProgramFiles(x86)"] === undefined
		? undefined
		: resolve(process.env["ProgramFiles(x86)"], "Google/Chrome/Application/chrome.exe"),
].filter((candidate) => candidate !== undefined);
const chrome_path = chrome_candidates.find(existsSync);
if (chrome_path === undefined) {
	throw new Error("Set ARTISAN_CHROME_PATH to a Chromium executable to render app icons");
}

const DataUrl = async (name, media_type) => {
	const bytes = await readFile(resolve(asset_root, name));
	return `data:${media_type};base64,${bytes.toString("base64")}`;
};

const [gradient, jaw_shaded] = await Promise.all([
	DataUrl("logo-gradient.svg", "image/svg+xml"),
	DataUrl("artisan-street-jaw-shaded.png", "image/png"),
]);

const plastic_shadow = [
	"inset 0 6.5px 0 rgb(255 255 255 / 20%)",
	"inset 0 43.5px 78px -43.5px rgb(255 255 255 / 20%)",
	"inset 0 -43.5px 78px -43.5px rgb(0 0 0 / 38%)",
	"inset 0 -6.5px 0 rgb(0 0 0 / 28%)",
	"inset 0 0 0 2.2px rgb(255 255 255 / 20%)",
	"0 4.4px 8.7px rgb(0 0 0 / 22%)",
	"0 34.8px 69.5px -26px rgb(0 0 0 / 38%)",
	"0 95.6px 191px -78px rgb(0 0 0 / 48%)",
].join(",");

const DocumentFor = (variant) => {
	const plastic_jaw_symbol = `<img class="symbol" src="${jaw_shaded}" alt="" />`;
	const keyed_symbol = `
		<div class="symbol keyed"></div>
		<div class="symbol keyed tint"></div>
	`;
	const is_plastic_jaw = variant === "plastic-jaw-shading";
	const foreground = "oklch(0.9006 0.0045 285.9)";
	return `<!doctype html>
		<html>
			<head>
				<meta charset="utf-8" />
				<style>
					html, body {
						width: 1024px;
						height: 1024px;
						margin: 0;
						overflow: hidden;
						background: transparent;
					}
					body { display: grid; place-items: center; }
					.icon {
						position: relative;
						isolation: isolate;
						width: 95%;
						aspect-ratio: 1;
						overflow: hidden;
						border-radius: 22.5%;
						corner-shape: squircle;
						background: ${is_plastic_jaw ? `center / cover url("${gradient}")` : foreground};
						box-shadow: ${plastic_shadow};
					}
					.symbol {
						position: absolute;
						inset: 0;
						display: block;
						width: 100%;
						height: 100%;
						object-fit: cover;
						filter: drop-shadow(0 4.4px 4.4px rgb(0 0 0 / 12%));
					}
					.keyed {
						background: center / cover url("${gradient}");
						mask: center / cover no-repeat url("${jaw_shaded}");
						-webkit-mask: center / cover no-repeat url("${jaw_shaded}");
					}
					.tint { opacity: 0.45; }
				</style>
			</head>
			<body><div class="icon">${is_plastic_jaw ? plastic_jaw_symbol : keyed_symbol}</div></body>
		</html>`;
};

const Render = async (variant, output_name) => {
	const temporary_root = await mkdtemp(join(tmpdir(), "artisan-runtime-app-icon-"));
	const document_path = resolve(temporary_root, "icon.html");
	const output_path = resolve(output_root, output_name);
	await writeFile(document_path, DocumentFor(variant), "utf8");
	try {
		await Run(chrome_path, [
			"--headless=new",
			"--disable-extensions",
			"--disable-gpu",
			"--hide-scrollbars",
			"--no-first-run",
			"--default-background-color=00000000",
			`--user-data-dir=${resolve(temporary_root, "profile")}`,
			`--screenshot=${output_path}`,
			"--window-size=1024,1024",
			new URL(`file:///${document_path.replaceAll("\\", "/")}`).href,
		]);
	} finally {
		await rm(temporary_root, { force: true, recursive: true });
	}
};

await mkdir(output_root, { recursive: true });
await Render("plastic-jaw-shading", "plastic-jaw-shading.png");
await Render("foreground-gradient-symbol", "foreground-gradient-symbol.png");
for (const name of ["plastic-jaw-shading", "foreground-gradient-symbol"]) {
	await Run("python", [
		resolve(import.meta.dirname, "png-to-windows-icon.py"),
		resolve(output_root, `${name}.png`),
		resolve(output_root, `${name}.ico`),
	]);
}
