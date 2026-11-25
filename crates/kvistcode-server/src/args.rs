//! Command line: a handful of flags, parsed by hand.

use std::path::PathBuf;

pub const HELP: &str = "\
kvistcode-server: Kvistcode without a window.

Runs your agents on this machine and serves the Kvistcode web app, which you open and
pair from any browser (the same app a phone uses with the desktop app).

USAGE
    kvistcode-server [OPTIONS]

OPTIONS
    --data-dir <path>   Where the server keeps its data (default: kvistcode-server in
                        your user data folder, separate from the desktop app's).
    --port <number>     Port to listen on (default: the saved one, 47821 at first).
    --lan               Listen on this machine's private network address, so other
                        devices on the same network can connect (plain HTTP).
    --tailscale         Listen on this machine only and publish it to your tailnet
                        with Tailscale Serve (HTTPS). Recommended away from home.
    --no-tool-updates   Don't install or update agent tools, don't check provider
                        sign-ins or usage, and don't ask a login shell for PATH at
                        startup. For tests and sandboxes (also KVISTCODE_NO_TOOL_UPDATES=1).
    --pair              Print a new pairing link for the server already running on
                        this data folder, and exit.
    --unpair-all        Sign out every browser and phone paired with the server
                        running on this data folder, and exit. Pair again with --pair.
    -h, --help          Show this help.
    -V, --version       Show the version.

By default the server listens on 127.0.0.1 only. --lan and --tailscale can't be combined.
";

#[derive(Debug, PartialEq, Default)]
pub struct Args {
    pub data_dir: Option<PathBuf>,
    pub port: Option<u16>,
    pub lan: bool,
    pub tailscale: bool,
    pub pair: bool,
    pub unpair_all: bool,
    pub no_tool_updates: bool,
    pub help: bool,
    pub version: bool,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut out = Args::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        // `--flag=value` as well as `--flag value`.
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg, None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| args.next())
                .filter(|v| !v.is_empty())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "--data-dir" => out.data_dir = Some(PathBuf::from(value("--data-dir")?)),
            "--port" => {
                let v = value("--port")?;
                out.port = Some(
                    v.parse::<u16>()
                        .ok()
                        .filter(|p| *p >= 1024)
                        .ok_or_else(|| format!("'{v}' isn't a port between 1024 and 65535"))?,
                );
            }
            "--lan" => out.lan = true,
            "--tailscale" => out.tailscale = true,
            "--pair" => out.pair = true,
            "--unpair-all" => out.unpair_all = true,
            "--no-tool-updates" => out.no_tool_updates = true,
            "-h" | "--help" => out.help = true,
            "-V" | "--version" => out.version = true,
            other => return Err(format!("unknown option '{other}' (see --help)")),
        }
    }
    if std::env::var_os("KVISTCODE_NO_TOOL_UPDATES").is_some_and(|v| v == "1") {
        out.no_tool_updates = true;
    }
    if out.pair && out.unpair_all {
        return Err("--pair and --unpair-all can't be combined; pick one".into());
    }
    if out.lan && out.tailscale {
        return Err("--lan and --tailscale can't be combined; pick one".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_flags() {
        let a = p(&["--data-dir", "/x", "--port=5000", "--lan", "--pair"]).unwrap();
        assert_eq!(a.data_dir, Some(PathBuf::from("/x")));
        assert_eq!(a.port, Some(5000));
        assert!(a.lan && a.pair && !a.tailscale);
        assert!(p(&["--unpair-all"]).unwrap().unpair_all);
    }

    #[test]
    fn rejects_bad_input() {
        assert!(p(&["--port"]).is_err());
        assert!(p(&["--port", "80"]).is_err());
        assert!(p(&["--port", "abc"]).is_err());
        assert!(p(&["--lan", "--tailscale"]).is_err());
        assert!(p(&["--nope"]).is_err());
        assert!(p(&["--pair", "--unpair-all"]).is_err());
    }
}
