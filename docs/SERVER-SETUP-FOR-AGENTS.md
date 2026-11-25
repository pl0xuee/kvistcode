# Setting up a Kvistcode server: instructions for AI agents

You are a coding agent asked to install `kvistcode-server` on a Linux machine for the person you work with. Follow these steps in order. The person-facing guide is [SERVER-SETUP.md](SERVER-SETUP.md) and the reference is [HEADLESS.md](HEADLESS.md); read them if something here doesn't cover your case.

## Ground rules

- **Run everything as the person's normal user, never as root.** Agents started by the server run commands as that user. If you are root, stop and ask the person which user to set it up for.
- **Ask before any `sudo`.** Steps that may need it are marked. Everything else works without it.
- **Never print, log or paste secrets.** That covers provider tokens, `~/.local/share/kvistcode-server/secrets.json` and `remote.json`. The pairing link is a one-time secret: give it to the person and don't put it anywhere else (files, commits, issue trackers).
- **Don't open the server to the internet.** No `--lan` on a machine with a public IP, no port forwarding, no firewall changes, no reverse proxy, unless the person explicitly asks and understands that a paired browser can run commands as their user.
- **Don't sign in for the person.** Provider logins (`claude` → `/login`, `codex login`) belong to them. Start the login if they ask, and let them complete it in their browser.
- Don't edit files in the data folder (`~/.local/share/kvistcode-server`) by hand.

## 1. Check the machine

```sh
uname -sm                       # needs Linux, x86_64 or aarch64
ldd --version | head -n1        # needs glibc 2.35 or newer
id -u                           # must not be 0
systemctl --user show-environment >/dev/null && echo "systemd user session: yes"
command -v tailscale && tailscale status >/dev/null 2>&1 && echo "tailscale: up"
```

- Not Linux, another architecture, or an older glibc: tell the person there is no ready-made build, and offer to build from source (HEADLESS.md → "Build from source").
- No systemd user session (some containers, WSL without systemd): continue with `--no-service` in step 2, and tell the person they must start it themselves.

## 2. Choose how it is reached, then install

Ask the person how they want to open it, unless they already said:

| They want | Flag | Notes |
| --- | --- | --- |
| Access from anywhere | `--tailscale` | Best choice. Tailscale must be installed and signed in on the server (ask before `sudo` to install it) and on their device. |
| Home network only | `--lan` | Plain HTTP. Only on a trusted home network, never on a VPS. |
| This machine / SSH tunnel | `--local` | Default when Tailscale isn't running. |

Install (the same command updates an existing install):

```sh
curl -fsSL https://github.com/pl0xuee/kvistcode/releases/latest/download/kvistcode-server-install.sh | sh -s -- --tailscale
```

Replace `--tailscale` with the chosen flag. The script:

- verifies the download;
- installs to `~/.local/bin/kvistcode-server`;
- writes `~/.config/systemd/user/kvistcode-server.service`;
- enables and starts the service;
- tries `loginctl enable-linger`;
- prints a pairing link.

If it prints "run 'sudo loginctl enable-linger …'", ask the person before running that. Without it the server stops when they log out.

## 3. Verify

```sh
~/.local/bin/kvistcode-server --version
systemctl --user is-active kvistcode-server      # expect: active
journalctl --user -u kvistcode-server -n 30 --no-pager
```

The journal should show `starting kvistcode-server`, followed by an address. If the service isn't active:

| The journal says | Do this |
| --- | --- |
| Port in use | Pick another port: add `--port <n>` to `ExecStart=` in the unit, then `systemctl --user daemon-reload && systemctl --user restart kvistcode-server`. |
| Tailscale problem | Make sure `tailscale status` works. It may need `sudo tailscale up` (ask the person first). |
| "belongs to another user" or "can't be made private" | The data folder has the wrong owner or permissions. Tell the person; don't chmod other users' files. |

## 4. Pair

```sh
~/.local/bin/kvistcode-server --pair
```

Give the person the link it prints, and the QR code if they're looking at your terminal.

- The link works once and expires after 5 minutes.
- If they're slow, run `--pair` again.
- With `--local`, also give them the tunnel command from the installer output (`ssh -L 47821:127.0.0.1:47821 user@host`). They open the tunnel first, then the link.

## 5. Agent tools and logins

Find out which providers the person uses. For each one that isn't installed, offer to install it:

```sh
# Claude Code
curl -fsSL https://claude.ai/install.sh | bash
# Codex (needs Node.js)
npm install -g @openai/codex
```

The person then signs in with `claude` → `/login`, or `codex login`. These print a URL for them to open. Don't complete the login yourself.

From then on, Kvistcode keeps its own copies of these tools up to date.

## 6. Hand over

Tell the person, briefly:

- how to open it: the link, the tunnel if any, and that "Add to Home Screen" works on phones;
- that updates install by themselves when no agent is working, and that `kvistcode-server --update` or Settings → Update does it now;
- the everyday commands: `kvistcode-server --pair`, `kvistcode-server --unpair-all` (lost device), `journalctl --user -u kvistcode-server -e` (logs), `systemctl --user restart kvistcode-server`;
- that the guide is docs/SERVER-SETUP.md.

## Facts you may need

- Data folder: `~/.local/share/kvistcode-server`, mode 0700. Back up the whole folder to move the server.
- Default port: 47821, kept in the data folder.
- Only one server can run per data folder; a second one refuses to start.
- `kvistcode-server --help` lists every flag.
- `--no-tool-updates` turns off tool installs, sign-in checks and self-updates. It is meant for tests; don't use it for a normal install.
- `--no-auto-update` keeps update notices but doesn't install updates by itself.
- Uninstall: run the install command with `--uninstall`. The data folder is kept.
