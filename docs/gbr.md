# Pair a phone with Build Remote Agent

con can use **Build Remote Agent** as a pairing device: the paid iOS/Android
app spectates (and can inject into) this desktop session through the free MIT
`gbr-agent`. Phone and PC never open ports to each other.

con stays in charge of panes, SSH, tmux, and the built-in agent. Do not drive
`con-cli` from the phone. Protocol `gbr/1`. Need agent **v0.6.0+**.

Website: https://grokbuildremote.com/
Agent: https://github.com/LinespottingOrg/GrokBuildRemote-Agents (MIT)

Independent product by Linespotting AB. Not affiliated with xAI or SpaceX.

## Install + pair

```bash
# macOS / Linux
curl -fsSL https://grokbuildremote.com/install.sh | bash
gbr-agent version          # must print v0.6.0 or newer
gbr-agent pair             # QR in browser + printed 8-char code
gbr-agent run              # leave running
```

```powershell
# Windows
irm https://grokbuildremote.com/install.ps1 | iex
gbr-agent version
gbr-agent pair
gbr-agent run
```

Phone: open Build Remote Agent → **Scan QR from computer** (or type the 8-char
code). **Unpair** in Settings before changing PCs. Force-close is not enough.

## Attach

After `gbr-agent run`:

| How | Where |
|-----|--------|
| Bot API | `http://127.0.0.1:8788` |
| MCP | stdio `gbr-mcp` |

```bash
curl -sS http://127.0.0.1:8788/health
curl -sS http://127.0.0.1:8788/v1/sessions
```

Phone is spectator + veto. See also [con-cli and surfaces](con-cli.md) for
external agents that should get a real terminal pane — that path is not gbr/1.

Do not commit mailbox keys. Phone **Settings → Bot API** is the only place the
relay key is copied.

Related skill (repo tree): `skills/gbr/SKILL.md`.
