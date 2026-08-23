---
name: gbr
description: Pair a phone running Build Remote Agent to this con desktop session via gbr-agent (gbr/1). Attach 127.0.0.1:8788 or gbr-mcp. Spectator only — do not drive con-cli panes from the phone.
---

# Build Remote Agent — pairing device

One adapter. Protocol `gbr/1`. No fourth pair protocol.

Independent product by Linespotting AB. Not affiliated with xAI or SpaceX.

Requires `gbr-agent` ≥ 0.6.0 on the host. Loopback only. No mailbox keys in this file.

## Pair

```bash
curl -fsSL https://grokbuildremote.com/install.sh | bash
gbr-agent version
gbr-agent pair && gbr-agent run
```

Phone: [Build Remote Agent](https://grokbuildremote.com/) scans the QR or types the 8-char code. Unpair on the phone before a new mailbox.

## Attach (only these)

| How | Where |
|-----|--------|
| Bot API | `http://127.0.0.1:8788` after `gbr-agent run` |
| MCP | `gbr-mcp` stdio |

```bash
curl -sS http://127.0.0.1:8788/health
curl -sS http://127.0.0.1:8788/v1/sessions
```

Phone is spectator + veto. con's agent and `con-cli` stay on the desktop. See `docs/gbr.md`.
