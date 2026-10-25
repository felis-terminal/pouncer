//! The felis-to-crossterm translation lives here so that only `source` names a
//! felis type. Indexed and default colours are not resolved: the terminal
//! pouncer runs in applies its own palette, as it would to the producer's
//! output.

use crossterm::style::{Attribute, Attributes, Color};
use felis_grid::{AttrFlags, Attributes as FelisAttributes, Color as FelisColor, UnderlineStyle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Style {
    pub(crate) fg: Color,
    pub(crate) bg: Color,
    /// [`Color::Reset`] follows the foreground, as an absent `SGR 58` asks.
    pub(crate) underline: Color,
    pub(crate) attrs: Attributes,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            fg: Color::Reset,
            bg: Color::Reset,
            underline: Color::Reset,
            attrs: Attributes::default(),
        }
    }
}

impl Style {
    /// A foreground-only pen leaves no ink on a space, nor does a
    /// background-only one under `NO_COLOR`; keeping such a blank would make
    /// an erased row count as content to the trailing-row trim.
    #[must_use]
    pub(crate) const fn paints_blank(&self, colors_enabled: bool) -> bool {
        (colors_enabled && !matches!(self.bg, Color::Reset))
            || self.attrs.has(Attribute::Reverse)
            || self.attrs.has(Attribute::Underlined)
            || self.attrs.has(Attribute::DoubleUnderlined)
            || self.attrs.has(Attribute::Undercurled)
            || self.attrs.has(Attribute::Underdotted)
            || self.attrs.has(Attribute::Underdashed)
            || self.attrs.has(Attribute::CrossedOut)
            || self.attrs.has(Attribute::OverLined)
    }
}

impl From<&FelisAttributes> for Style {
    fn from(attributes: &FelisAttributes) -> Self {
        let flags = attributes.flags;
        let mut attrs = Attributes::none();
        for (flag, attribute) in [
            (AttrFlags::BOLD, Attribute::Bold),
            (AttrFlags::FAINT, Attribute::Dim),
            (AttrFlags::ITALIC, Attribute::Italic),
            (AttrFlags::BLINK, Attribute::SlowBlink),
            (AttrFlags::REVERSE, Attribute::Reverse),
            (AttrFlags::CONCEAL, Attribute::Hidden),
            (AttrFlags::STRIKETHROUGH, Attribute::CrossedOut),
            (AttrFlags::OVERLINE, Attribute::OverLined),
        ] {
            if flags.contains(flag) {
                attrs.set(attribute);
            }
        }
        // PROTECTED / ISO_PROTECTED are DECSCA and SPA/EPA erase guards, not
        // SGR: they have no rendering at all, so they map to nothing.
        if flags.contains(AttrFlags::UNDERLINE) {
            attrs.set(match attributes.underline_style {
                UnderlineStyle::Single => Attribute::Underlined,
                UnderlineStyle::Double => Attribute::DoubleUnderlined,
                UnderlineStyle::Curly => Attribute::Undercurled,
                UnderlineStyle::Dotted => Attribute::Underdotted,
                UnderlineStyle::Dashed => Attribute::Underdashed,
            });
        }
        Self {
            fg: color(attributes.fg),
            bg: color(attributes.bg),
            underline: color(attributes.underline_color),
            attrs,
        }
    }
}

const fn color(c: FelisColor) -> Color {
    match c {
        FelisColor::Default => Color::Reset,
        FelisColor::Indexed(n) => Color::AnsiValue(n),
        FelisColor::Rgb(r, g, b) => Color::Rgb { r, g, b },
    }
}

#[cfg(test)]
mod tests {
    use super::Style;
    use crossterm::style::{Attribute, Attributes, Color};
    use felis_grid::{
        AttrFlags, Attributes as FelisAttributes, Color as FelisColor, UnderlineStyle,
    };

    fn with_flags(flags: AttrFlags) -> Style {
        Style::from(&FelisAttributes {
            flags,
            ..FelisAttributes::default()
        })
    }

    #[test]
    fn default_felis_attributes_convert_to_the_default_style() {
        assert_eq!(Style::from(&FelisAttributes::default()), Style::default());
    }

    #[test]
    fn each_felis_flag_maps_to_its_crossterm_attribute() {
        for (flag, attribute) in [
            (AttrFlags::BOLD, Attribute::Bold),
            (AttrFlags::FAINT, Attribute::Dim),
            (AttrFlags::ITALIC, Attribute::Italic),
            (AttrFlags::BLINK, Attribute::SlowBlink),
            (AttrFlags::REVERSE, Attribute::Reverse),
            (AttrFlags::CONCEAL, Attribute::Hidden),
            (AttrFlags::STRIKETHROUGH, Attribute::CrossedOut),
            (AttrFlags::OVERLINE, Attribute::OverLined),
        ] {
            assert_eq!(with_flags(flag).attrs, Attributes::from(attribute));
        }
    }

    #[test]
    fn the_protection_flags_map_to_no_attribute() {
        assert!(with_flags(AttrFlags::PROTECTED).attrs.is_empty());
        assert!(with_flags(AttrFlags::ISO_PROTECTED).attrs.is_empty());
    }

    #[test]
    fn colors_map_default_to_reset_indexed_to_ansi_and_rgb_to_rgb() {
        let style = Style::from(&FelisAttributes {
            fg: FelisColor::Indexed(9),
            bg: FelisColor::Rgb(1, 2, 3),
            underline_color: FelisColor::Default,
            ..FelisAttributes::default()
        });
        assert_eq!(style.fg, Color::AnsiValue(9));
        assert_eq!(style.bg, Color::Rgb { r: 1, g: 2, b: 3 });
        assert_eq!(style.underline, Color::Reset);
    }

    #[test]
    fn a_blank_paints_under_reverse_underline_strike_or_overline_whatever_the_color_policy() {
        for flag in [
            AttrFlags::REVERSE,
            AttrFlags::STRIKETHROUGH,
            AttrFlags::OVERLINE,
        ] {
            assert!(with_flags(flag).paints_blank(true), "{flag:?} should paint");
            assert!(
                with_flags(flag).paints_blank(false),
                "{flag:?} survives NO_COLOR"
            );
        }
        for underline_style in [
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Curly,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
        ] {
            let style = Style::from(&FelisAttributes {
                flags: AttrFlags::UNDERLINE,
                underline_style,
                ..FelisAttributes::default()
            });
            assert!(style.paints_blank(true), "{underline_style:?} should paint");
            assert!(
                style.paints_blank(false),
                "{underline_style:?} survives NO_COLOR"
            );
        }
    }

    #[test]
    fn a_background_only_blank_paints_only_while_colors_are_enabled() {
        // Under NO_COLOR crossterm writes no background at all, so the cell
        // would repaint as a plain space while still counting as content.
        let style = Style {
            bg: Color::AnsiValue(1),
            ..Style::default()
        };
        assert!(style.paints_blank(true));
        assert!(!style.paints_blank(false));
    }

    #[test]
    fn a_blank_paints_nothing_under_a_foreground_only_pen() {
        for colors_enabled in [true, false] {
            assert!(!Style::default().paints_blank(colors_enabled));
            assert!(
                !Style {
                    fg: Color::AnsiValue(1),
                    underline: Color::AnsiValue(2),
                    ..Style::default()
                }
                .paints_blank(colors_enabled)
            );
            for flag in [
                AttrFlags::BOLD,
                AttrFlags::FAINT,
                AttrFlags::ITALIC,
                AttrFlags::BLINK,
                AttrFlags::CONCEAL,
            ] {
                assert!(
                    !with_flags(flag).paints_blank(colors_enabled),
                    "{flag:?} paints nothing"
                );
            }
        }
    }

    #[test]
    fn the_underline_shape_maps_only_when_the_underline_flag_is_set() {
        let shaped = |flags, underline_style| {
            Style::from(&FelisAttributes {
                flags,
                underline_style,
                ..FelisAttributes::default()
            })
            .attrs
        };
        assert!(shaped(AttrFlags::empty(), UnderlineStyle::Curly).is_empty());
        for (underline_style, attribute) in [
            (UnderlineStyle::Single, Attribute::Underlined),
            (UnderlineStyle::Double, Attribute::DoubleUnderlined),
            (UnderlineStyle::Curly, Attribute::Undercurled),
            (UnderlineStyle::Dotted, Attribute::Underdotted),
            (UnderlineStyle::Dashed, Attribute::Underdashed),
        ] {
            assert_eq!(
                shaped(AttrFlags::UNDERLINE, underline_style),
                Attributes::from(attribute)
            );
        }
    }
}
