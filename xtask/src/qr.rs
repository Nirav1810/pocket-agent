//! Terminal QR codes for pairing.

use anyhow::{Context, Result};
use pocket_agent_protocol::PairingPayload;
use qrcode::QrCode;
use qrcode::render::unicode::Dense1x2;

/// Print the pairing QR code plus the URL and code for manual entry.
pub fn print_pairing(url: &str, code: &str) -> Result<()> {
    let payload = PairingPayload::new(url, code).context("invalid relay URL or pairing code")?;
    let qr = QrCode::new(payload.to_uri().as_bytes()).context("could not encode QR code")?;
    // Inverted colours: dark modules drawn as spaces read best on dark terminals
    // and still scan on light ones thanks to the quiet zone.
    let image = qr
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .quiet_zone(true)
        .build();
    println!("\n{image}\n");
    println!("Scan this in Pocket Agent → Relay mode → Scan QR, or enter manually:");
    println!("  Relay URL:    {}", payload.url);
    println!("  Pairing code: {}", payload.code);
    println!();
    println!("The pairing code is shown only now and is not saved anywhere.");
    println!("Lost it? Run `cargo xtask relay-rotate-code` to make a new one.");
    Ok(())
}
