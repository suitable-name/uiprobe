//! The counter under the harness: a geometry sweep at two sizes, a click journey with an
//! expectation, and a click-everything pass. The report goes to the default
//! `<target>/uiprobe/counter`. Run:
//! `cargo test -p counter --features ui-harness --test ui` (the `ui` target requires the
//! feature, as the README recipe does).

use counter::Counter;
use std::process::ExitCode;
use uiprobe::prelude::*;

fn main() -> ExitCode {
    Harness::new()
        .sizes([(320, 200), (240, 160)])
        .scenario("startup", |ctx| {
            let app = ctx.launch(Counter::new)?;
            ctx.sweep_at_sizes(&app, "startup");
            let view = ctx.view(&app);
            view.click("Increment")?;
            view.click("Increment")?;
            pump(20);
            let count = app.component::<Counter>().map_or(-1, Counter::get_count);
            ctx.expect(count == 2, "counter", format!("count is {count}, wanted 2"));
            ctx.shutdown(app);
            Ok(())
        })
        .scenario("smoke", |ctx| {
            ctx.smoke(Smoke::new("startup"), |ctx| {
                Ok(vec![ctx.launch(Counter::new)?])
            });
            Ok(())
        })
        .run()
}
