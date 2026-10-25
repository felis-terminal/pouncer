//! The Windows `--action` grammar: a program and its arguments, split the way
//! a C program splits its own command line, with `{}` expanding like `"$@"`.
//! There is no shell to hand the values to out of band: cmd.exe re-parses
//! its whole command line, and PowerShell reads its arguments as script.

/// The Microsoft C runtime rules ("Parsing C command-line arguments"). The
/// first word takes quotes but no escapes, so `"C:\Program Files\x.exe"`
/// keeps its backslashes.
pub(crate) fn split(line: &str) -> Vec<String> {
    let mut chars = line.chars().peekable();
    let mut words = Vec::new();
    while chars.next_if(|&c| is_blank(c)).is_some() {}
    if chars.peek().is_none() {
        return words;
    }

    let mut program = String::new();
    let mut quoted = false;
    while let Some(c) = chars.next_if(|&c| quoted || !is_blank(c)) {
        if c == '"' {
            quoted = !quoted;
        } else {
            program.push(c);
        }
    }
    words.push(program);

    loop {
        while chars.next_if(|&c| is_blank(c)).is_some() {}
        if chars.peek().is_none() {
            return words;
        }
        let mut word = String::new();
        let mut quoted = false;
        loop {
            let mut backslashes = 0;
            while chars.next_if_eq(&'\\').is_some() {
                backslashes += 1;
            }
            if chars.peek() == Some(&'"') {
                word.extend(std::iter::repeat_n('\\', backslashes / 2));
                chars.next();
                // An odd backslash escapes the quote; inside quotes `""` is one.
                if backslashes % 2 == 1 || (quoted && chars.next_if_eq(&'"').is_some()) {
                    word.push('"');
                } else {
                    quoted = !quoted;
                }
                continue;
            }
            word.extend(std::iter::repeat_n('\\', backslashes));
            match chars.next_if(|&c| quoted || !is_blank(c)) {
                Some(c) => word.push(c),
                None => break,
            }
        }
        words.push(word);
    }
}

const fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

/// Each `{}` in a word joins the first value to the text before it and the
/// last value to the text after it, the rest standing as words of their own.
pub(crate) fn expand(words: &[String], values: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for word in words {
        let mut pieces = word.split("{}");
        let mut current = pieces.next().unwrap_or_default().to_owned();
        for piece in pieces {
            if let Some((last, init)) = values.split_last() {
                for value in init {
                    current.push_str(value);
                    out.push(std::mem::take(&mut current));
                }
                current.push_str(last);
            }
            current.push_str(piece);
        }
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{expand, split};

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|&s| s.to_owned()).collect()
    }

    #[test]
    fn split_follows_microsofts_table_for_arguments() {
        // "Parsing C command-line arguments", the results table.
        assert_eq!(
            split(r#"p "a b c" d e"#),
            strings(&["p", "a b c", "d", "e"])
        );
        assert_eq!(
            split(r#"p "ab\"c" "\\" d"#),
            strings(&["p", r#"ab"c"#, r"\", "d"])
        );
        assert_eq!(
            split(r#"p a\\\b d"e f"g h"#),
            strings(&["p", r"a\\\b", "de fg", "h"])
        );
        assert_eq!(
            split(r#"p a\\\"b c d"#),
            strings(&["p", r#"a\"b"#, "c", "d"])
        );
        assert_eq!(
            split(r#"p a\\\\"b c" d e"#),
            strings(&["p", r"a\\b c", "d", "e"])
        );
        assert_eq!(split(r#"p a"b"" c d"#), strings(&["p", r#"ab" c d"#]));
    }

    #[test]
    fn split_keeps_backslashes_in_a_quoted_program_path() {
        assert_eq!(
            split(r#""C:\Program Files\app\" -g {}"#),
            strings(&[r"C:\Program Files\app\", "-g", "{}"])
        );
    }

    #[test]
    fn split_treats_tabs_as_blanks_and_single_quotes_as_text() {
        assert_eq!(
            split("  code\t-g  'a b'  "),
            strings(&["code", "-g", "'a", "b'"])
        );
    }

    #[test]
    fn split_of_a_blank_line_is_empty() {
        assert_eq!(split(" \t "), Vec::<String>::new());
    }

    #[test]
    fn split_keeps_an_explicitly_empty_argument() {
        assert_eq!(split(r#"p "" x"#), strings(&["p", "", "x"]));
    }

    #[test]
    fn expand_gives_each_value_its_own_word() {
        assert_eq!(
            expand(&strings(&["code", "-g", "{}"]), &strings(&["a b", "c"])),
            strings(&["code", "-g", "a b", "c"])
        );
    }

    #[test]
    fn expand_attaches_surrounding_text_to_the_first_and_last_value() {
        assert_eq!(
            expand(&strings(&["--file=<{}>"]), &strings(&["a", "b", "c"])),
            strings(&["--file=<a", "b", "c>"])
        );
    }

    #[test]
    fn expand_handles_two_placeholders_in_one_word() {
        assert_eq!(
            expand(&strings(&["{}+{}"]), &strings(&["a", "b"])),
            strings(&["a", "b+a", "b"])
        );
    }

    #[test]
    fn expand_never_resplits_a_value() {
        assert_eq!(
            expand(&strings(&["x", "{}"]), &strings(&[r#"a "b" {}"#])),
            strings(&["x", r#"a "b" {}"#])
        );
    }
}
