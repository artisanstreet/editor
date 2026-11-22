# Security policy

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's security advisories:
open [Report a vulnerability](https://github.com/artisanstreet/editor/security/advisories/new)
on this repository's Security tab. Do not open a public issue, pull request or
discussion for a suspected vulnerability.

Include what you can of:

- the affected program (Artisan Editor, Forge or `ae`), its version (`ae --version`,
  or Settings → About in the Editor) and the operating system;
- the steps or input that reproduce the problem, and what an attacker gains;
- any proof of concept, logs or crash output, with secrets removed.

We discuss the report with you in the private advisory, credit you in the
published advisory unless you ask us not to, and ask that you keep the details
private until a fix is released or 90 days have passed, whichever comes first.

## Supported versions

Only the newest builds receive security fixes:

| Channel | Supported |
| --- | --- |
| Latest stable release (`vMAJOR.MINOR.PATCH`) | Yes |
| Latest nightly | Yes |
| Older releases and nightlies | No; update with `ae update` |

Fixes ship in the next nightly and, for stable users, in a new patch release.

## Response targets

| Step | Target |
| --- | --- |
| Acknowledge the report | 3 business days |
| Confirm or reject it, with a severity | 7 days |
| Release a fix for a critical issue | 7 days after confirmation |
| Release a fix for a high-severity issue | 30 days after confirmation |
| Release a fix for a medium- or low-severity issue | 90 days after confirmation |

When a fix is released we publish the GitHub security advisory, with a CVE
where one applies.

## Scope

In scope: the Artisan Editor, Forge, `ae`, the installer and the release
signing and update path in this repository.

Out of scope: vulnerabilities in the coding engines Forge runs (Claude Code,
Codex, OpenCode) or in third-party services; report those to their vendors.
Known vulnerabilities in Rust dependencies are tracked by the `cargo deny`
advisories check in CI; report one here only if it is exploitable through
Artisan.
