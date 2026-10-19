//! A small notes app driven by `uiprobe`: one window with a title field, the buttons a
//! real app has (open, save as, copy, help, clear, delete, quit), an Autosave switch that
//! writes a settings file, and an error notice. Every seam of `uiprobe-hooks` is used
//! under the `ui-harness` feature: file dialogs are answered from the harness's queue,
//! clipboard writes and opened URLs are captured, and the settings file lives in the
//! harness's data directory. With the feature off the same functions reach the OS instead;
//! here they are stubs (this example ships no dialog, clipboard or opener crate), so the app
//! never opens a real dialog or touches the network. The test in `tests/ui.rs` drives all of
//! it.
//!
//! The pattern to copy: one function per seam, the feature-on arm first, the feature-off arm
//! the real thing, both with one signature, so every call site is identical in both builds.

use slint::ComponentHandle;
use std::{fs, path::PathBuf};

slint::include_modules!();

/// The page the Help button opens. Under the harness it is captured, never opened.
pub const HELP_URL: &str = "https://example.com/notes/help";

/// The settings file name inside the configuration directory.
const SETTINGS_FILE: &str = "notes-settings.txt";

/// Builds the window with every global callback wired to its Rust handler and the Autosave
/// setting loaded. The harness launches it with `ctx.launch(notes::build)`.
///
/// # Errors
///
/// Slint's platform error when the window cannot be created.
pub fn build() -> Result<NotesWindow, slint::PlatformError> {
    let window = NotesWindow::new()?;
    window.set_autosave(load_autosave());
    wire_file_callbacks(&window);
    wire_other_callbacks(&window);
    Ok(window)
}

/// Open, Save As and Delete Note: the callbacks that go through a file dialog or a file.
fn wire_file_callbacks(window: &NotesWindow) {
    let model = window.global::<Model>();

    let weak = window.as_weak();
    model.on_open_note(move || {
        let Some(window) = weak.upgrade() else { return };
        match pick_note_to_open() {
            Some(path) => match fs::read_to_string(&path) {
                Ok(text) => {
                    window.set_note_title(text.trim().into());
                    window.set_status(format!("opened {}", path.display()).into());
                }
                Err(e) => show_notice(
                    &window,
                    "error",
                    &format!("cannot open {}: {e}", path.display()),
                ),
            },
            None => window.set_status("open cancelled".into()),
        }
    });

    let weak = window.as_weak();
    model.on_save_note(move || {
        let Some(window) = weak.upgrade() else { return };
        match pick_save_path() {
            Some(path) => match fs::write(&path, window.get_note_title().as_str()) {
                Ok(()) => {
                    window.set_saved_path(path.display().to_string().into());
                    window.set_status(format!("saved as {}", path.display()).into());
                }
                Err(e) => show_notice(
                    &window,
                    "error",
                    &format!("cannot save {}: {e}", path.display()),
                ),
            },
            None => show_notice(&window, "warning", "nothing saved"),
        }
    });

    let weak = window.as_weak();
    model.on_delete_note(move || {
        let Some(window) = weak.upgrade() else { return };
        let saved = window.get_saved_path();
        if saved.is_empty() {
            window.set_status("nothing to delete".into());
            return;
        }
        match fs::remove_file(saved.as_str()) {
            Ok(()) => {
                window.set_saved_path("".into());
                window.set_note_title("".into());
                window.set_status(format!("deleted {saved}").into());
            }
            Err(e) => show_notice(&window, "error", &format!("cannot delete {saved}: {e}")),
        }
    });
}

/// Copy Title, Help, Clear, Quit and the Autosave switch.
fn wire_other_callbacks(window: &NotesWindow) {
    let model = window.global::<Model>();

    let weak = window.as_weak();
    model.on_copy_title(move || {
        let Some(window) = weak.upgrade() else { return };
        copy_to_clipboard(window.get_note_title().as_str());
        window.set_status("title copied".into());
    });

    let weak = window.as_weak();
    model.on_open_help(move || {
        let Some(window) = weak.upgrade() else { return };
        open_in_browser(HELP_URL);
        window.set_status("help opened".into());
    });

    let weak = window.as_weak();
    model.on_clear_note(move || {
        let Some(window) = weak.upgrade() else { return };
        window.set_note_title("".into());
        window.set_status("cleared".into());
    });

    let weak = window.as_weak();
    model.on_quit(move || {
        if let Some(window) = weak.upgrade() {
            let _ = window.hide();
        }
    });

    let weak = window.as_weak();
    model.on_autosave_changed(move |on| {
        let Some(window) = weak.upgrade() else { return };
        match save_autosave(on) {
            Ok(()) => {
                let state = if on { "on" } else { "off" };
                window.set_status(format!("autosave {state}").into());
            }
            Err(e) => show_notice(&window, "error", &format!("cannot write settings: {e}")),
        }
    });
}

/// Shows the notice row: `kind` is `error`, `warning` or `info`.
fn show_notice(window: &NotesWindow, kind: &str, text: &str) {
    window.set_notice_kind(kind.into());
    window.set_notice_text(text.into());
}

/// Where the settings file lives.
///
/// Under the harness feature the harness's throw-away data directory comes first, because a
/// lookup through the OS (Windows Known Folders) ignores the environment the harness redirects;
/// the fallback is the environment lookup below. The test hands this path to
/// `Profile::guard_paths`, so a run aborts if it ever resolves outside the profile.
#[must_use]
pub fn settings_path() -> PathBuf {
    #[cfg(feature = "ui-harness")]
    if let Some(dir) = uiprobe_hooks::paths::data_dir() {
        return dir.join(SETTINGS_FILE);
    }
    config_dir().join(SETTINGS_FILE)
}

/// The configuration directory from the environment: `APPDATA` (Windows), else
/// `XDG_CONFIG_HOME`, else `HOME/.config`. The harness redirects all three into its profile.
fn config_dir() -> PathBuf {
    for var in ["APPDATA", "XDG_CONFIG_HOME"] {
        if let Some(dir) = std::env::var_os(var) {
            return PathBuf::from(dir);
        }
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".config")
}

/// Reads the Autosave setting; a missing or unreadable file means off.
fn load_autosave() -> bool {
    fs::read_to_string(settings_path()).is_ok_and(|text| text.contains("autosave=true"))
}

/// Writes the Autosave setting.
fn save_autosave(on: bool) -> std::io::Result<()> {
    let path = settings_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("autosave={on}\n"))
}

/// The Open dialog. Feature on: the harness's queued answer; off: the native dialog.
fn pick_note_to_open() -> Option<PathBuf> {
    #[cfg(feature = "ui-harness")]
    {
        uiprobe_hooks::picker::next("Open Note")
    }
    #[cfg(not(feature = "ui-harness"))]
    {
        native_dialog("Open Note")
    }
}

/// The Save As dialog, same shape.
fn pick_save_path() -> Option<PathBuf> {
    #[cfg(feature = "ui-harness")]
    {
        uiprobe_hooks::picker::next("Save Note As")
    }
    #[cfg(not(feature = "ui-harness"))]
    {
        native_dialog("Save Note As")
    }
}

/// Stands in for the native file dialog (for example `rfd::FileDialog`), which this example
/// does not ship: without the harness the app behaves as if the user cancelled.
#[cfg(not(feature = "ui-harness"))]
fn native_dialog(title: &str) -> Option<PathBuf> {
    eprintln!("notes: no native dialog in this example ({title}), treated as cancelled");
    None
}

/// The clipboard write. Feature on: captured by the harness; off: the system clipboard (a
/// stand-in here).
fn copy_to_clipboard(text: &str) {
    #[cfg(feature = "ui-harness")]
    if uiprobe_hooks::clipboard::capture(text) {
        return;
    }
    eprintln!("notes: would copy {text:?} to the system clipboard (no clipboard crate here)");
}

/// The OS opener. Feature on: captured by the harness; off: the browser (a stand-in here).
fn open_in_browser(url: &str) {
    #[cfg(feature = "ui-harness")]
    if uiprobe_hooks::opener::capture(url) {
        return;
    }
    eprintln!("notes: would open {url} in the browser (no opener crate here)");
}
