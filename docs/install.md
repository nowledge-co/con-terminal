# Install con

con is in beta. macOS is the primary supported platform today. Windows and Linux
builds are available as previews.

Every installer includes the app and `con-cli`. The CLI is used by scripts,
test runners, and external agent orchestrators to talk to a running con session.

Use the official installer when you want the newest beta as soon as it ships.
Community packages are convenient if they already fit your system workflow, but
they may trail the GitHub release for a short time.

## macOS with Homebrew

```sh
brew install --cask nowledge-co/tap/con-beta
```

This installs con and adds `con-cli` to your PATH for automation.

## macOS direct install

```sh
curl -fsSL https://con-releases.nowledge.co/install.sh | sh
```

This installs con into `/Applications` and links `con-cli` into `~/.local/bin`.
You can also download the DMG from
[Releases](https://github.com/nowledge-co/con-terminal/releases).

## Windows preview

### Official installer

```powershell
irm https://con-releases.nowledge.co/install.ps1 | iex
```

This installs `con-app.exe` and `con-cli.exe` into the same PATH directory.

### Scoop

Scoop users can install Con from the community-maintained `jam` bucket:

```powershell
scoop bucket add jam https://github.com/EFLKumo/jam
scoop install jam/con-terminal
```

The Scoop manifest is maintained by
[`EFLKumo`](https://github.com/EFLKumo) in
[`EFLKumo/jam`](https://github.com/EFLKumo/jam). It installs Con as a portable
app and adds both `con-app` and `con-cli` to your PATH.

Windows is still early. Follow the
[Windows tracker](https://github.com/nowledge-co/con-terminal/issues/34) for
current limits and fixes.

## Linux preview

### Official installer

```sh
curl -fsSL https://con-releases.nowledge.co/install.sh | sh
```

This installs `con` and `con-cli` into `~/.local/bin`, registers the desktop
launcher, and refreshes the app icon. You can also download the Linux tarball
from the latest
[Release](https://github.com/nowledge-co/con-terminal/releases).

### Flatpak (Official Repository)

You can install con from the official self-hosted Flatpak repository:

```sh
# Add the remote (with signature verification for Flatpak >= 1.17)
flatpak remote-add --user \
  --signature-lookaside=https://con-releases.nowledge.co/flatpak/sigs \
  con-terminal https://con-releases.nowledge.co/flatpak/con-terminal.flatpakrepo

# Install con terminal
flatpak install --user con-terminal co.nowledge.con
```

For older Flatpak clients (< 1.17):

```sh
flatpak remote-add --if-not-exists --user --no-gpg-verify \
  con-terminal oci+https://con-releases.nowledge.co/flatpak
flatpak install --user con-terminal co.nowledge.con
```

### Arch Linux / AUR

Arch users can install the community AUR package:

```sh
yay -S con-bin
```

`paru -S con-bin` works as well. The AUR package is maintained by
[`czyt`](https://aur.archlinux.org/account/czyt), and the package page is
[`con-bin`](https://aur.archlinux.org/packages/con-bin).

Linux is in preview. Follow the
[Linux tracker](https://github.com/nowledge-co/con-terminal/issues/18) for
current limits and fixes.

## Shell integration

con embeds Ghostty's shell-integration scripts and tries to auto-inject them
into the shells it spawns. Most users will never notice: Ghostty handles this
through `ZDOTDIR` for zsh, `XDG_DATA_DIRS` for fish, and `--rcfile` for bash.

Auto-injection can be skipped, though, when something else owns the shell
startup path: tmux, `exec zsh`, login-shell mode, a framework that resets
`ZDOTDIR`, and similar setups. When that happens, con still works as a
terminal, but it loses shell metadata: new panes may open at `$HOME`, the
sidebar may miss the foreground process or cwd, and AI tab labels have less
context.

You can check whether integration loaded in a fresh con tab:

- zsh: `echo $precmd_functions | grep _ghostty_precmd`
- bash: `declare -F | grep __ghostty_precmd`
- fish: `functions -a | grep __ghostty_mark_prompt_start`

If the command prints nothing, source the integration script explicitly. Add
the line for your shell to its startup file, save it, and open a new tab:

**zsh** (`~/.zshrc`):

```zsh
[[ -n $GHOSTTY_RESOURCES_DIR ]] && \
  source "$GHOSTTY_RESOURCES_DIR/shell-integration/zsh/ghostty-integration"
```

**bash** (`~/.bashrc`):

```bash
if [[ -n "$GHOSTTY_RESOURCES_DIR" ]]; then
  builtin source "$GHOSTTY_RESOURCES_DIR/shell-integration/bash/ghostty.bash"
fi
```

If you run bash as a login shell, put the same snippet in `~/.bash_profile` or
`~/.profile`, or make that file source `~/.bashrc`. Login bash does not always
read `~/.bashrc` on its own.

**fish** (`~/.config/fish/config.fish`):

```fish
if set -q GHOSTTY_RESOURCES_DIR; and string length -q -- "$GHOSTTY_RESOURCES_DIR"
    source "$GHOSTTY_RESOURCES_DIR/shell-integration/fish/vendor_conf.d/ghostty-shell-integration.fish"
end
```

`$GHOSTTY_RESOURCES_DIR` resolves correctly under both the installed app
bundle and a `cargo run` debug build, so the same line works in either.

## Build from source

If you want to build or change con itself, use the
[contributor quickstart](https://github.com/nowledge-co/con-terminal/blob/main/HACKING.md)
in the source repo.
