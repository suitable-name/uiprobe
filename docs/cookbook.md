# uiprobe cookbook

Recipes for situations the README does not cover. Every snippet is a scenario body or a builder
call against the API in `crates/uiprobe/src`; they are not compiled by the doc tests, so
read them next to the rustdoc.

## Secondary windows and dialogs

A window the app creates itself is wrapped with `ctx.attach(name, component)` and then behaves
like a launched one: `ctx.view(&dialog)`, `ctx.sweep(&dialog, "settings open")`. A dialog that
lives inside the main window is a subtree: find its root by id and build a `WindowView` over it
with `WindowView::subtree`, so the sweep checks it against the window size. Both kinds can be
returned from a smoke `restore`; the first window of the list is the main one.

```rust
.scenario("settings", |ctx| {
    let app = ctx.launch(MainWindow::new)?;
    ctx.view(&app).click("Settings")?;
    // `open_settings` is the app's own function that creates and shows the dialog window.
    let dialog = my_app::open_settings().map_err(|e| Finding::simple("launch", e.to_string()))?;
    let settings = ctx.attach("Settings", dialog);
    ctx.sweep(&settings, "settings open");

    // An in-window dialog: a subtree of the main window.
    let view = ctx.view(&app);
    let root = view.find_id("MainWindow::confirm")?;
    let confirm = WindowView::subtree("Confirm", root, view.size);
    ctx.sweep_view(&confirm, "confirm open");
    ctx.shutdown(settings);
    ctx.shutdown(app);
    Ok(())
})
```

## Apps with threads

When the window is built from a larger app handle that owns worker threads, keep the handle
alive with the window: `Attached::keep` stores any value until the window is dropped. Background
work runs in real time while timers run in mock time, so wait with `ctx.wait_for` (a `timeout`
finding on failure) or `settle`, both of which pump. Stop the threads in `Harness::teardown`, which
runs before the profile is deleted; on Linux the harness warns when threads remain.

```rust
use std::time::Duration;

Harness::new()
    .teardown(|| stop_worker_threads())
    .scenario("load", |ctx| {
        let app = ctx.launch(|| build_app().map(|app| app.ui.clone()))?.keep(app_state());
        let loaded = ctx.wait_for("document loaded", Duration::from_secs(5), || is_loaded());
        if loaded {
            settle(100); // lets the last queued callbacks run
        }
        ctx.shutdown(app);
        Ok(())
    })
    .run()
```

## Sticky notices

An error notice that does not hide itself would be reported again after every click of the
smoke pass. Register two per-scenario routines: `ctx.on_error_notice(window, ..)` reads the
notice (kind and text, `None` when nothing shows) and `ctx.clear_notice_with(..)` hides it.
Keep a weak handle of the current window in a shared slot, because `restore` builds a new window
every time. A state that is expected to warn lists the texts in `Smoke::allowed_notices`.

```rust
use std::cell::RefCell;
use std::rc::{Rc, Weak};

type NoticeSlot = Rc<RefCell<Option<Weak<NotesWindow>>>>;

fn register_notice(ctx: &mut Ctx<'_>, slot: &NoticeSlot) {
    let reader = Rc::clone(slot);
    ctx.on_error_notice("NotesWindow", move || {
        let window = reader.borrow().as_ref()?.upgrade()?;
        let kind = window.get_notice_kind().to_string();
        (!kind.is_empty()).then(|| (kind, window.get_notice_text().to_string()))
    });
    let clearer = Rc::clone(slot);
    ctx.clear_notice_with(move || {
        if let Some(window) = clearer.borrow().as_ref().and_then(Weak::upgrade) {
            window.set_notice_kind("".into());
        }
    });
}

// In the smoke restore, remember the new window: *slot.borrow_mut() = Some(window.as_weak());
let smoke = Smoke::new("startup").allowed_notices(["nothing saved"]);
```

## Deny rules from data

The default deny list (`DEFAULT_DENY`) skips controls whose label says delete, quit and the
like. Words your team keeps in a file become rules with `DenyRule::label_owned`; ids use
`DenyRule::id` (or `id_owned`). The list is a public field, so `smoke.deny = Vec::new()` drops
the defaults entirely. If a layer that covers a quarter of the window is not a modal backdrop,
`skip_blocked(false)` stops the pass from leaving the controls under it out.

```rust
let words = std::fs::read_to_string("tests/deny.txt").unwrap_or_default();
let rules: Vec<DenyRule> = words
    .lines()
    .filter(|line| !line.trim().is_empty())
    .map(|line| DenyRule::label_owned(DenyMatch::Contains, line.trim().to_owned(), "from deny.txt"))
    .collect();
let mut smoke = Smoke::new("startup").deny_rules(&rules).skip_blocked(false);
smoke.deny = Vec::new(); // no built-in list: deny.txt is the whole list
```

## Key chords

`ctx.press(&app, "Ctrl+S")` sends Slint `KeyPressed` and `KeyReleased` events to the window:
modifiers first, then the key, released in reverse. The accepted names (`Escape`, `Return`,
`Tab`, `Backspace`, `Delete`, the arrows, `F1`..`F12`, single characters) are in
`crates/uiprobe/src/keys.rs`. The events reach the focused element only, so give the
window a `forward-focus` target or call `focus()` on an element first; otherwise a handler on a
`FocusScope` never fires. A chord that does not parse is a `bad-chord` finding.

```rust
.scenario("shortcuts", |ctx| {
    let app = ctx.launch(MainWindow::new)?;
    // The window forwards focus to its editor (`forward-focus: editor;` in the .slint file).
    ctx.press(&app, "Ctrl+S");
    ctx.press(&app, "Escape");
    ctx.shutdown(app);
    Ok(())
})
```

## Reading report.json in CI

`report.json` is written on every run that does not abort. `findings[]` holds `window`,
`element`, `check`, `detail`, `state` and `repeats`; `scenarios[]` holds `name`, `findings` and
`duration_ms`; `per_check` maps a check name to its count; `notes` and `sections` carry the
text the smoke pass and the coverage gate add; `harness` and `slint` name the versions. The exit
code is 0 clean, 1 findings, 2 aborted. While a baseline is being built, run with
`UIPROBE_ALLOW_FAIL=1` (exit 0 with findings); `UIPROBE_FORMATS=txt` writes a plain
text file that is friendly to a CI log.

```text
$ UIPROBE_FORMATS=txt cargo test --features ui-harness --test ui
$ jq '.per_check' target/uiprobe/my-app/report.json
{ "overlap": 2, "not-found": 1 }
$ jq -r '.findings[] | "\(.check) \(.window): \(.detail) x\(.repeats)"' target/uiprobe/my-app/report.json
```

## Baselines across machines

Geometry depends on text metrics, and the testing backend measures text with the system fonts,
so the same app can differ between your laptop and a CI box (README, Limits). Fixtures that need
exact geometry set `default-font-family: "FixedTestFont"`. For a real app, compare the
`per_check` totals on one machine and keep one baseline per CI box, or install a pinned font
package on the runners.

```toml
# baseline-ci-linux.toml, kept next to the CI job that compares it with report.json
overlap = 0
zero-size = 0
missing-label = 3   # known: icon buttons, tracked in the issue tracker
```

## A custom SmokeHost

`ctx.smoke` fits a state that is "relaunch a window". When the state is something else (a wizard
page, an open document) implement `uiprobe::smoke::SmokeHost` yourself. It has six
required methods (`restore`, `views`, `notice`, `clear_notice`, `main_alive`, `report`) and the
optional `sweep_options`; `uiprobe::smoke::run(&mut host, &smoke)` runs the pass. Inside a
scenario `host.report()` is `ctx.report()`.

```rust
use uiprobe::smoke::SmokeHost;

struct WizardHost<'a, 'b> {
    ctx: &'a mut Ctx<'b>,
    app: Option<Attached>,
}

impl SmokeHost for WizardHost<'_, '_> {
    fn restore(&mut self) -> Result<Vec<WindowView>, String> {
        if let Some(old) = self.app.take() {
            self.ctx.shutdown(old);
        }
        let app = self.ctx.launch(Wizard::new).map_err(|f| f.detail)?;
        self.ctx.view(&app).click("Next").map_err(|f| f.detail)?; // go to page two
        let view = app.view();
        self.app = Some(app);
        Ok(vec![view])
    }
    fn views(&self) -> Vec<WindowView> {
        self.app.iter().map(Attached::view).collect()
    }
    fn notice(&mut self) -> Option<(String, String)> {
        None
    }
    fn clear_notice(&mut self) {}
    fn main_alive(&self) -> bool {
        self.app.as_ref().is_some_and(Attached::is_visible)
    }
    fn report(&mut self) -> &mut Report {
        self.ctx.report()
    }
}

// In a scenario:
let mut host = WizardHost { ctx, app: None };
let stats = uiprobe::smoke::run(&mut host, &Smoke::new("wizard page 2"));
```

## Floating layers and scoped smoke passes

A toast, a floating panel or a popup is drawn above the content on purpose, so its controls
"overlap" whatever lies below. List the layer types in `SweepOptions::floating_layers` (type
name or any base name; the default is `["PopupWindow"]`): a control in a layer is never reported
against a control outside it or in another layer, while two overlapping controls inside one layer
still are.

A smoke pass over an in-window dialog uses a `WindowView::subtree` named `Window/<scope>`. Its
Close button destroys the scope, which is the control doing its job: `scope_may_close(true)`
counts that in `SmokeStats::closed` instead of a `window-invalid` finding (a window without a `/`
in its name going away stays a finding). When the scope has no element to root a view at, run
the pass over the whole window and override `SmokeHost::admits` to admit only the controls the
scope added; `admits` is applied to enumeration, lookup and the structure fingerprint.

```rust
Harness::new().sweep_options(SweepOptions {
    floating_layers: vec!["Toast".into(), "PopupWindow".into(), "HelpPanel".into()],
    ..SweepOptions::default()
})

let smoke = Smoke::new("confirm dialog").scope_may_close(true);

// in a custom SmokeHost
fn admits(&self, control: &Control) -> bool {
    control.id.starts_with("ConfirmDialog::")
}
```

## Filtering scenarios while developing

`UIPROBE_FILTER=open,save` runs the scenarios whose names contain `open` or `save`;
positional arguments that do not start with `-` filter the same way. Under a filter the coverage
gate is skipped, because the scenarios that would reach the rest did not run; give the gate
`GateRule::Always` to run it anyway. Beware `cargo test <name>`: cargo hands `<name>` to the
harness binary as a scenario filter, and when nothing matches the run does nothing and writes
no report (README, Troubleshooting).

```text
$ UIPROBE_FILTER=open,save cargo test --features ui-harness --test ui
uiprobe 0.18.8 (Slint 1.18.1): 2 of 7 scenario(s) selected, report dir target/uiprobe/my-app
```

## Speed up a long suite

A suite with many scenarios and long waits finishes sooner when scenarios run in several worker
processes. No code change is needed to try it:

```text
$ UIPROBE_JOBS=auto cargo test --features ui-harness --test ui
```

`auto` starts one worker and adds one every 5 seconds while cores are idle and scenarios are
waiting; `UIPROBE_JOBS=4` starts four at once. For an app that is heavy on the GPU or on
memory, cap the workers so they do not slow each other down:

```text
$ UIPROBE_JOBS=auto UIPROBE_JOBS_MAX=2 cargo test --features ui-harness --test ui
```

To make it the suite's default, set it in code (the environment variables still override it):

```rust
Harness::new()
    .jobs(Jobs::Auto)
    .max_jobs(2)
```

The report is the same as a serial run's (plus one note naming the workers used), and the
console prints each scenario's output in scenario order. Every worker re-runs `main`, so `main`
must build the same harnesses in the same order and do nothing before `Harness::new()` that must
not run twice (README, Parallel runs).

## Finding where a slow run spends its time

A run that takes hours with almost no CPU or memory is waiting. Every scenario prints a `timing:`
line under its `-> N finding(s), M ms` line, and the report has a "Timing" section; the labels
`wait`, `settle` and `pump` come from the backend:

```text
uiprobe:   -> 0 finding(s), 839000 ms
uiprobe:   timing: wait 790.0 s (4120x, 38 timed out 380.0 s), settle 20.1 s (900x)
```

Many timed-out waits mean a condition that never becomes true (a handle that is never released, a
window that never closes), and each one costs its full timeout. Give the suspicious waits their
own label and run again:

```rust
let released = wait_until_labeled("db-release", Duration::from_secs(10), || db_free());
let app = timing::time("relaunch", || launch_again(ctx));
timing::record("profile-delete", started.elapsed());
```

Read the section in `report.md`: the slowest scenarios with their three biggest labels show which
scenario to open first, the totals show which label to fix across the suite, and the timeouts per
label show how much of the run was spent giving up. For a parallel run the last paragraph gives
the workers and the utilisation; a low value with a few huge scenarios means the wall time is
bounded by those scenarios, not by the worker count, so split them. Work that must see the
coverage of the others belongs in `Harness::scenario_last`, which runs when the ordinary
scenarios are done and, in a parallel run, receives every worker's coverage.

## Guarded paths for apps that use the dirs crate

The profile redirects `APPDATA` and the XDG variables, but on Windows the `dirs` crate asks the
Known Folders API, which ignores them. Under the feature, ask the harness first with
`uiprobe_hooks::paths::data_dir()` and fall back to the real lookup otherwise, in one
function that the app and the guard share. Then hand that function to the profile: a path that
still resolves outside the profile aborts the run with exit code 2 before a window exists.

```rust
pub fn settings_path() -> PathBuf {
    #[cfg(feature = "ui-harness")]
    if let Some(dir) = uiprobe_hooks::paths::data_dir() {
        return dir.join("settings.txt");
    }
    dirs::config_dir().unwrap_or_default().join("my-app").join("settings.txt")
}

// tests/ui.rs
Harness::new()
    .profile(Profile::isolated().guard_paths(|| vec![my_app::settings_path()]))
```
