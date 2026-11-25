# Set up a Kvistcode server

Run your agents on a machine that is always on (a home server, a VPS, a spare laptop, a Raspberry Pi) and use them from the browser on your phone or computer. This takes about five minutes.

Rather have an AI agent do it? Skip to [Let an agent set it up](#let-an-agent-set-it-up).

## What you need

- A Linux machine, x86_64 or ARM64, with Ubuntu 22.04, Debian 12, Fedora 36 or newer. You need a normal user account on it, not root, and SSH or a terminal on it.
- An account with at least one agent provider, such as a Claude or ChatGPT subscription or an API key.
- To reach it from outside your home, **Tailscale** (free for personal use) is the easy way. Install it on the server and on your phone or computer, and sign in to the same account on both. It's optional: without it you can use your home network or an SSH tunnel.

## 1. Install

On the server, as your normal user, run:

```sh
curl -fsSL https://github.com/pl0xuee/kvistcode/releases/latest/download/kvistcode-server-install.sh | sh
```

This one command:

1. downloads the server for your machine and checks the download;
2. puts it in `~/.local/bin`, so it can update itself (no `sudo` needed);
3. starts it, and starts it again on every boot;
4. prints a **pairing link** and a QR code.

If Tailscale is running, the server is published to your tailnet over HTTPS. Otherwise it only listens on the server itself, and the installer tells you how to reach it. To choose, add an option at the end:

```sh
curl -fsSL https://github.com/pl0xuee/kvistcode/releases/latest/download/kvistcode-server-install.sh | sh -s -- --tailscale
```

| Option | Reach it from |
| --- | --- |
| `--tailscale` | Your devices on your tailnet, anywhere, over HTTPS. Recommended. |
| `--lan` | Devices on the same home network (plain HTTP: only on a network you trust). |
| `--local` | The server only. From your computer use `ssh -L 47821:127.0.0.1:47821 you@server`, then open the link. |

## 2. Pair your browser

Open the pairing link on the phone or computer you want to use, or scan the QR code. It works once and expires after 5 minutes.

Need a new link, for another device or because it expired? Run this on the server:

```sh
kvistcode-server --pair
```

On a phone, use "Add to Home Screen" for an app-like icon.

## 3. Sign in to your agents

The server has no window, so install and sign in to the agent tools you use once on the server, over SSH. For example:

```sh
# Claude Code
curl -fsSL https://claude.ai/install.sh | bash
claude          # type /login and follow the link

# Codex (ChatGPT); needs Node.js
npm install -g @openai/codex
codex login
```

After that, Kvistcode keeps its own up-to-date copy of each tool you use, and agents pick up your login the next time they start.

That's it. Start an agent from the browser with **New**.

## Everyday use

| To | Do |
| --- | --- |
| Pair another device | `kvistcode-server --pair` |
| Sign out every device (lost phone) | `kvistcode-server --unpair-all`, then pair the ones you keep again |
| Update now | `kvistcode-server --update`, or **Settings → Update** in the browser |
| See what it is doing | `journalctl --user -u kvistcode-server -e` |
| Restart it | `systemctl --user restart kvistcode-server` |
| Stop it | `systemctl --user stop kvistcode-server` (and `disable` to keep it off after a reboot) |
| Change how it is reached | Run the install command again with `--tailscale`, `--lan` or `--local` |
| Uninstall | Run the install command with `--uninstall` (your agents and history are kept in `~/.local/share/kvistcode-server`) |

## Updates

Updates install by themselves. The server checks every few hours and, when a new version is out, installs it as soon as no agent is working, then restarts. Your agents, history and paired devices are kept, and the browser reconnects by itself. The browser also shows **Update to x.y.z** in the title bar and in Settings, if you'd rather do it right away.

Every update is signed, and the server refuses one whose signature doesn't match.

Don't want automatic updates? Add `--no-auto-update` to the `ExecStart=` line in `~/.config/systemd/user/kvistcode-server.service`, then run `systemctl --user daemon-reload && systemctl --user restart kvistcode-server`. You'll still see when one is out.

## If something goes wrong

- **"no kvistcode-server is running"** from `--pair`: it isn't started yet, or it failed. Check `journalctl --user -u kvistcode-server -e`.
- **The link doesn't open.**
  - With `--local`, open the SSH tunnel first.
  - With Tailscale, make sure your device is signed in to the same tailnet.
  - The link may have expired: get a new one with `--pair`.
- **It stops when I log out.** Run `sudo loginctl enable-linger $USER` once.
- **"can't update itself"**: it was installed somewhere your user can't write, like `/usr/local/bin`. Run the install command again; it moves it to `~/.local/bin`.
- **An agent says it isn't signed in, or the provider is missing.** Install and sign in on the server as in step 3, then start the agent again.

More detail, including security notes, where data lives and building from source: [HEADLESS.md](HEADLESS.md).

## Let an agent set it up

Already use Claude Code, Codex or another coding agent on the server, or one that can SSH into it? Paste this into the agent:

> Set up the Kvistcode server on this machine by following https://github.com/pl0xuee/kvistcode/blob/main/docs/SERVER-SETUP-FOR-AGENTS.md. Ask me before anything that needs sudo, and give me the pairing link at the end.

The agent can install, start, check and pair the server for you. You still do two things yourself:

- open the pairing link;
- sign in to the agent tools (the logins open in your browser).
