//! The notes app under the harness: every seam in one run. Scenarios, in order:
//!
//! 1. `startup`: launch, sweep at both configured sizes, wait for the status, fetch the title
//!    field (a `find_id` counts as "touched" for the coverage gate).
//! 2. `open and save`: a dialog opened with nothing queued is reported back to the scenario, a
//!    queued answer feeds the app, and the note round-trips through a file inside the profile.
//! 3. `notice`: a cancelled Save As raises a warning the harness can read; Dismiss hides it.
//! 4. `copy and help`: clipboard writes and opened URLs are captured, never sent to the OS.
//! 5. `clear and delete`: the controls the smoke pass refuses to click, driven on purpose.
//! 6. `settings`: the Autosave switch writes the settings file, inside the profile.
//! 7. `smoke`: the click-everything pass with a scenario deny word and an expected notice.
//!
//! The coverage gate scans `ui/` and expects every global callback and interactive element to
//! be covered or listed in `tests/coverage_allow.toml`; a scenario that drove a callback
//! through the UI says so with `record_callback`. Reports: `report.md`, `report.html`,
//! `report.txt` and `report.json` under `<target>/uiprobe/notes`. Run:
//! `cargo test -p notes --features ui-harness --test ui`.

use notes::{HELP_URL, NotesWindow, build, settings_path};
use slint::{ComponentHandle, Weak};
use std::{cell::RefCell, fs, process::ExitCode, rc::Rc, time::Duration};
use uiprobe::{coverage::record_callback, prelude::*};

/// The window the notice routines read: set by every launch, so a relaunch during the smoke
/// pass keeps the reader current.
type NoticeSlot = Rc<RefCell<Option<Weak<NotesWindow>>>>;

/// Registers the notice reader and the clear routine (both are per scenario) on `slot`.
fn watch_notice(ctx: &mut Ctx<'_>, slot: &NoticeSlot) {
    let reader = Rc::clone(slot);
    ctx.on_error_notice("NotesWindow", move || {
        let window = reader.borrow().as_ref()?.upgrade()?;
        let kind = window.get_notice_kind();
        if kind.is_empty() {
            return None;
        }
        Some((kind.to_string(), window.get_notice_text().to_string()))
    });
    let clearer = Rc::clone(slot);
    ctx.clear_notice_with(move || {
        if let Some(window) = clearer.borrow().as_ref().and_then(Weak::upgrade) {
            window.set_notice_kind("".into());
            window.set_notice_text("".into());
        }
    });
}

/// Launches the app and points `slot` at the new window.
fn launch(ctx: &mut Ctx<'_>, slot: &NoticeSlot) -> Result<Attached, Finding> {
    let app = ctx.launch(build)?;
    *slot.borrow_mut() = app.component::<NotesWindow>().map(ComponentHandle::as_weak);
    Ok(app)
}

fn window_of(app: &Attached) -> Option<&NotesWindow> {
    app.component::<NotesWindow>()
}

fn status(app: &Attached) -> String {
    window_of(app).map_or_else(String::new, |w| w.get_status().to_string())
}

fn title(app: &Attached) -> String {
    window_of(app).map_or_else(String::new, |w| w.get_note_title().to_string())
}

fn set_title(app: &Attached, text: &str) {
    if let Some(window) = window_of(app) {
        window.set_note_title(text.into());
    }
}

/// The notice showing, as `(kind, text)`.
fn notice_of(app: &Attached) -> Option<(String, String)> {
    let window = window_of(app)?;
    let kind = window.get_notice_kind();
    (!kind.is_empty()).then(|| (kind.to_string(), window.get_notice_text().to_string()))
}

fn startup(ctx: &mut Ctx<'_>) -> Result<(), Finding> {
    let app = ctx.launch(build)?;
    ctx.sweep_at_sizes(&app, "startup");
    let weak = window_of(&app).map(ComponentHandle::as_weak);
    ctx.wait_for("status is ready", Duration::from_millis(500), move || {
        weak.as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|w| w.get_status().as_str() == "ready")
    });
    // Fetching the field counts as "touched" for the coverage gate (nothing types into it).
    let _title_field = ctx.view(&app).find_id("NotesWindow::title-edit")?;
    ctx.shutdown(app);
    Ok(())
}

fn open_and_save(ctx: &mut Ctx<'_>) -> Result<(), Finding> {
    let app = ctx.launch(build)?;
    let view = ctx.view(&app);
    // Nothing queued: the app sees a cancelled dialog, the harness records the failure, and
    // the scenario reads it back (left in place it would be an `unexpected-file-dialog`
    // finding at the end of the scenario).
    view.click("Open...")?;
    pump(20);
    let failures = ctx.hooks().take_picker_failures();
    ctx.expect(
        failures.len() == 1 && failures[0].contains("Open Note"),
        "picker",
        format!("picker failures {failures:?}, wanted one naming `Open Note`"),
    );
    let now = status(&app);
    ctx.expect(
        now == "open cancelled",
        "open",
        format!("status {now:?}, wanted `open cancelled`"),
    );
    // A queued answer: save the note to a file inside the profile, clear it, open it again.
    let path = ctx.scratch_file("shopping.txt");
    set_title(&app, "Shopping");
    ctx.hooks().queue_pick(path.clone());
    view.click("Save As...")?;
    pump(20);
    let saved = fs::read_to_string(&path).is_ok_and(|text| text == "Shopping");
    ctx.expect(
        saved,
        "save",
        format!("{} does not hold the note", path.display()),
    );
    record_callback("Model.save_note");
    set_title(&app, "");
    ctx.hooks().queue_pick(path);
    view.click("Open...")?;
    pump(20);
    let now = title(&app);
    ctx.expect(
        now == "Shopping",
        "open",
        format!("title {now:?} after Open, wanted `Shopping`"),
    );
    record_callback("Model.open_note");
    ctx.shutdown(app);
    Ok(())
}

fn notice(ctx: &mut Ctx<'_>) -> Result<(), Finding> {
    let slot = NoticeSlot::default();
    watch_notice(ctx, &slot);
    let app = launch(ctx, &slot)?;
    let view = ctx.view(&app);
    ctx.hooks().queue_cancel();
    view.click("Save As...")?;
    pump(20);
    let shown = notice_of(&app);
    ctx.expect(
        shown == Some(("warning".to_owned(), "nothing saved".to_owned())),
        "notice",
        format!("notice {shown:?} after a cancelled Save As, wanted warning `nothing saved`"),
    );
    // The notice row is visible now, so its Dismiss button can be found and clicked. Were it
    // still showing at the end of the scenario, the health check would add an `error-toast`.
    view.click("Dismiss")?;
    pump(20);
    let after = notice_of(&app);
    ctx.expect(
        after.is_none(),
        "notice",
        format!("notice {after:?} after Dismiss, wanted none"),
    );
    ctx.shutdown(app);
    Ok(())
}

fn copy_and_help(ctx: &mut Ctx<'_>) -> Result<(), Finding> {
    let app = ctx.launch(build)?;
    let view = ctx.view(&app);
    set_title(&app, "Shopping");
    view.click("Copy Title")?;
    let copied = ctx.hooks().take_clipboard_writes();
    ctx.expect(
        copied == ["Shopping"],
        "clipboard",
        format!("clipboard writes {copied:?}, wanted [\"Shopping\"]"),
    );
    record_callback("Model.copy_title");
    view.click("Help")?;
    let opened = ctx.hooks().take_opened();
    ctx.expect(
        opened == [HELP_URL],
        "opener",
        format!("opened {opened:?}, wanted [{HELP_URL:?}]"),
    );
    record_callback("Model.open_help");
    ctx.shutdown(app);
    Ok(())
}

fn clear_and_delete(ctx: &mut Ctx<'_>) -> Result<(), Finding> {
    let app = ctx.launch(build)?;
    let view = ctx.view(&app);
    set_title(&app, "Shopping");
    view.click("Clear")?;
    let now = title(&app);
    ctx.expect(
        now.is_empty(),
        "clear",
        format!("title {now:?} after Clear, wanted empty"),
    );
    record_callback("Model.clear_note");
    let path = ctx.scratch_file("old.txt");
    set_title(&app, "Old");
    ctx.hooks().queue_pick(path.clone());
    view.click("Save As...")?;
    pump(20);
    view.click("Delete Note")?;
    pump(20);
    ctx.expect(
        !path.exists(),
        "delete",
        format!("{} still exists after Delete Note", path.display()),
    );
    let now = status(&app);
    ctx.expect(
        now.starts_with("deleted "),
        "delete",
        format!("status {now:?}, wanted `deleted ...`"),
    );
    record_callback("Model.delete_note");
    ctx.shutdown(app);
    Ok(())
}

fn settings(ctx: &mut Ctx<'_>) -> Result<(), Finding> {
    let app = ctx.launch(build)?;
    let path = settings_path();
    let inside = ctx.profile().contains(&path);
    ctx.expect(
        inside,
        "settings",
        format!("{} is outside the profile", path.display()),
    );
    ctx.view(&app).click("Autosave")?;
    pump(20);
    let written = fs::read_to_string(&path).is_ok_and(|text| text.contains("autosave=true"));
    ctx.expect(
        written,
        "settings",
        format!("{} does not say autosave=true", path.display()),
    );
    record_callback("Model.autosave_changed");
    ctx.shutdown(app);
    Ok(())
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "a scenario function has the signature `Harness::scenario` asks for"
)]
fn smoke(ctx: &mut Ctx<'_>) -> Result<(), Finding> {
    let slot = NoticeSlot::default();
    watch_notice(ctx, &slot);
    // `clear` is denied for this pass only (it wipes the state mid-pass); `delete` and `quit`
    // are on the default list. A cancelled Save As raises `nothing saved`, which is expected.
    let smoke = Smoke::new("main")
        .deny(["clear"])
        .allowed_notices(["nothing saved"]);
    let stats = ctx.smoke(smoke, move |ctx| Ok(vec![launch(ctx, &slot)?]));
    ctx.expect(
        stats.activated == 5,
        "smoke",
        format!(
            "{} control(s) activated, wanted 5 (Open, Save As, Copy Title, Help, Autosave)",
            stats.activated
        ),
    );
    ctx.expect(
        stats.denied == 3,
        "smoke",
        format!(
            "{} control(s) denied, wanted 3 (Clear, Delete Note, Quit)",
            stats.denied
        ),
    );
    Ok(())
}

fn main() -> ExitCode {
    Harness::new()
        .sizes([(480, 360), (560, 400)])
        .formats([Format::Markdown, Format::Html, Format::Text])
        .profile(Profile::isolated().guard_paths(|| vec![settings_path()]))
        .coverage(Coverage::scan("ui").allow_file("tests/coverage_allow.toml"))
        .scenario("startup", startup)
        .scenario("open and save", open_and_save)
        .scenario("notice", notice)
        .scenario("copy and help", copy_and_help)
        .scenario("clear and delete", clear_and_delete)
        .scenario("settings", settings)
        .scenario("smoke", smoke)
        .run()
}
