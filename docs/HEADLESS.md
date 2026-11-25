# Running Kvistcode without a window

`kvistcode-server` is Kvistcode with no window. It runs your agents on a machine (a home server, a VPS, a spare laptop) and serves the same web app a phone uses with the desktop app. You open that app in any browser, on a phone or a desktop, and pair it the same way.

It has the agents, queue, schedules, triggers, budgets and the phone app's review and ship tools. It does not have the desktop shell: no tray, hotkeys, notifications, voice, or preview panes.

## Build

The server serves the phone app from inside the binary, so build the app first.

```sh
cd apps/remote && npm ci && npm run build && cd ../..
cargo build --release -p kvistcode-server    # needs cmake on PATH
```

The binary is `target/release/kvistcode-server`. Copy it anywhere on your PATH.

## Run

```sh
kvistcode-server
```

It prints the address, a one-time pairing link and a QR code, then keeps running until you press Ctrl+C or stop it (SIGTERM). Agents are stopped cleanly on exit, as when you quit the desktop app.

| Flag | What it does |
| --- | --- |
| `--data-dir <path>` | Where the server keeps its data. Default: `kvistcode-server` in your user data folder (`~/.local/share/kvistcode-server` on Linux). It is never the desktop app's folder. |
| `--port <n>` | Port to listen on. Default: the saved one, 47821 at first. |
| `--lan` | Also reachable from your private network (plain HTTP). |
| `--tailscale` | Reachable from your tailnet over HTTPS through Tailscale Serve. |
| `--pair` | Print a new pairing link for the server that is already running. |
| `--no-tool-updates` | Don't install agent tools, don't check sign-ins, don't run a login shell for PATH. For tests and sandboxes. |

With no `--lan` or `--tailscale` the server listens on `127.0.0.1` only. The flags are applied on every start; they are not remembered.

## Pairing

Open the link the server printed (or scan the QR code) in the browser you want to use. The link works once and expires after 5 minutes. The browser trades it for its own access token and stores it; the link is then useless.

You can pair as many browsers as you like. For a new one, run this on the server while it is running:

```sh
kvistcode-server --pair            # add --data-dir <path> if you used one
```

It asks the running server (over a private socket in the data folder, readable only by you) for a fresh link. Anyone who can run that command as your user can already read your data, so it adds no new exposure.

Revoking a browser: the phone app has no revoke button for a headless server. Stop the server, delete `remote.json` in the data folder, and start it again. All browsers have to pair again.

## Reaching it: LAN or Tailscale

- **Same network (home).** `--lan` listens on the machine's private address. Traffic is plain HTTP, so use it only on a network you trust.
- **Away from home: use Tailscale.** `--tailscale` keeps the server on loopback and publishes it to your tailnet over HTTPS with Tailscale Serve. Nothing is opened to the internet. Install and sign in to Tailscale on the server and on the device first.
- **SSH tunnel.** `ssh -L 47821:127.0.0.1:47821 you@server` and open `http://localhost:47821`. Use the same port on both sides: the server only accepts requests addressed to its own port.

Do not put the plain HTTP port on the public internet.

## Signing in to providers

The server has no window, so sign in on the box with each tool's own login, over SSH:

```sh
claude          # then /login
codex login
```

Each provider's "System" account uses that login, so agents pick it up on the next start. The phone app cannot add accounts, including API-key accounts: it has no screen or route for that. Add API-key accounts in the desktop app, or set the key in the tool's own config on the server.

Agent tools that Kvistcode installs itself are kept under the data folder and updated in the background (not with `--no-tool-updates`).

## Run it as a service (systemd user unit)

`~/.config/systemd/user/kvistcode-server.service`:

```ini
[Unit]
Description=Kvistcode server
After=network-online.target

[Service]
ExecStart=%h/.local/bin/kvistcode-server --tailscale
Restart=on-failure
TimeoutStopSec=30

[Install]
WantedBy=default.target
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now kvistcode-server
loginctl enable-linger "$USER"        # keep it running after you log out
journalctl --user -u kvistcode-server  # the pairing link is printed here at each start
```

To get a new link later: `kvistcode-server --pair`.

The agents run as your user with your files. A user unit is deliberate: don't run it as root.

## Where things live

All in the data folder (mode 0700):

| Path | What |
| --- | --- |
| `armada.db` | Agents, sessions and history (the file name is kept from the old app name). |
| `secrets.json` | Provider keys and tokens, one file, mode 0600, written atomically. Used because most servers have no OS keychain. |
| `remote.json` | Paired browsers (only a hash of each token), mode 0600. |
| `logs/` | Size-capped logs. Secrets are redacted. |
| `server.lock` | Held while the server runs; a second server on the same folder refuses to start. |
| `admin.sock` | The private socket `--pair` uses (mode 0600). |

Back up the whole folder to move the server. Set `KVISTCODE_LOG` (a tracing filter such as `debug`) for more logging.

## Security notes

- Only one server per data folder. Two servers would corrupt each other's database, so the lock is exclusive.
- Pairing links are single use, last 5 minutes and are never written to disk or logs. Device tokens are stored hashed.
- Requests must carry the server's own Host and Origin, which blocks DNS rebinding and other websites from driving it.
- `secrets.json` is plain JSON protected by file permissions only, like `~/.ssh`. Anyone who can read your home folder can read it. Use disk encryption on a machine you don't physically control.
- A paired browser can start agents that run commands and edit files as your user. Treat pairing like giving someone a shell.
