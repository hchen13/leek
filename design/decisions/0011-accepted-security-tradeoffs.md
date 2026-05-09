# 0011 — Accepted security tradeoffs (P1, single-user local)

**Status:** Accepted
**Date:** 2026-05-07

## Context

L.E.E.K runs as a local single-user daemon by default (`leek serve`,
`127.0.0.1:8080`). The threat model is "the user runs this on their own
machine, on their own network." It is **not** a multi-tenant SaaS.
Several deliberately-loose security defaults make sense in that model and
would be wrong in a hosted deployment. We document them here so future
me / a future contributor doesn't tighten them blindly and break local
research workflows.

## Decisions

### web_fetch: SSRF policy is intentionally lax

`crates/gateway/src/agent/tools/web_fetch.rs` allows private RFC1918 ranges
(`10/8`, `172.16/12`, `192.168/16`) and CGNAT (`100.64/10` minus the cloud
metadata sub-range). It blocks only the *known dangerous* slices:

- `localhost` / `127.0.0.0/8` and any single-label / `.local` / `.internal` /
  `.lan` hostnames (`check_hostname`)
- `169.254.x.x` link-local (cloud metadata)
- `100.100.100.200`-type Alibaba CGNAT metadata endpoints

**Why allow private ranges?** Self-hosted services (Obsidian Local REST API,
NAS dashboards, internal wikis) are legitimate research targets for this
user. Clash TUN fake-IP also lives in CGNAT. Blocking them would force
the user to copy-paste content into the chat for every internal source.

**Risk accepted:** model could be tricked into hitting a private endpoint
that should not be reachable. Mitigated by:

- single-user deployment — no privilege boundary to cross
- explicit metadata blocklist
- no DNS rebinding protection (TODO if/when we ship hosted)

If we ever ship a multi-tenant deployment, flip the policy to default-deny
and gate per-user override.

### auth/jwt.rs: signature verification is intentionally absent

`crates/gateway/src/auth/jwt.rs` only decodes the `exp` claim — it does **not**
verify the JWT signature, `iat`, `nbf`, `iss`, or `aud`. This is correct for
its use case: deciding whether a Codex OAuth token is *probably expired*
before sending it to OpenAI's backend. The backend itself enforces signature.

**Risk accepted:** if the Codex token is forged with a future `exp` we'll
treat it as valid and forward it; OpenAI will reject it on the actual
request. Net effect: one wasted request, not a security breach. Forging
the token requires write access to the vault sqlite, at which point the
attacker has more direct paths.

### prompt injection: tool output sentinel + harness rule, no detector

`crates/gateway/src/agent/mod.rs` wraps every `function_call_output` in
`<<LEEK_TOOL_OUTPUT call_id=...>> ... <</LEEK_TOOL_OUTPUT>>` and the system
prompt (`harness::TOOL_OUTPUT_HANDLING`) tells the model never to act on
imperatives inside those delimiters. We do **not** ship a regex detector
that flags `record_investment_action` substrings inside tool output.

**Why no detector?** Decision drafts already require user confirmation
in the UI before they hit `decisions`. The blast radius of a successful
injection is "annoying decision draft," which is bounded.

**Risk accepted:** model misuses tool output as instruction; the user sees
a draft they didn't ask for. Mitigated by: (a) sentinel + harness rule,
(b) decision contract requires opposing_case + risks + invalidation —
hard to inject all of them coherently, (c) UI confirmation gate.

### bearer-token redaction in error propagation

`crates/gateway/src/llm/codex_oauth.rs` redacts `Bearer ...` substrings
from upstream error bodies before propagating them into logs / SSE error
events. Today the OpenAI backend doesn't echo the access token in error
bodies — this is defense-in-depth in case it ever does, since SSE error
events end up in `vault.events` and would be replayable.

## Triggers to revisit

Re-open this ADR (with full review of each section) if any of:

- The deployment shifts from "local single-user" to multi-user / hosted.
- A new tool can act on tool output without UI confirmation (today only
  `record_investment_action` and `record_research_note` write to vault,
  and both produce confirmable artifacts).
- We add an LLM provider whose error bodies include credentials.
- We ship a setting that lets the user expose the gateway over LAN.
