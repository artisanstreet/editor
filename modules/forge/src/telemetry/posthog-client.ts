import { PostHog } from "posthog-node";

const approved_hosts = new Set(["https://eu.i.posthog.com", "https://us.i.posthog.com"]);
const default_host = "https://eu.i.posthog.com";

/** Prevents a runtime variable from turning analytics into arbitrary network egress. */
export const DecodePostHogHost = (input: string | undefined): string => {
  const host = input ?? default_host;
  if (!approved_hosts.has(host)) {
    throw new Error("PostHog host is not an approved ingestion origin");
  }
  return host;
};

/** Creates the only PostHog SDK instance in Artisan, with a bounded memory-only queue. */
export const MakePostHogClientFactory = (host: string) => {
  const approved_host = DecodePostHogHost(host);
  return (project_key: string) => {
    const client = new PostHog(project_key, {
      enableExceptionAutocapture: false,
      enableFullAiCapture: false,
      flushAt: 1,
      flushInterval: 0,
      host: approved_host,
      maxQueueSize: 1,
      persistence: "memory",
      privacyMode: true,
    });
    return {
      capture: (input: Parameters<PostHog["captureImmediate"]>[0]) =>
        client.captureImmediate(input),
      disable: () => client.disable(),
      options: client.options,
      shutdown: () => client.shutdown(),
    };
  };
};
