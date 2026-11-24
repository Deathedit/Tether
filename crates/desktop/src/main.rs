//! `tether`: desktop entry point. With no subcommand it opens the Slint UI;
//! the subcommands are a command-line interface to the same logic.

mod app;
mod stream;
mod video;

use std::io::{BufRead, Write};
use std::net::{IpAddr, ToSocketAddrs};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rp_core::{AccountId, ConsoleState, DiscoveryOptions, RegistRequest, psn};
use tether_core::{
    Config, ConsoleSecrets, PairedConsole, Store, VideoCodec, VideoResolution, WakeProgress,
};
use tracing_subscriber::EnvFilter;

/// Where the pairing code is on the console.
const PAIR_SCREEN: &str = "Settings > System > Remote Play > Link Device";

#[derive(Parser)]
#[command(
    name = "tether",
    version,
    about = "Remote Play client. Works with PS5."
)]
struct Cli {
    /// More log output (-v: info, -vv: debug, -vvv: trace). RUST_LOG overrides this.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Without a subcommand, the app window opens.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// List consoles on the local network
    Discover {
        /// How long to listen for answers, in seconds
        #[arg(short, long, default_value_t = 2.0)]
        timeout: f64,

        /// Ask only this console (IP address or host name) instead of broadcasting
        #[arg(long)]
        host: Option<String>,
    },
    /// Sign in to PSN once to get the account ID that pairing needs
    Login {
        /// Set the account ID directly (base64, e.g. from chiaki-ng) instead of signing in
        #[arg(long)]
        account_id: Option<String>,
    },
    /// Pair with a console using the 8-digit code it shows
    Pair {
        /// IP address or host name of the console
        console: String,

        /// The 8-digit code from the console (asked for if omitted)
        #[arg(long)]
        pin: Option<String>,

        /// PSN account ID (base64) to use instead of the one from `tether login`
        #[arg(long)]
        account_id: Option<String>,
    },
    /// List paired consoles
    Consoles,
    /// Forget a paired console and its keys
    Unpair {
        /// Name, address or ID of the paired console
        console: String,
    },
    /// Wake a console from rest mode
    Wake {
        /// Name, address or ID of a paired console, or any address with --regist-key
        console: String,

        /// Registration key (up to 8 hex digits) for a console Tether hasn't paired with
        #[arg(short = 'k', long)]
        regist_key: Option<String>,

        /// With --regist-key: the console is a PS4 (uses the PS4 wake port)
        #[arg(long, requires = "regist_key")]
        ps4: bool,

        /// Paired consoles only: keep waking until the console reports ready
        #[arg(short, long, conflicts_with = "regist_key")]
        wait: bool,
    },
    /// Open the app and stream a paired console (waking it first if
    /// needed); quits when the session ends. Unset options come from the
    /// app's settings.
    Stream {
        /// Name, address or ID of the paired console
        console: String,

        /// Video resolution
        #[arg(short, long, value_enum)]
        resolution: Option<ResolutionArg>,

        /// Frames per second (30 or 60)
        #[arg(long, value_parser = parse_fps)]
        fps: Option<u32>,

        /// Video codec
        #[arg(long, value_enum)]
        codec: Option<CodecArg>,
    },
}

fn parse_fps(s: &str) -> Result<u32, String> {
    match s {
        "30" => Ok(30),
        "60" => Ok(60),
        _ => Err("must be 30 or 60".into()),
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ResolutionArg {
    #[value(name = "360p")]
    P360,
    #[value(name = "540p")]
    P540,
    #[value(name = "720p")]
    P720,
    #[value(name = "1080p")]
    P1080,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum CodecArg {
    H264,
    H265,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    match run(cli.command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn init_logging(verbose: u8) {
    let default = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}

fn run(command: Option<Command>) -> Result<()> {
    let Some(command) = command else {
        return app::run(app::Options::default());
    };
    match command {
        Command::Discover { timeout, host } => discover(timeout, host),
        Command::Login { account_id } => login(account_id),
        Command::Pair {
            console,
            pin,
            account_id,
        } => pair(&console, pin, account_id),
        Command::Consoles => consoles(),
        Command::Unpair { console } => {
            let mut store = Store::open_default()?;
            match store.remove(&console)? {
                Some(c) => println!("Forgot {} ({}).", c.name, c.host_id),
                None => bail!("no paired console matches {console:?}"),
            }
            Ok(())
        }
        Command::Wake {
            console,
            regist_key,
            ps4,
            wait,
        } => wake(&console, regist_key, ps4, wait),
        Command::Stream {
            console,
            resolution,
            fps,
            codec,
        } => app::run(app::Options {
            stream: Some(console),
            overrides: app::Overrides {
                resolution: resolution.map(|r| match r {
                    ResolutionArg::P360 => VideoResolution::P360,
                    ResolutionArg::P540 => VideoResolution::P540,
                    ResolutionArg::P720 => VideoResolution::P720,
                    ResolutionArg::P1080 => VideoResolution::P1080,
                }),
                fps,
                codec: codec.map(|c| match c {
                    CodecArg::H264 => VideoCodec::H264,
                    CodecArg::H265 => VideoCodec::H265,
                }),
            },
        }),
    }
}

fn discover(timeout: f64, host: Option<String>) -> Result<()> {
    if !(timeout.is_finite() && timeout > 0.0) {
        bail!("--timeout must be a positive number of seconds");
    }
    let options = DiscoveryOptions {
        host: host.as_deref().map(resolve).transpose()?,
        ..Default::default()
    };
    let found = rp_core::discover(Duration::from_secs_f64(timeout), &options)?;
    if found.is_empty() {
        eprintln!("No consoles found.");
        return Ok(());
    }
    // Only the config file is needed here; skip the credential store.
    let config = Config::load(&Store::default_dir()?.join("config.toml")).unwrap_or_default();
    let rows = found
        .iter()
        .map(|c| {
            let paired = config
                .consoles
                .iter()
                .any(|p| p.host_id.eq_ignore_ascii_case(&c.host_id));
            vec![
                c.name.clone(),
                c.addr.to_string(),
                c.state.to_string(),
                if paired { "yes" } else { "no" }.into(),
            ]
        })
        .collect();
    print!("{}", table(&["NAME", "ADDRESS", "STATE", "PAIRED"], rows));
    Ok(())
}

fn login(account_id: Option<String>) -> Result<()> {
    let mut store = Store::open_default()?;
    let id: AccountId = match account_id {
        Some(s) => s.parse()?,
        None => {
            let url = psn::login_url();
            eprintln!(
                "1. Sign in to PSN in your browser. If no browser window opened, open this link:\n\n   {url}\n\n\
                 2. After signing in, the browser lands on a blank or error page. Copy that\n   \
                 page's full address from the address bar and paste it below.\n"
            );
            let _ = open::that_detached(&url);
            let input = prompt("Address: ")?;
            let code = psn::parse_redirect(&input)?;
            psn::fetch_account_id(&code)?
        }
    };
    store.set_account_id(id)?;
    println!("PSN account ID: {id}");
    println!("Saved to {}", store.config_path().display());
    Ok(())
}

fn pair(console: &str, pin: Option<String>, account_id: Option<String>) -> Result<()> {
    let mut store = Store::open_default()?;
    let account_id: AccountId = match account_id {
        Some(s) => {
            let id = s.parse()?;
            if store.account_id()?.is_none() {
                store.set_account_id(id)?;
            }
            id
        }
        None => store
            .account_id()?
            .context("no PSN account ID yet: run `tether login` first, or pass --account-id")?,
    };

    let ip = resolve(console)?;
    let options = DiscoveryOptions {
        host: Some(ip),
        ..Default::default()
    };
    let found = rp_core::discover(Duration::from_secs(3), &options)?
        .into_iter()
        .next()
        .with_context(|| {
            format!("no console answered at {ip}. Is it on and on the same network?")
        })?;
    if found.state == ConsoleState::Standby {
        bail!(
            "{} is in rest mode. Turn it on, then open {PAIR_SCREEN}.",
            found.name
        );
    }

    let pin = match pin {
        Some(p) => rp_core::parse_pin(&p)?,
        None => {
            eprintln!(
                "On {}, open {PAIR_SCREEN} and keep the code on screen.",
                found.name
            );
            rp_core::parse_pin(&prompt("8-digit code: ")?)?
        }
    };

    eprintln!("Pairing with {} at {ip}...", found.name);
    let registered = rp_core::register(&RegistRequest {
        host: ip.to_string(),
        target: found.target,
        account_id,
        pin,
    })?;
    let paired = store.add_paired(&registered, &found.name, &ip.to_string())?;
    println!("Paired with {} ({}).", paired.name, paired.host_id);
    println!("Keys saved to the {}.", store.secret_store().describe());
    Ok(())
}

fn consoles() -> Result<()> {
    let store = Store::open_default()?;
    if store.paired().is_empty() {
        eprintln!("No paired consoles. Pair one with `tether pair <address>`.");
        return Ok(());
    }
    let rows = store
        .paired()
        .iter()
        .map(|c| {
            vec![
                c.name.clone(),
                c.address.clone(),
                c.host_id.clone(),
                c.target().to_string(),
            ]
        })
        .collect();
    print!("{}", table(&["NAME", "ADDRESS", "ID", "TYPE"], rows));
    Ok(())
}

fn wake(console: &str, regist_key: Option<String>, ps4: bool, wait: bool) -> Result<()> {
    if let Some(key) = regist_key {
        rp_core::wake(console, &key, !ps4)?;
        println!("Wake-up packet sent to {console}.");
        return Ok(());
    }
    let store = Store::open_default()?;
    let (paired, secrets) = paired_with_secrets(&store, console)?;
    if wait {
        eprint!("Waking {}", paired.name);
        tether_core::ensure_awake(&paired, &secrets, Duration::from_secs(60), |p| {
            if p != WakeProgress::Waking {
                eprint!(".");
                std::io::stderr().flush().ok();
            }
        })?;
        eprintln!();
        println!("{} is ready.", paired.name);
        return Ok(());
    }
    rp_core::wake(
        &paired.address,
        &secrets.regist_key_hex(),
        paired.target().is_ps5(),
    )?;
    println!(
        "Wake-up packet sent to {} ({}).",
        paired.name, paired.address
    );
    Ok(())
}

/// Looks up a paired console and its keys.
fn paired_with_secrets(store: &Store, query: &str) -> Result<(PairedConsole, ConsoleSecrets)> {
    let paired = store
        .find(query)
        .with_context(|| format!("{query:?} is not paired. Pair it with `tether pair <address>`."))?
        .clone();
    let secrets = store.secrets_for(&paired)?.with_context(|| {
        format!(
            "keys for {} are missing from the {}; pair it again",
            paired.name,
            store.secret_store().describe()
        )
    })?;
    Ok((paired, secrets))
}

/// Reads one line from stdin after printing `label` to stderr.
fn prompt(label: &str) -> Result<String> {
    eprint!("{label}");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    if std::io::stdin().lock().read_line(&mut line)? == 0 {
        bail!("no input");
    }
    Ok(line.trim().to_owned())
}

/// Parses an IP address or resolves a host name, preferring IPv4.
fn resolve(host: &str) -> Result<IpAddr> {
    if let Ok(ip) = host.parse() {
        return Ok(ip);
    }
    let addrs: Vec<IpAddr> = (host, 0)
        .to_socket_addrs()
        .with_context(|| format!("could not resolve {host}"))?
        .map(|a| a.ip())
        .collect();
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
        .with_context(|| format!("{host} has no addresses"))
}

/// Left-aligned columns separated by two spaces; the last column is not padded.
fn table(header: &[&str], rows: Vec<Vec<String>>) -> String {
    let widths: Vec<usize> = (0..header.len())
        .map(|i| {
            rows.iter()
                .map(|r| r[i].chars().count())
                .chain([header[i].len()])
                .max()
                .unwrap()
        })
        .collect();
    let line = |cells: Vec<&str>| {
        let mut out = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if i + 1 == cells.len() {
                out += cell;
            } else {
                out += &format!("{cell:<w$}  ", w = widths[i]);
            }
        }
        out + "\n"
    };
    let mut out = line(header.to_vec());
    for row in &rows {
        out += &line(row.iter().map(String::as_str).collect());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_layout() {
        let rows = vec![
            vec!["Living Room".into(), "192.168.1.20".into(), "ready".into()],
            vec!["Den".into(), "10.0.0.5".into(), "rest mode".into()],
        ];
        assert_eq!(
            table(&["NAME", "ADDRESS", "STATE"], rows),
            "NAME         ADDRESS       STATE\n\
             Living Room  192.168.1.20  ready\n\
             Den          10.0.0.5      rest mode\n"
        );
    }

    #[test]
    fn resolves_ip_literals() {
        assert_eq!(
            resolve("192.168.1.20").unwrap(),
            "192.168.1.20".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn cli_parses() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
