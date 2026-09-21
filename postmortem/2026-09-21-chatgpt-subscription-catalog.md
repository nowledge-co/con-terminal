# ChatGPT Subscription model discovery and reasoning settings

## What happened

Issue #375 identified a subscription fallback catalog missing Astra and still
offering retired GPT-5.4 models, a GPT-5.5 default approaching retirement, and no
reasoning-effort control. Fetch Models could use an expired access token even
when OAuth credentials were refreshable. Findings originated in code review;
no authenticated production failure was reproduced during that review.

## Root cause

The subscription catalog was represented as an in-memory list of strings, losing
model capabilities and account identity. Discovery read the token file directly
instead of using Rig's OAuth authentication path. Configuration exposed only
generic provider fields, so reasoning effort never reached Responses requests.

## Fix applied

- Default new subscription configurations to GPT-5.6 Sol and curate current
  fallback models. Preserve saved selections and explain dated retirements.
- Share noninteractive OAuth clients between chat and discovery, refreshing
  credentials before requesting models and sending the account header.
- Parse model capabilities and persist the last successful catalog per account
  and endpoint. Reject malformed/empty catalogs without replacing valid data.
- Add a typed reasoning-effort configuration and GPUI Select, validate known
  capabilities, and pass the setting through both agent and short-completion
  paths. Unknown catalog effort values are not offered by the current adapter.

## What we learned

Subscription model availability must remain separate from the OpenAI API catalog.
Rendering reasoning output does not imply support for configuring its effort.
Directory updates, authentication, and request capabilities need regression tests
together; changing a default model string or bumping a dependency alone is not
sufficient. Rig 0.42 migration and support for effort levels beyond `xhigh` remain
separate work. Read-only authenticated checks returned HTTP 200 for both catalog
versions: `0.144.0` omitted Astra while `0.155.0` included it for the same account.
The request now uses `0.155.0`, with a sanitized response fixture validating its
model/capability schema. No production generation or token refresh was performed
for these checks.

## CI follow-up

PR #377 failed its Linux and Windows UI checks because the former unscoped
`set_provider_models` method remained after subscription discovery moved to the
account-scoped catalog. Its only remaining caller was a unit test. Local checks
used `--tests`, which kept that caller alive and concealed the production
`dead_code` warning; CI promoted the warning to an error with `-D warnings`.

Removed the unused setter and its unreachable unscoped cache lookup, and updated
the regression test to exercise the endpoint-scoped setter used by settings.
Production binaries must also be checked with warnings denied, independently of
test-target checks.
