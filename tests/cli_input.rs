//! The input contract pouncer exposes to its launchers: scrollback comes from
//! stdin by default (kitty, tmux, a bare shell pipe) or from a file argument
//! (felis' pipe action, which occupies stdin with the interactive pty). A
//! single match resolves without the picker, so these run headless and assert
//! the value pouncer would act on.

// Test scaffolding may panic on failure; the workspace restriction lints
// (mirrored from felis) treat that as a smell in shipping code only.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::io::Write;
use std::process::{Command, Stdio};

/// Run pouncer with the given args and stdin, returning (stdout, stderr, success).
fn run(args: &[&str], stdin: &[u8]) -> (String, String, bool) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pouncer"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pouncer");
    // A startup-validation case (bad flag) exits before reading stdin, closing
    // the pipe; the write then races to a broken pipe. That is expected here,
    // so ignore the write result and let the assertions judge stdout/stderr.
    let _ = child.stdin.take().expect("stdin").write_all(stdin);
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn reads_a_single_match_from_stdin() {
    let (stdout, _, ok) = run(&["--preset", "url"], b"go to https://example.com now\n");
    assert!(ok);
    assert_eq!(stdout.trim(), "https://example.com");
}

#[test]
fn reads_a_single_match_from_a_file_argument() {
    let mut file = tempfile();
    file.write_all(b"open https://felis.example here\n")
        .unwrap();
    let path = file.path_string();
    // stdin is left empty: the file argument is the source, mirroring felis
    // handing over a temp file while stdin is the keyboard pty.
    let (stdout, _, ok) = run(&["--preset", "url", &path], b"");
    assert!(ok);
    assert_eq!(stdout.trim(), "https://felis.example");
}

#[test]
fn a_dash_argument_means_stdin() {
    let (stdout, _, ok) = run(&["--preset", "url", "-"], b"see https://rust-lang.org ok\n");
    assert!(ok);
    assert_eq!(stdout.trim(), "https://rust-lang.org");
}

#[test]
fn a_missing_file_fails_with_a_clear_message() {
    let (_, stderr, ok) = run(&["--preset", "url", "/no/such/pouncer/input"], b"");
    assert!(!ok);
    assert!(
        stderr.contains("/no/such/pouncer/input"),
        "error names the path: {stderr:?}"
    );
}

#[test]
fn min_len_drops_the_short_matches_and_keeps_the_long_one() {
    // Two hex runs; --min-len leaves exactly one, which resolves headless.
    let (stdout, _, ok) = run(
        &["--regex", "[0-9a-f]+", "--min-len", "6"],
        b"ab cd abcdef\n",
    );
    assert!(ok);
    assert_eq!(stdout.trim(), "abcdef");
}

#[test]
fn min_len_filtering_everything_out_reports_no_matches() {
    // An emptied set must be reported like any other, not handed to the
    // picker as an empty screen.
    let (stdout, stderr, ok) = run(&["--regex", "[0-9a-f]+", "--min-len", "20"], b"ab abcdef\n");
    assert!(!ok);
    assert!(stdout.is_empty(), "nothing is acted on: {stdout:?}");
    assert!(stderr.contains("no matches"), "{stderr:?}");
}

#[test]
fn a_capture_group_a_pattern_lacks_is_rejected_at_startup() {
    let (_, stderr, ok) = run(
        &["--preset", "mdlink,url", "--capture", "1"],
        b"[a](https://a.io) https://b.io\n",
    );
    assert!(!ok);
    assert!(
        stderr.contains("--preset url has no group 1"),
        "error names the pattern: {stderr:?}"
    );
}

#[test]
fn a_capture_group_every_pattern_has_is_extracted() {
    let (stdout, _, ok) = run(&["--regex", "#(\\d+)", "--capture", "1"], b"PR #42\n");
    assert!(ok);
    assert_eq!(stdout.trim(), "42");
}

#[cfg(unix)]
#[test]
fn an_opener_that_cannot_be_started_fails_pouncer() {
    let empty = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("no-opener-on-path");
    std::fs::create_dir_all(&empty).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pouncer"))
        .args(["--preset", "url", "--action", "open"])
        .env("PATH", &empty)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pouncer");
    let _ = child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"see https://example.com\n");
    let out = child.wait_with_output().expect("wait");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr:?}");
    assert!(stderr.contains("spawn"), "{stderr:?}");
}

#[cfg(unix)]
const FAILING_COMMAND: &str = "exit 3";
#[cfg(windows)]
const FAILING_COMMAND: &str = "cmd /c exit 3";

#[test]
fn a_failing_action_command_fails_pouncer() {
    let (_, stderr, ok) = run(
        &["--preset", "url", "--action", FAILING_COMMAND],
        b"see https://example.com\n",
    );
    assert!(!ok);
    assert!(stderr.contains("--action command failed"), "{stderr:?}");
}

#[cfg(unix)]
#[test]
fn a_succeeding_action_command_receives_the_value() {
    let (stdout, _, ok) = run(
        &["--preset", "url", "--action", "printf '<%s>' {}"],
        b"see https://example.com\n",
    );
    assert!(ok);
    assert_eq!(stdout, "<https://example.com>");
}

#[cfg(unix)]
#[test]
fn a_command_action_receives_shell_and_quoting_characters_verbatim() {
    let value = r#"<"q" \ \" a\\ & | ; % $HOME `x` 日本>"#;
    let input = format!("see {value} here\n");
    let (stdout, stderr, ok) = run(
        &["--regex", "<[^>]*>", "--action", "printf '%s' {}"],
        input.as_bytes(),
    );
    assert!(ok, "{stderr:?}");
    assert_eq!(stdout, value);
}

/// Windows has no `printf` to receive the value, so a second pouncer does:
/// `--preset <value>` fails naming the value in `{:?}` form, which shows the
/// exact argument it was given. `--preset` splits on commas, so the value has
/// none; `argv::expand` never re-splits a value, commas included.
#[cfg(windows)]
#[test]
fn a_command_action_receives_quoting_and_cmd_characters_as_one_argument() {
    let value = r#"<"q" \ \" a\\ & | % ^ $HOME 日本 x\>"#;
    let action = format!("\"{}\" --preset {{}}", env!("CARGO_BIN_EXE_pouncer"));
    let input = format!("see {value} here\n");
    let (_, stderr, ok) = run(
        &["--regex", "<[^>]*>", "--action", &action],
        input.as_bytes(),
    );
    assert!(!ok);
    assert!(
        stderr.contains(&format!("unknown --preset {value:?}")),
        "{stderr:?}"
    );
}

/// A temp dir holding only `probe.cmd`, put first on `PATH`.
#[cfg(windows)]
fn path_with_probe_cmd(name: &str) -> std::ffi::OsString {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("probe.cmd"), "@echo off\r\necho [%*]\r\n").unwrap();
    let mut paths = vec![dir];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    std::env::join_paths(paths).unwrap()
}

#[cfg(windows)]
#[test]
fn a_bare_program_name_finds_a_batch_file_that_gets_the_value_as_one_quoted_argument() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_pouncer"))
        .args(["--regex", r"x&\S+", "--action", "probe {}"])
        .env("PATH", path_with_probe_cmd("probe-cmd"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pouncer");
    let _ = child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"see x&echo.INJECTED here\n");
    let out = child.wait_with_output().expect("wait");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout.trim(), r#"["x&echo.INJECTED"]"#);
}

#[cfg(windows)]
#[test]
fn a_program_in_the_current_directory_is_not_run_even_named_with_its_extension() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cwd-planted");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("planted.cmd"), "@echo PLANTED\r\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_pouncer"))
        .args(["--preset", "url", "--action", "planted.cmd {}"])
        .current_dir(&dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pouncer");
    let _ = child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"see https://example.com\n");
    let out = child.wait_with_output().expect("wait");
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("PLANTED"));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("was not found on PATH"),
        "{:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(windows)]
#[test]
fn a_batch_file_with_an_exe_twin_is_refused_rather_than_running_the_twin() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("exe-twin");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("twin.cmd"), "@echo RAN-CMD\r\n").unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_pouncer"), dir.join("twin.cmd.exe")).unwrap();
    let action = format!("\"{}\" {{}}", dir.join("twin.cmd").display());
    let (stdout, stderr, ok) = run(
        &["--preset", "url", "--action", &action],
        b"see https://example.com\n",
    );
    assert!(!ok);
    assert!(!stdout.contains("RAN-CMD"), "{stdout:?}");
    assert!(stderr.contains("refusing to guess"), "{stderr:?}");
}
#[cfg(windows)]
#[test]
fn a_command_action_naming_a_missing_program_says_so() {
    let (_, stderr, ok) = run(
        &["--preset", "url", "--action", "no-such-pouncer-program {}"],
        b"see https://example.com\n",
    );
    assert!(!ok);
    assert!(
        stderr.contains(r#"program "no-such-pouncer-program" was not found on PATH"#),
        "{stderr:?}"
    );
}

#[test]
fn input_past_the_grid_row_limit_scans_the_tail_and_says_what_it_dropped() {
    let mut input = b"lost https://head.example\n".to_vec();
    input.extend(b"x\n".repeat(70_000));
    input.extend(b"kept https://tail.example\n");
    let (stdout, stderr, ok) = run(&["--preset", "url", "--cols", "80"], &input);
    assert!(ok);
    assert_eq!(stdout.trim(), "https://tail.example");
    assert!(
        stderr.contains("rows were not scanned"),
        "the loss is reported: {stderr:?}"
    );
}

/// A self-deleting temp file under the test runner's temp dir. Avoids a
/// dev-dependency for a one-write-one-read fixture.
struct TempFile {
    path: std::path::PathBuf,
    file: std::fs::File,
}

fn tempfile() -> TempFile {
    // CARGO_TARGET_TMPDIR is per-test-binary and cleaned by cargo; the pid
    // keeps parallel test processes from colliding on the name.
    let mut path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    path.push(format!("pouncer-input-{}.txt", std::process::id()));
    let file = std::fs::File::create(&path).expect("create temp file");
    TempFile { path, file }
}

impl TempFile {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.write_all(bytes)?;
        self.file.flush()
    }
    fn path_string(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn a_scroll_the_input_itself_requests_is_not_reported_as_loss() {
    // `CSI 1 S` scrolls a blank row off a grid that holds all of the input.
    let (stdout, stderr, ok) = run(&["--preset", "url"], b"\x1b[1Shttps://example.com\n");
    assert!(ok);
    assert_eq!(stdout.trim(), "https://example.com");
    assert!(stderr.is_empty(), "no warning: {stderr:?}");
}

#[test]
fn generate_man_prints_a_roff_page_naming_pouncer() {
    let (stdout, _, ok) = run(&["--generate", "man"], b"");
    assert!(ok);
    assert!(stdout.contains(".TH pouncer 1"), "{stdout:?}");
}

#[test]
fn generate_completions_cover_every_supported_shell() {
    // fish declares a long flag by bare name, without the dashes.
    for (kind, flag) in [
        ("complete-bash", "--preset"),
        ("complete-zsh", "--preset"),
        ("complete-fish", "-l preset"),
    ] {
        let (stdout, _, ok) = run(&["--generate", kind], b"");
        assert!(ok, "{kind}");
        assert!(stdout.contains(flag), "{kind} completes flags");
    }
}

#[test]
fn input_that_reaches_the_grid_row_cap_but_fits_reports_no_loss() {
    // Long enough that the grid is sized at the cap, short enough that
    // nothing scrolls off it.
    let mut input = b"x\n".repeat(65_000);
    input.extend(b"kept https://tail.example\n");
    let (stdout, stderr, ok) = run(&["--preset", "url", "--cols", "80"], &input);
    assert!(ok);
    assert_eq!(stdout.trim(), "https://tail.example");
    assert!(stderr.is_empty(), "no warning: {stderr:?}");
}

#[test]
fn generate_into_a_reader_that_already_left_is_not_an_error() {
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    let status = Command::new(env!("CARGO_BIN_EXE_pouncer"))
        .args(["--generate", "man"])
        .stdout(writer)
        .status()
        .expect("run pouncer");
    assert!(status.success(), "{status}");
}

#[cfg(target_os = "linux")]
#[test]
fn generate_into_a_full_device_fails() {
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("open /dev/full");
    let out = Command::new(env!("CARGO_BIN_EXE_pouncer"))
        .args(["--generate", "man"])
        .stdout(full)
        .stderr(Stdio::piped())
        .output()
        .expect("run pouncer");
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("write stdout"),
        "{out:?}"
    );
}

#[test]
fn an_uppercase_alphabet_is_fine_without_an_upper_action() {
    let (stdout, stderr, ok) = run(
        &["--preset", "url", "--alphabet", "abC"],
        b"see https://example.com\n",
    );
    assert!(ok, "{stderr:?}");
    assert_eq!(stdout.trim(), "https://example.com");
}

#[test]
fn help_text() {
    let (short, _, ok) = run(&["-h"], b"");
    assert!(ok);
    insta::assert_snapshot!("short_help", short);
    let (long, _, ok) = run(&["--help"], b"");
    assert!(ok);
    insta::assert_snapshot!("long_help", long);
}

#[test]
fn startup_rejections_name_what_is_wrong() {
    let cases: &[&[&str]] = &[
        &[],
        &["--preset", "nope"],
        &["--regex", "("],
        &["--regex", "a", "--capture", "1"],
        &["--preset", "url", "--alphabet", "aa"],
        &["--preset", "url", "--multi", "--upper-action", "open"],
        &[
            "--preset",
            "url",
            "--upper-action",
            "open",
            "--alphabet",
            "abC",
        ],
        &["--preset", "url", "--hint-bg", "nope"],
        &["--preset", "url", "--cols", "0"],
    ];
    let report: Vec<String> = cases
        .iter()
        .map(|args| {
            let (_, stderr, ok) = run(args, b"x\n");
            assert!(!ok, "{args:?}");
            format!("$ pouncer {}\n{}", args.join(" "), stderr.trim_end())
        })
        .collect();
    insta::assert_snapshot!(report.join("\n\n"));
}

#[test]
fn unique_resolves_repeats_of_one_value_without_the_picker() {
    let (stdout, _, ok) = run(
        &["--preset", "url", "--unique"],
        b"https://example.com and again https://example.com\n",
    );
    assert!(ok);
    assert_eq!(stdout.trim(), "https://example.com");
}
