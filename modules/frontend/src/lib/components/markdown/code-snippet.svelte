<script lang="ts" effect>
	import Check from "@tabler/icons-svelte/icons/check";
	import Copy from "@tabler/icons-svelte/icons/copy";
	import { Effect } from "effect";
	import type { Snippet } from "svelte";
	import { WriteClipboardText } from "$lib/browser/clipboard";
	import { RunBrowserDom } from "$lib/browser/dom";
	import { resolve_file_icon } from "$lib/conversation/file-icon";
	import { cn } from "$lib/utils";

	type CopyState = "idle" | "success" | "failure";

	type Props = {
		children?: Snippet;
		class?: string;
		filename?: string;
		highlights?: ReadonlyArray<number>;
		language?: string;
		meta?: string;
	};

	let {
		children,
		class: class_name,
		filename,
		highlights: _highlights,
		language: _language,
		meta: _meta,
	}: Props = $props();

	let code_element = $state<HTMLPreElement>();
	let copy_attempt = $state(0);
	let copy_state = $state<CopyState>("idle");

	const CopyCode = Effect.gen(function* () {
		const attempt = copy_attempt + 1;
		copy_attempt = attempt;
		copy_state = "idle";

		const text = yield* RunBrowserDom(() => code_element?.textContent ?? "").pipe(
			Effect.catchTag("BrowserDomFailure", () =>
				Effect.gen(function* () {
					if (copy_attempt === attempt) copy_state = "failure";
					return "";
				}),
			),
		);

		if (text.length === 0) {
			if (copy_attempt === attempt) copy_state = "failure";
			return;
		}

		let write_failed = false;
		yield* WriteClipboardText(text).pipe(
			Effect.catchTag("ClipboardWriteError", () =>
				Effect.gen(function* () {
					write_failed = true;
					if (copy_attempt === attempt) copy_state = "failure";
				}),
			),
		);

		if (write_failed || copy_attempt !== attempt) return;

		copy_state = "success";
		yield* Effect.sleep("1400 millis");
		if (copy_attempt === attempt) copy_state = "idle";
	});

	const copy_label = $derived(copy_state === "success" ? "Code copied" : "Copy code");
	const copy_message = $derived(
		copy_state === "success"
			? "Code copied to clipboard."
			: copy_state === "failure"
				? "Couldn't copy code. Try again."
				: "",
	);
</script>

{#snippet copy_control(overlay)}
	<button
		class={cn(
			"docs-code-snippet-copy group/docs-copy flex items-center justify-center rounded-full bg-linear-to-b from-foreground/7.5 to-foreground/2.5 p-2 leading-none card-lg",
			overlay && "docs-code-snippet-copy-overlay",
		)}
		type="button"
		aria-label={copy_label}
		data-copy-state={copy_state}
		onclick={yield* CopyCode}
	>
		<span class="relative grid size-4 place-items-center" aria-hidden="true">
			<Copy
				class="col-start-1 row-start-1 size-4 text-muted-foreground transition-all duration-(--duration-fast) ease-in-out group-hover/docs-copy:text-foreground group-data-[copy-state=success]/docs-copy:scale-75 group-data-[copy-state=success]/docs-copy:opacity-0 motion-reduce:transition-none"
			/>
			<Check
				class="col-start-1 row-start-1 size-4 scale-75 text-muted-foreground opacity-0 transition-all duration-(--duration-fast) ease-in-out group-hover/docs-copy:text-foreground group-data-[copy-state=success]/docs-copy:scale-100 group-data-[copy-state=success]/docs-copy:opacity-100 motion-reduce:transition-none"
			/>
		</span>
	</button>
{/snippet}

<div class="docs-code-snippet not-prose">
	{#if filename === undefined}
		{@render copy_control(true)}
	{/if}

	<div class="docs-code-snippet-body">
		{#if filename !== undefined}
			<div class="docs-code-snippet-header">
				<span class="docs-code-snippet-filename">
					<img
						src={resolve_file_icon(filename)}
						alt=""
						aria-hidden="true"
						class="size-4 shrink-0"
					/>
					<span class="docs-source-location-name">{filename}</span>
				</span>
				{@render copy_control(false)}
			</div>
		{/if}
		<pre bind:this={code_element} class={class_name}>{@render children?.()}</pre>
	</div>
	<span class="sr-only" role="status" aria-live="polite">{copy_message}</span>
</div>
