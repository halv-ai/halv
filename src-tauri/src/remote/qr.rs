//! The QR code of a pairing offer (docs/remote-protocol.md, section 5.1).
//!
//! The offer URI holds the pairing secret. The QR code holds the same secret,
//! so the gateway gives it only to the main webview and never logs it.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use qrcode::render::svg;
use qrcode::{EcLevel, QrCode};

/// Renders `text` as an SVG QR code and returns it as a `data:` URI.
pub(crate) fn qr_svg_data_uri(text: &str) -> Result<String, String> {
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M)
        .map_err(|_| "Halv could not make the pairing QR code.".to_string())?;
    let image = code
        .render::<svg::Color<'_>>()
        .quiet_zone(true)
        .min_dimensions(256, 256)
        .dark_color(svg::Color("#000000"))
        .light_color(svg::Color("#ffffff"))
        .build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        STANDARD.encode(image)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_code_is_an_svg_data_uri() {
        let uri = qr_svg_data_uri("halv://pair?p=eyJ2IjoxfQ").unwrap();
        let encoded = uri
            .strip_prefix("data:image/svg+xml;base64,")
            .expect("an SVG data URI");
        let svg = String::from_utf8(STANDARD.decode(encoded).unwrap()).unwrap();
        assert!(svg.contains("<svg"));
        assert!(svg.ends_with("</svg>"));
    }

    #[test]
    fn a_full_pairing_uri_fits_in_a_qr_code() {
        let long = format!("halv://pair?p={}", "A".repeat(600));
        assert!(qr_svg_data_uri(&long).is_ok());
    }
}
