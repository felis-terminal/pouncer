/// Default alphabet: the QWERTY home row first (shortest labels land on the
/// easiest keys), then the top and bottom rows.
pub(crate) const DEFAULT: &str = "asdfghjklqwertyuiopzxcvbnm";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Invalid {
    TooFewDistinct(usize),
    Duplicate(char),
    NotTypeable(char),
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooFewDistinct(n) => {
                write!(
                    f,
                    "alphabet needs at least 2 distinct characters, found {n}"
                )
            }
            Self::Duplicate(c) => write!(f, "alphabet repeats the character {c:?}"),
            Self::NotTypeable(c) => {
                write!(
                    f,
                    "alphabet character {c:?} is not a typeable ASCII graphic character"
                )
            }
        }
    }
}

impl std::error::Error for Invalid {}

pub(crate) fn validate(alphabet: &str) -> Result<(), Invalid> {
    let mut seen = std::collections::HashSet::new();
    for c in alphabet.chars() {
        if !c.is_ascii_graphic() {
            return Err(Invalid::NotTypeable(c));
        }
        if !seen.insert(c) {
            return Err(Invalid::Duplicate(c));
        }
    }
    if seen.len() < 2 {
        return Err(Invalid::TooFewDistinct(seen.len()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{DEFAULT, Invalid, validate};

    #[test]
    fn default_alphabet_is_valid() {
        assert!(validate(DEFAULT).is_ok());
    }

    #[test]
    fn rejects_repeated_characters() {
        assert_eq!(validate("aba"), Err(Invalid::Duplicate('a')));
    }

    #[test]
    fn rejects_fewer_than_two_distinct_characters() {
        assert_eq!(validate("a"), Err(Invalid::TooFewDistinct(1)));
    }

    #[test]
    fn two_distinct_characters_are_enough() {
        assert_eq!(validate("ab"), Ok(()));
    }

    #[test]
    fn each_rejection_says_what_is_wrong_with_the_alphabet() {
        assert_eq!(
            Invalid::TooFewDistinct(1).to_string(),
            "alphabet needs at least 2 distinct characters, found 1"
        );
        assert_eq!(
            Invalid::Duplicate('a').to_string(),
            "alphabet repeats the character 'a'"
        );
        assert_eq!(
            Invalid::NotTypeable(' ').to_string(),
            "alphabet character ' ' is not a typeable ASCII graphic character"
        );
    }

    #[test]
    fn rejects_untypeable_characters() {
        assert_eq!(validate("ab cd"), Err(Invalid::NotTypeable(' ')));
    }
}
