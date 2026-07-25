# ChatGPT model discovery required a client version

## What happened

ChatGPT Subscription sign-in succeeded, but **Fetch Models** failed with HTTP
400. The Codex model catalog reported that the `client_version` query parameter
was missing, so the model picker could not discover GPT-5.6 or any other models
available to the account.

## Root cause

Con owns model discovery because Rig's ChatGPT provider does not expose a model
listing capability. Con called the correct Codex `/models` endpoint with the
correct OAuth token, but the endpoint contract evolved to require a client
compatibility version.

The value is also a capability boundary. Codex filters catalog entries by each
model's minimum client version; GPT-5.6 first appears at version `0.144.0`.
Using Con's package version would therefore satisfy the parameter syntactically
while still hiding compatible models.

## Fix applied

- Declare the Codex model-catalog compatibility level explicitly as `0.144.0`.
- Add `client_version` to ChatGPT model-list URLs while preserving unrelated
  query parameters and respecting a non-empty advanced override.
- Add GPT-5.6 Sol, Terra, and Luna to the curated offline fallback.
- Cover default URLs, custom query parameters, explicit overrides, empty
  overrides, and fallback ordering with unit tests.

## What we learned

Provider discovery endpoints are versioned protocols, even when they look like
simple REST lists. Their compatibility identifiers must describe the wire
contract Con supports, not Con's marketing version or the newest upstream
number. Advancing this value should be an explicit, tested decision so newer
catalog entries are only exposed after their request and response behavior is
known to work.
