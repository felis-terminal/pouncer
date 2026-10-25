//! Patterns are permissive rather than RFC-exact: a strict grammar misses
//! real-world text, while a slightly wide match only adds a candidate the user
//! skips.

/// The order here is only the listing order: same-start ties follow the order
/// the user names presets in (see matcher).
const PRESETS: &[(&str, &str)] = &[
    // Scheme then any non-delimiter run; stops at whitespace and the brackets
    // and quotes that usually *surround* a URL rather than belong to it.
    ("url", r#"(?:https?|ftp|file)://[^\s)\]>"'}]+"#),
    ("email", r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}"),
    ("ipv4", r"\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b"),
    // Empty hextets (`{0,4}`) let one internal `::` match; a leading or
    // trailing `::` does not.
    ("ipv6", r"(?i)\b(?:[0-9a-f]{0,4}:){2,7}[0-9a-f]{1,4}\b"),
    // An optional leading segment then one or more `/segment`, so a bare word is
    // never a path but `/usr/bin`, `./x`, `~/dotfiles`, `a/b/c` all are.
    ("path", r"(?:[.\w@~-]+)?(?:/[.\w@-]+)+/?"),
    ("sha", r"\b[0-9a-f]{7,40}\b"),
    (
        "uuid",
        r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
    ),
    // The file needs a `/`, a `.ext` with a letter, or a capitalised bare name
    // (`Makefile`), so `12:30:45`, `ip:port` and `localhost:8080` are not
    // positions. The bare-name form also takes `ERROR:404`; a closed list of
    // filenames would avoid that but is one more thing to keep current.
    (
        "filepos",
        r"(?:[.\w@~-]*/)+[.\w@~-]+:[0-9]+(?::[0-9]+)?|[\w@~-][.\w@~-]*\.[A-Za-z]\w*:[0-9]+(?::[0-9]+)?|\b[A-Z][\w@~-]*:[0-9]+(?::[0-9]+)?\b",
    ),
    // A bracketed IPv6, a dotted quad, or a hostname with at least one letter
    // in its first label: the letter is what keeps `12:30` from being a port.
    (
        "hostport",
        r"(?:\[[0-9A-Fa-f:.]+\]|\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}|\b[A-Za-z0-9-]*[A-Za-z][A-Za-z0-9-]*(?:\.[A-Za-z0-9-]+)*):[0-9]{1,5}\b",
    ),
    // `#rgb` and `#rrggbb`. The 4- and 8-digit alpha forms are left out: `#1234`
    // is far more often an issue number than a colour. `#123` stays ambiguous,
    // since an all-digit `#rgb` is a real colour.
    ("hex", r"#(?:[0-9A-Fa-f]{6}|[0-9A-Fa-f]{3})\b"),
    // The URL is group 1, so `--capture 1` hands over the target, not the
    // markdown around it. One level of balanced parentheses is kept in the URL
    // (`wiki/Function_(mathematics)`), as CommonMark does.
    ("mdlink", r"\[[^\]\n]*\]\(((?:[^()\s]|\([^()\s]*\))+)\)"),
    // Every JWT header is a JSON object, and base64url of `{"` is `eyJ`. The
    // signature may be empty (`alg: none`).
    (
        "jwt",
        r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]*",
    ),
    // A Deployment-managed pod: `<name>-<replicaset hash>-<suffix>`, both drawn
    // from Kubernetes' vowel-free alphabet. A bare DNS label is not matched; it
    // is every word.
    (
        "pod",
        r"\b[a-z0-9](?:[-a-z0-9]*[a-z0-9])?-[bcdfghjklmnpqrstvwxz2456789]{6,10}-[bcdfghjklmnpqrstvwxz2456789]{5}\b",
    ),
    ("number", r"\b[0-9]+\b"),
];

#[must_use]
pub(crate) fn pattern(name: &str) -> Option<&'static str> {
    PRESETS.iter().find(|(n, _)| *n == name).map(|(_, p)| *p)
}

#[must_use]
pub(crate) fn available() -> String {
    PRESETS
        .iter()
        .map(|(n, _)| *n)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{PRESETS, available, pattern};
    use regex::Regex;

    fn re(name: &str) -> Regex {
        let p = pattern(name).unwrap_or_else(|| panic!("preset {name} missing"));
        Regex::new(p).unwrap_or_else(|_| panic!("preset {name} does not compile"))
    }

    #[test]
    fn every_preset_compiles_under_the_regex_crate() {
        for (name, p) in PRESETS {
            Regex::new(p).unwrap_or_else(|_| panic!("preset {name} does not compile"));
        }
    }

    #[test]
    fn url_matches_a_scheme_url_without_trailing_bracket() {
        let m = re("url")
            .find("see (https://example.com/a) here")
            .expect("a url");
        assert_eq!(m.as_str(), "https://example.com/a");
    }

    #[test]
    fn email_matches_an_address() {
        let m = re("email")
            .find("from a.b+x@sub.example.org now")
            .expect("an email");
        assert_eq!(m.as_str(), "a.b+x@sub.example.org");
    }

    #[test]
    fn ipv4_matches_a_dotted_quad() {
        let m = re("ipv4").find("host 192.168.0.1 up").expect("an ipv4");
        assert_eq!(m.as_str(), "192.168.0.1");
    }

    #[test]
    fn ipv6_matches_a_hextet_address() {
        let m = re("ipv6").find("at 2001:db8::1:0:0:1 ok").expect("an ipv6");
        assert_eq!(m.as_str(), "2001:db8::1:0:0:1");
    }

    #[test]
    fn path_matches_a_slash_path_but_not_a_bare_word() {
        assert_eq!(
            re("path")
                .find("edit /usr/local/bin done")
                .map(|m| m.as_str()),
            Some("/usr/local/bin")
        );
        assert!(re("path").find("just a word").is_none());
    }

    #[test]
    fn sha_matches_a_git_short_hash() {
        let m = re("sha").find("commit 1a2b3c4 fixed").expect("a sha");
        assert_eq!(m.as_str(), "1a2b3c4");
    }

    #[test]
    fn uuid_matches_a_canonical_uuid() {
        let m = re("uuid")
            .find("id 550E8400-e29b-41d4-a716-446655440000 set")
            .expect("a uuid");
        assert_eq!(m.as_str(), "550E8400-e29b-41d4-a716-446655440000");
    }

    fn all<'a>(name: &str, text: &'a str) -> Vec<&'a str> {
        re(name).find_iter(text).map(|m| m.as_str()).collect()
    }

    #[test]
    fn filepos_matches_file_line_and_optional_column() {
        assert_eq!(
            all("filepos", "error at src/main.rs:140:5 and lib.rs:3"),
            ["src/main.rs:140:5", "lib.rs:3"]
        );
        assert_eq!(all("filepos", "./Makefile:12: oops"), ["./Makefile:12"]);
    }

    #[test]
    fn filepos_matches_a_capitalised_extensionless_file() {
        assert_eq!(
            all(
                "filepos",
                "README:12:bad\nMakefile:3: *** missing separator"
            ),
            ["README:12", "Makefile:3"]
        );
    }

    #[test]
    fn filepos_ignores_clock_times_ip_ports_and_lowercase_hosts() {
        assert_eq!(
            all("filepos", "at 12:30:45 from 10.0.0.1:8080 via localhost:80"),
            Vec::<&str>::new()
        );
    }

    #[test]
    fn hostport_matches_names_ipv4_and_bracketed_ipv6() {
        assert_eq!(
            all("hostport", "localhost:8080 0.0.0.0:80->80/tcp [::1]:443"),
            ["localhost:8080", "0.0.0.0:80", "[::1]:443"]
        );
    }

    #[test]
    fn hostport_ignores_clock_times() {
        assert_eq!(all("hostport", "started 12:30:45"), Vec::<&str>::new());
    }

    #[test]
    fn hex_matches_three_and_six_digit_colours_only() {
        assert_eq!(
            all("hex", "fg #ff8800 bg #abc tag #1234"),
            ["#ff8800", "#abc"]
        );
    }

    #[test]
    fn mdlink_matches_the_link_and_captures_its_url() {
        let caps = re("mdlink")
            .captures("see [the docs](https://x.io/a) now")
            .expect("an mdlink");
        assert_eq!(&caps[0], "[the docs](https://x.io/a)");
        assert_eq!(&caps[1], "https://x.io/a");
    }

    #[test]
    fn mdlink_keeps_balanced_parentheses_in_the_url() {
        let caps = re("mdlink")
            .captures("[wiki](https://en.wikipedia.org/wiki/Function_(mathematics)).")
            .expect("an mdlink");
        assert_eq!(
            &caps[1],
            "https://en.wikipedia.org/wiki/Function_(mathematics)"
        );
    }

    #[test]
    fn jwt_matches_three_base64url_runs() {
        let token = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl";
        assert_eq!(all("jwt", &format!("Bearer {token} ok")), [token]);
    }

    #[test]
    fn pod_matches_a_deployment_pod_but_not_a_plain_name() {
        assert_eq!(
            all("pod", "web-7d4b9c8f6d-x2kq9   1/1 Running\nweb   3/3"),
            ["web-7d4b9c8f6d-x2kq9"]
        );
    }

    #[test]
    fn number_matches_digit_runs_but_not_digits_inside_words() {
        assert_eq!(all("number", "PR 42 took 1500ms, sha 1a2b"), ["42"]);
    }

    #[test]
    fn unknown_preset_is_none() {
        assert!(pattern("nope").is_none());
    }

    #[test]
    fn available_lists_every_preset_name() {
        let listed = available();
        for (name, _) in PRESETS {
            assert!(listed.contains(name), "{name} missing from available()");
        }
    }
}
