use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Terminal, backend::CrosstermBackend};
use rustaller::model::InstallPlan;
use std::io;

mod draft;
mod editor;
mod fields;
mod menus;
mod plan;
mod render;
mod state;
use state::{Progress, Wizard};

const PREVIEW: &str =
    "Linux VPS reinstallation · Limine target bootloader · Installation erases the selected disk.";

struct TerminalMode;
impl Drop for TerminalMode {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = disable_raw_mode();
    }
}

pub fn run(
    mut preflight: impl FnMut(&InstallPlan) -> Result<rustaller::host::inspect::PreflightReport, String>,
    mut execute: impl FnMut(&InstallPlan) -> Result<(), String>,
) -> Result<(), String> {
    enable_raw_mode().map_err(|error| error.to_string())?;
    let _mode = TerminalMode;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)
        .map_err(|error| error.to_string())?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).map_err(|error| error.to_string())?;
    terminal.clear().map_err(|error| error.to_string())?;
    let mut wizard = Wizard::new();
    wizard.prefill_hostnames(std::path::Path::new("/"));
    loop {
        terminal
            .draw(|frame| wizard.render(frame))
            .map_err(|error| error.to_string())?;
        match event::read().map_err(|error| error.to_string())? {
            Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                match wizard.key(key) {
                    Progress::Preflight(plan) => {
                        let result = native_terminal(&mut terminal, || preflight(&plan))?;
                        wizard.preflight_complete(result);
                    }
                    Progress::Ready(plan) => {
                        native_terminal(&mut terminal, || {
                            execute_ready(&mut wizard, &mut execute, &plan)
                        })?;
                    }
                    Progress::GenerateKey(options) => {
                        // Restore the real terminal for native passphrase prompts.
                        let result = native_terminal(&mut terminal, || {
                            eprintln!(
                                "rustaller: ssh-keygen will ask for the private key passphrase. An empty passphrase has no KDF protection. Back up the key before installation."
                            );
                            rustaller::host::ssh_keygen::generate(
                                &mut rustaller::command::SystemRunner,
                                &options,
                            )
                            .map_err(|e| e.to_string())
                        })?;
                        wizard.keygen_complete(result);
                    }
                    Progress::Cancel => return Ok(()),
                    Progress::Continue => {}
                }
            }
            Event::Paste(text) => wizard.paste(&text),
            _ => {}
        }
    }
}

fn native_terminal<T>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    action: impl FnOnce() -> T,
) -> Result<T, String> {
    disable_raw_mode().map_err(|e| e.to_string())?;
    execute!(
        io::stdout(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        crossterm::cursor::Show
    )
    .map_err(|e| e.to_string())?;
    let result = action();
    enable_raw_mode().map_err(|e| e.to_string())?;
    execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)
        .map_err(|e| e.to_string())?;
    terminal.clear().map_err(|e| e.to_string())?;
    Ok(result)
}

fn execute_ready(
    wizard: &mut Wizard,
    execute: &mut impl FnMut(&InstallPlan) -> Result<(), String>,
    plan: &InstallPlan,
) {
    wizard.complete(execute(plan));
}

#[cfg(test)]
mod tests;
