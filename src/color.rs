use crossterm::style::Color;

pub(crate) fn parse(s: &str) -> Result<Color, String> {
    if let Some(hex) = s.strip_prefix('#') {
        // Guard ASCII hex + length first so the byte-slicing below lands on
        // char boundaries and each component is known to parse.
        if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
            return Ok(Color::Rgb {
                r: byte(0),
                g: byte(2),
                b: byte(4),
            });
        }
        return Err(format!("invalid hex color {s:?}; expected #rrggbb"));
    }
    // Names never parse as u8, so an index cannot shadow one.
    if let Ok(n) = s.parse::<u8>() {
        return Ok(Color::AnsiValue(n));
    }
    // TryFrom, not FromStr: FromStr maps an unknown name to white, so a typo in
    // `--hint-bg` would pass silently.
    Color::try_from(s).map_err(|()| {
        format!("unknown color {s:?}; expected a name (e.g. red, dark_blue), 0-255, or #rrggbb")
    })
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crossterm::style::Color;

    #[test]
    fn parses_a_named_color() {
        assert_eq!(parse("red"), Ok(Color::Red));
        assert_eq!(parse("dark_blue"), Ok(Color::DarkBlue));
    }

    #[test]
    fn parses_an_ansi_index() {
        assert_eq!(parse("0"), Ok(Color::AnsiValue(0)));
        assert_eq!(parse("255"), Ok(Color::AnsiValue(255)));
    }

    #[test]
    fn parses_a_hex_truecolor() {
        assert_eq!(
            parse("#ff8800"),
            Ok(Color::Rgb {
                r: 0xff,
                g: 0x88,
                b: 0x00
            })
        );
    }

    #[test]
    fn rejects_an_out_of_range_index_rather_than_silently_accepting() {
        // 256 overflows a u8, so it must be an error, not wrapped to white.
        assert!(parse("256").is_err());
    }

    #[test]
    fn rejects_an_unknown_name_instead_of_defaulting_to_white() {
        // The whole reason for this parser: crossterm's FromStr would return
        // white here, hiding the typo.
        assert!(parse("definitely-not-a-color").is_err());
    }

    #[test]
    fn rejects_malformed_hex() {
        assert!(parse("#fff").is_err()); // too short
        assert!(parse("#gggggg").is_err()); // non-hex digits
    }
}
