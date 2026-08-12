use std::fmt;
use std::sync::{Mutex, TryLockError};

static WORKSPACE_PICKER_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, PartialEq, Eq)]
pub enum PickerOutcome {
    Selected(String),
    Cancelled,
}

#[derive(Debug)]
pub enum PickerError {
    Unavailable,
    Busy,
    Launch(std::io::Error),
    Script(String),
    EmptySelection,
}

impl fmt::Display for PickerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("native folder picker is unavailable"),
            Self::Busy => formatter.write_str("folder picker is already open"),
            Self::Launch(error) => write!(formatter, "failed to launch folder picker: {error}"),
            Self::Script(error) => write!(formatter, "folder picker failed: {error}"),
            Self::EmptySelection => formatter.write_str("folder picker returned an empty path"),
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn workspace_picker_script() -> &'static str {
    r#"set selectedFolder to choose folder with prompt "Choose a folder for this GitIM workspace. Use New Folder to create an empty folder."
POSIX path of selectedFolder"#
}

#[cfg(any(target_os = "macos", test))]
fn interpret_osascript_output(
    success: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> Result<PickerOutcome, String> {
    if !success {
        let error = String::from_utf8_lossy(stderr);
        if error.contains("(-128)") {
            return Ok(PickerOutcome::Cancelled);
        }
        return Err(error.trim().to_string());
    }

    let path = String::from_utf8_lossy(stdout);
    let path = path.trim_end_matches(['\r', '\n']);
    let normalized = if path == "/" {
        path
    } else {
        path.trim_end_matches('/')
    };
    if normalized.is_empty() {
        return Err("folder picker returned an empty path".to_string());
    }
    Ok(PickerOutcome::Selected(normalized.to_string()))
}

fn pick_workspace_directory_with_lock<F>(
    lock: &Mutex<()>,
    picker: F,
) -> Result<PickerOutcome, PickerError>
where
    F: FnOnce() -> Result<PickerOutcome, PickerError>,
{
    let _guard = match lock.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::WouldBlock) => return Err(PickerError::Busy),
        Err(TryLockError::Poisoned(error)) => error.into_inner(),
    };
    picker()
}

#[cfg(target_os = "macos")]
pub fn pick_workspace_directory() -> Result<PickerOutcome, PickerError> {
    pick_workspace_directory_with_lock(&WORKSPACE_PICKER_LOCK, || {
        let output = std::process::Command::new("/usr/bin/osascript")
            .arg("-e")
            .arg(workspace_picker_script())
            .output()
            .map_err(PickerError::Launch)?;

        interpret_osascript_output(output.status.success(), &output.stdout, &output.stderr).map_err(
            |error| {
                if error == "folder picker returned an empty path" {
                    PickerError::EmptySelection
                } else {
                    PickerError::Script(error)
                }
            },
        )
    })
}

#[cfg(not(target_os = "macos"))]
pub fn pick_workspace_directory() -> Result<PickerOutcome, PickerError> {
    pick_workspace_directory_with_lock(&WORKSPACE_PICKER_LOCK, || Err(PickerError::Unavailable))
}

#[cfg(test)]
mod tests {
    use super::{
        interpret_osascript_output, pick_workspace_directory_with_lock, workspace_picker_script,
        PickerError, PickerOutcome,
    };
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };

    #[test]
    fn selected_directory_is_returned_without_macos_trailing_slash() {
        let result = interpret_osascript_output(true, b"/Users/dev/Workspaces/team-alpha/\n", b"")
            .expect("selection should parse");

        assert_eq!(
            result,
            PickerOutcome::Selected("/Users/dev/Workspaces/team-alpha".into())
        );
    }

    #[test]
    fn filesystem_root_keeps_its_slash() {
        let result =
            interpret_osascript_output(true, b"/\n", b"").expect("root selection should parse");

        assert_eq!(result, PickerOutcome::Selected("/".into()));
    }

    #[test]
    fn user_cancel_is_not_an_error() {
        let result =
            interpret_osascript_output(false, b"", b"execution error: User canceled. (-128)\n")
                .expect("cancel should parse");

        assert_eq!(result, PickerOutcome::Cancelled);
    }

    #[test]
    fn other_osascript_failures_are_reported() {
        let error = interpret_osascript_output(
            false,
            b"",
            b"execution error: Not authorized to send Apple events. (-1743)\n",
        )
        .expect_err("permission failure should be returned");

        assert!(error.contains("Not authorized"));
    }

    #[test]
    fn picker_uses_the_macos_folder_dialog_with_empty_folder_guidance() {
        let script = workspace_picker_script();

        assert!(script.contains("choose folder"));
        assert!(script.contains("New Folder"));
        assert!(script.contains("POSIX path"));
    }

    #[test]
    fn concurrent_picker_request_returns_busy_without_opening_another_dialog() {
        let lock = Mutex::new(());
        let held = lock.lock().unwrap();
        let invoked = AtomicBool::new(false);

        let error = pick_workspace_directory_with_lock(&lock, || {
            invoked.store(true, Ordering::SeqCst);
            Ok(PickerOutcome::Cancelled)
        })
        .expect_err("second picker should be rejected");

        assert!(matches!(error, PickerError::Busy));
        assert!(!invoked.load(Ordering::SeqCst));
        drop(held);
    }
}
