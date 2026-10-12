# Simple Icons marks — license attestation

Scope: `modules/assets/svg/simple-icons/*` (bitbucket, codeberg, gitea,
sourcehut).

Evidence: each legacy source component
(`modules/frontend/src/lib/vcs/marks/<host>.svelte`) carries the doc comment
"vendored from Simple Icons (CC0); fills follow `currentColor`". The four files
are path-only currentColor marks extracted verbatim from those components.

The vendored artwork originates from the Simple Icons project
(https://simpleicons.org), which dedicates its icon paths under CC0 1.0
(Universal). The full dedication text is checked in as
`licenses/simple-icons-CC0.txt` (verbatim legalcode). CC0 explicitly does not
waive trademark rights (section 4a); host names remain the trademarks of their
owners and are used here to identify the corresponding repository hosts, exactly
as in the legacy frontend.
