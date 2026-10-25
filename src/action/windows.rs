#![allow(unsafe_code)]

use super::argv;
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
};
use windows_sys::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub(super) fn run_command(template: &str, values: &[String]) -> Result<ExitStatus> {
    let words = argv::expand(&argv::split(template), values);
    let Some((program, args)) = words.split_first() else {
        bail!("--action is empty");
    };
    let Some(path) = resolve(program) else {
        bail!("--action program {program:?} was not found on PATH");
    };
    // std tries the path with `.exe` appended before the path itself, so it
    // would run `probe.cmd.exe` in place of the `probe.cmd` resolved here.
    let is_exe = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"));
    let mut twin = path.clone().into_os_string();
    twin.push(".exe");
    if !is_exe && Path::new(&twin).exists() {
        bail!(
            "--action program {} has a {} beside it that would run instead; refusing to guess",
            path.display(),
            Path::new(&twin).display()
        );
    }
    match Command::new(&path).args(args).status() {
        // std's batch-file quoting refuses what cmd.exe could misread.
        Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => bail!(
            "--action: {} cannot pass these values to a batch file safely",
            path.display()
        ),
        status => status.with_context(|| format!("run --action program {}", path.display())),
    }
}

/// Every name without a directory goes through `PATH` here, never through
/// std's search, which tries the executable's own directory first and
/// appends only `.exe`, so `code` would miss `code.cmd`. The current
/// directory is never searched.
fn resolve(program: &str) -> Option<PathBuf> {
    if program.contains(['\\', '/', ':']) {
        return Some(PathBuf::from(program));
    }
    let as_named = Path::new(program).extension().map(|_| String::new());
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned());
    let suffixes: Vec<String> = as_named
        .into_iter()
        .chain(
            pathext
                .split(';')
                .filter(|ext| !ext.is_empty())
                .map(str::to_owned),
        )
        .collect();
    let path = std::env::var_os("PATH")?;
    // A trailing `;` in PATH is an empty entry, and an empty or relative entry
    // would resolve against the current directory.
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .find_map(|dir| {
            suffixes
                .iter()
                .map(|suffix| dir.join(format!("{program}{suffix}")))
                .find(|candidate| candidate.is_file())
        })
}

pub(super) fn open(values: &[String]) -> Result<()> {
    for value in values {
        shell_open(value).with_context(|| format!("open {value:?} failed"))?;
    }
    Ok(())
}

/// `SEE_MASK_NOASYNC` because pouncer exits right after the call, which
/// Microsoft documents as needing it; `SEE_MASK_FLAG_NO_UI` so a value with no
/// handler fails instead of raising an "open with" dialog.
fn shell_open(value: &str) -> std::io::Result<()> {
    if value.contains('\0') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a NUL cannot be passed to the shell",
        ));
    }
    let file: Vec<u16> = value.encode_utf16().chain([0]).collect();
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpFile: file.as_ptr(),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };
    // SAFETY: the reserved argument must be null; the flags are valid COINIT
    // values. Some handlers ShellExecuteEx reaches are COM objects.
    let com = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    // SAFETY: `info` is a fully initialized SHELLEXECUTEINFOW with `cbSize`
    // set and every pointer field null except `lpFile`, which points at a
    // NUL-terminated UTF-16 buffer that outlives the call. Without
    // SEE_MASK_NOCLOSEPROCESS no handle is returned to close.
    let opened = unsafe { ShellExecuteExW(&raw mut info) } != 0;
    let result = if opened {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    };
    // S_OK and S_FALSE both count as an initialization to balance; a failure
    // such as RPC_E_CHANGED_MODE does not.
    if com >= 0 {
        // SAFETY: balances the successful CoInitializeEx on this thread.
        unsafe { CoUninitialize() };
    }
    result
}
