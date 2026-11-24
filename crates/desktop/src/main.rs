//! `tether`: desktop entry point. For now a CLI; the Slint UI arrives in
//! milestone 5.

use std::net::{IpAddr, ToSocketAddrs};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rp_core::{Console, DiscoveryOptions};
use tracing_subscriber::EnvFilter;

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

    #[command(subcommand)]
    command: Command,
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
    /// Wake a console from rest mode
    Wake {
        /// IP address or host name of the console
        host: String,

        /// Registration key from pairing (up to 8 hex digits)
        #[arg(short = 'k', long)]
        regist_key: String,

        /// The console is a PS4 (uses the PS4 wake port)
        #[arg(long)]
        ps4: bool,
    },
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

fn run(command: Command) -> Result<()> {
    match command {
        Command::Discover { timeout, host } => {
            if !(timeout.is_finite() && timeout > 0.0) {
                bail!("--timeout must be a positive number of seconds");
            }
            let options = DiscoveryOptions {
                host: host.as_deref().map(resolve).transpose()?,
                ..Default::default()
            };
            let consoles = rp_core::discover(Duration::from_secs_f64(timeout), &options)?;
            if consoles.is_empty() {
                eprintln!("No consoles found.");
            } else {
                print!("{}", format_table(&consoles));
            }
            Ok(())
        }
        Command::Wake {
            host,
            regist_key,
            ps4,
        } => {
            rp_core::wake(&host, &regist_key, !ps4)?;
            println!("Wake-up packet sent to {host}.");
            Ok(())
        }
    }
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

fn format_table(consoles: &[Console]) -> String {
    let rows: Vec<[String; 3]> = consoles
        .iter()
        .map(|c| [c.name.clone(), c.addr.to_string(), c.state.to_string()])
        .collect();
    let header = ["NAME", "ADDRESS", "STATE"];
    let width = |col: usize| {
        rows.iter()
            .map(|r| r[col].chars().count())
            .chain([header[col].len()])
            .max()
            .unwrap()
    };
    let (w0, w1) = (width(0), width(1));

    let mut out = format!("{:<w0$}  {:<w1$}  {}\n", header[0], header[1], header[2]);
    for [name, addr, state] in &rows {
        out += &format!("{name:<w0$}  {addr:<w1$}  {state}\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rp_core::{ConsoleKind, ConsoleState};

    fn console(name: &str, ip: &str, state: ConsoleState) -> Console {
        Console {
            host_id: "id".into(),
            name: name.into(),
            addr: ip.parse().unwrap(),
            state,
            kind: ConsoleKind::Ps5,
            host_type: None,
            system_version: None,
            request_port: 997,
            running_app: None,
        }
    }

    #[test]
    fn table_layout() {
        let table = format_table(&[
            console("Living Room", "192.168.1.20", ConsoleState::Ready),
            console("Den", "10.0.0.5", ConsoleState::Standby),
        ]);
        assert_eq!(
            table,
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
}
