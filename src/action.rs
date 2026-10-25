//! Copying uses OSC 52 rather than a platform clipboard tool: it is the one
//! path that works over SSH in any terminal (docs/design.md "Edge-case
//! handling").

#[cfg(any(windows, test))]
mod argv;
#[cfg(windows)]
mod windows;

use crate::terminal;
use anyhow::{Context, Result, bail};
use std::io::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    Print,
    Copy,
    Open,
    Command(String),
}

impl Action {
    #[must_use]
    pub(crate) fn parse(arg: Option<&str>) -> Self {
        match arg {
            None => Self::Print,
            Some("copy") => Self::Copy,
            Some("open") => Self::Open,
            Some(cmd) => Self::Command(cmd.to_owned()),
        }
    }

    pub(crate) fn run(&self, values: &[String]) -> Result<()> {
        match self {
            Self::Print => {
                let mut out = std::io::stdout().lock();
                for value in values {
                    writeln!(out, "{value}").context("write picked value to stdout")?;
                }
                Ok(())
            }
            Self::Copy => {
                let mut tty = terminal::open()
                    .with_context(|| format!("open {} for clipboard", terminal::NAME))?;
                tty.write_all(osc52(&values.join("\n")).as_bytes())
                    .and_then(|()| tty.flush())
                    .context("emit OSC 52")
            }
            Self::Open => open(values),
            Self::Command(cmd) => succeeded(run_command(cmd, values)?, "--action command"),
        }
    }
}

#[cfg(unix)]
fn open(values: &[String]) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = std::process::Command::new(opener)
        .args(values)
        .status()
        .with_context(|| format!("spawn {opener}"))?;
    succeeded(status, opener)
}

#[cfg(windows)]
use windows::open;

#[cfg(unix)]
fn run_command(cmd: &str, values: &[String]) -> Result<std::process::ExitStatus> {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(command_script(cmd))
        .arg("sh")
        .args(values)
        .status()
        .context("run --action command")
}

#[cfg(windows)]
use windows::run_command;

fn succeeded(status: std::process::ExitStatus, what: &str) -> Result<()> {
    if !status.success() {
        bail!("{what} failed ({status})");
    }
    Ok(())
}

/// `{}` becomes `"$@"`, so the values reach `sh` as positional parameters and
/// are never interpolated into the script text.
#[cfg(any(unix, test))]
fn command_script(cmd: &str) -> String {
    cmd.replace("{}", "\"$@\"")
}

#[must_use]
pub(crate) fn osc52(value: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64_encode(value.as_bytes()))
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18 & 63) as usize] as char);
        out.push(TABLE[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{Action, osc52};

    #[test]
    fn parse_maps_named_actions_and_defaults_to_print() {
        assert_eq!(Action::parse(None), Action::Print);
        assert_eq!(Action::parse(Some("copy")), Action::Copy);
        assert_eq!(Action::parse(Some("open")), Action::Open);
        assert_eq!(
            Action::parse(Some("git show {}")),
            Action::Command("git show {}".to_owned())
        );
    }

    #[test]
    fn osc52_wraps_base64_in_the_clipboard_sequence() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }
}

#[cfg(test)]
mod proptests {
    use super::{base64_encode, command_script};
    use proptest::prelude::*;

    /// Written against RFC 4648 independently of `base64_encode`, so the
    /// round trip cannot pass by sharing a mistake.
    fn decode(encoded: &str) -> Option<Vec<u8>> {
        let alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        if !encoded.len().is_multiple_of(4) {
            return None;
        }
        let mut bits = String::new();
        let mut padding = 0;
        for c in encoded.chars() {
            if c == '=' {
                padding += 1;
                bits.push_str("000000");
            } else if padding > 0 {
                return None;
            } else {
                let sextet = alphabet.find(c)?;
                bits.extend(
                    (0..6)
                        .rev()
                        .map(|i| if sextet >> i & 1 == 1 { '1' } else { '0' }),
                );
            }
        }
        let mut bytes: Vec<u8> = bits
            .as_bytes()
            .chunks(8)
            .map(|byte| u8::from_str_radix(std::str::from_utf8(byte).ok()?, 2).ok())
            .collect::<Option<_>>()?;
        bytes.truncate(bytes.len() - padding);
        Some(bytes)
    }

    proptest! {
        #[test]
        fn base64_round_trips_through_an_independent_decoder(
            input in proptest::collection::vec(any::<u8>(), 0..64),
        ) {
            let encoded = base64_encode(&input);
            prop_assert_eq!(encoded.len(), input.len().div_ceil(3) * 4);
            prop_assert_eq!(decode(&encoded), Some(input));
        }

        #[test]
        fn command_script_replaces_every_placeholder_and_nothing_else(
            parts in proptest::collection::vec("[^{}]{0,8}", 1..5),
        ) {
            let script = command_script(&parts.join("{}"));
            prop_assert_eq!(script, parts.join("\"$@\""));
        }
    }
}
