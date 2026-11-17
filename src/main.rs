//! tracemap — cyber-recon traceroute TUI.
//!
//! Entry point: argument parsing, terminal setup (ratatui + crossterm) and
//! the tokio event loop multiplexing keyboard input, network events and the
//! animation ticker.

mod app;
mod models;
mod network;
mod ui;

use std::io;
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind};
use futures::StreamExt;
use ratatui::DefaultTerminal;

use app::App;

const USAGE: &str = "\
{prog} — cyber-recon traceroute TUI

Usage: {prog} <hostname|ip>

Examples:
  {prog} google.com
  {prog} 1.1.1.1

Raw sockets require root or capabilities:
  sudo {prog} google.com
  sudo setcap cap_net_raw,cap_net_admin+ep ./{prog}
";

fn main() -> anyhow::Result<()> {
    // Show the invoked command name (tracemap, netrecorn, …) in usage.
    let prog = std::env::args()
        .next()
        .and_then(|p| {
            std::path::Path::new(&p)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "tracemap".to_owned());
    let usage = USAGE.replace("{prog}", &prog);
    let Some(target) = std::env::args().nth(1) else {
        eprintln!("{usage}");
        std::process::exit(1);
    };
    if target.starts_with('-') {
        eprintln!("{usage}");
        std::process::exit(1);
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    Ok(runtime.block_on(async move {
        let mut app = App::new(target);
        app.start_run();

        // init() enables raw mode + alternate screen and installs a panic
        // hook that restores the terminal; restore() undoes it on exit.
        let mut terminal = ratatui::init();
        let result = run(&mut terminal, &mut app).await;
        ratatui::restore();
        app.shutdown();
        result
    })?)
}

async fn run(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut keys = EventStream::new();
    let mut ticker = tokio::time::interval(Duration::from_millis(80));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        terminal.draw(|frame| ui::render(frame, app))?;

        tokio::select! {
            maybe_event = keys.next() => match maybe_event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => app.shutdown(),
                    KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                    KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
                    KeyCode::Home => app.select_first(),
                    KeyCode::End => app.select_last(),
                    KeyCode::Char('e') => match app.export_json() {
                        Ok(filename) => app.set_notice(format!("exported → {filename}")),
                        Err(err) => app.set_notice(format!("export failed: {err}")),
                    },
                    KeyCode::Char('r') => app.start_run(),
                    _ => {}
                },
                Some(Ok(_)) => {} // resize / mouse / focus: redraw next tick
                Some(Err(err)) => return Err(err),
                None => {}
            },
            maybe_net = app.rx.recv() => {
                if let Some(event) = maybe_net {
                    app.handle_net(event);
                }
            }
            _ = ticker.tick() => app.tick(),
        }

        if app.should_quit {
            return Ok(());
        }
    }
}
