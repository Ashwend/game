// Ship the client as a Windows GUI-subsystem binary so double-clicking it never
// allocates/flashes a console window. Release only: dev builds (`./cli dev`,
// `./cli run`) keep the console for logs. The same `ashwend.exe` is also the CLI
// (`server`/`admin`/`multiplayer-test`/`--help`); `crate::console::attach_parent_console`,
// called first thing in `cli::run`, reattaches to the launching terminal when
// there is one so that CLI output is not lost. No effect on non-Windows targets.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> anyhow::Result<()> {
    ashwend::run()
}
