//! Render a string to a minimal black-and-white SVG QR code.

use qrcode::{Color, QrCode};

/// Encode `data` as a QR code and return a self-contained SVG string.
/// Returns an empty string if the data is too large to encode.
pub fn qr_svg(data: &str) -> String {
    let Ok(code) = QrCode::new(data.as_bytes()) else {
        return String::new();
    };
    let width = code.width();
    let quiet = 2usize; // quiet-zone modules around the code
    let dim = width + quiet * 2;
    let colors = code.to_colors();

    let mut rects = String::new();
    for y in 0..width {
        for x in 0..width {
            if colors[y * width + x] == Color::Dark {
                rects.push_str(&format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"1\" height=\"1\"/>",
                    x + quiet,
                    y + quiet
                ));
            }
        }
    }

    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {dim} {dim}\" \
         shape-rendering=\"crispEdges\">\
         <rect width=\"{dim}\" height=\"{dim}\" fill=\"#ffffff\"/>\
         <g fill=\"#0a0a0a\">{rects}</g></svg>"
    )
}

#[cfg(test)]
mod tests {
    use super::qr_svg;

    #[test]
    fn encodes_link_as_svg() {
        let svg = qr_svg("https://file-send.app/#/room/abcd");
        assert!(svg.starts_with("<svg"), "should be an svg document");
        assert!(svg.contains("<rect"), "should contain module rects");
    }

    #[test]
    fn empty_string_on_unencodable_input() {
        // QR codes have a maximum capacity; an absurdly long payload fails.
        let huge = "x".repeat(10_000);
        assert_eq!(qr_svg(&huge), "");
    }
}
