//! `cargo xtask <command>`: developer and self-hosting tasks for Pocket Agent.
//!
//! This replaces shell scripts so the same commands work on Linux, macOS and
//! Windows. Run `cargo xtask --help` for the list.
//!
//! Secrets typed into these commands (API key, pairing code) are piped
//! straight to `wrangler secret put` and never written to disk.

mod check;
mod it;
mod qr;
mod setup;
mod util;
mod wrangler;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "cargo xtask",
    about = "Pocket Agent developer and self-hosting tasks"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Interactive first-time relay deployment to your Cloudflare account:
    /// creates the D1 database, applies migrations, deploys, sets secrets,
    /// and prints the pairing QR code.
    RelaySetup {
        /// Worker name (becomes <name>.<your-subdomain>.workers.dev).
        #[arg(long, default_value = "pocket-agent-relay")]
        name: String,
    },
    /// Rebuild and redeploy an already set-up relay (applies new migrations).
    RelayDeploy,
    /// Generate a new pairing code, store it as the PAIRING_CODE secret, and
    /// print the QR. Unlocks pairing after too many wrong attempts. Already
    /// paired devices keep working.
    RelayRotateCode {
        /// Relay URL, for the QR code (default: read from `wrangler deploy` output is not
        /// possible here, so it is asked for when omitted).
        #[arg(long)]
        url: Option<String>,
    },
    /// Print a pairing QR code for a relay URL (prompts for the code, hidden).
    PairQr {
        /// Relay URL, e.g. https://pocket-agent-relay.you.workers.dev
        #[arg(long)]
        url: String,
    },
    /// End-to-end check of a deployed relay: pairs a temporary device, sends a
    /// tiny request to Claude through the relay, then revokes the device.
    RelayCheck {
        /// Relay URL.
        #[arg(long)]
        url: String,
    },
    /// Local integration test: runs the relay under `wrangler dev` against a
    /// mock Anthropic upstream and exercises every route. No account needed.
    RelayIt,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::RelaySetup { name } => setup::relay_setup(&name),
        Cmd::RelayDeploy => setup::relay_deploy(),
        Cmd::RelayRotateCode { url } => setup::rotate_code(url),
        Cmd::PairQr { url } => {
            let code = util::prompt_secret("Pairing code: ")?;
            qr::print_pairing(&url, &code)
        }
        Cmd::RelayCheck { url } => {
            let code = util::prompt_secret("Pairing code (input hidden): ")?;
            check::relay_check(&url, &code)
        }
        Cmd::RelayIt => it::run(),
    }
}
