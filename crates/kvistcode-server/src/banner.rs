//! What the terminal shows: the address, a pairing link and its QR code.

use std::path::Path;

/// A QR code for `text` drawn with Unicode half blocks, two rows of pixels per line.
pub fn qr(text: &str) -> Option<String> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    Some(
        code.render::<qrcode::render::unicode::Dense1x2>()
            .quiet_zone(true)
            .build(),
    )
}

fn minutes_left(expires_at_ms: i64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    ((expires_at_ms - now + 59_999) / 60_000).max(1)
}

/// The pairing link, its QR code, and how to get another one. `data_dir` is only named in
/// the hint when it isn't the default.
pub fn pairing(url: &str, expires_at_ms: i64, hint_dir: Option<&Path>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Open this link in a browser to pair it. It works once and expires in {} minutes:\n\n  {url}\n\n",
        minutes_left(expires_at_ms)
    ));
    if let Some(qr) = qr(url) {
        out.push_str(&qr);
        out.push('\n');
    }
    let dir = hint_dir
        .map(|d| format!(" --data-dir {}", d.display()))
        .unwrap_or_default();
    out.push_str(&format!(
        "Need a new link? Run: kvistcode-server --pair{dir}\n"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_is_drawn_for_a_pairing_link() {
        let art = qr("http://127.0.0.1:47821/#pair=abc").unwrap();
        assert!(art.lines().count() > 10);
    }

    #[test]
    fn the_pairing_text_says_when_it_expires_and_how_to_renew() {
        let in_five = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
            + 5 * 60_000;
        let text = pairing("http://127.0.0.1:1/#pair=x", in_five, None);
        assert!(text.contains("expires in 5 minutes"));
        assert!(text.contains("kvistcode-server --pair"));
    }
}
