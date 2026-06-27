//! Terminal UI entrypoint for studiofs-bench.

use std::{
    io::{self, IsTerminal},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use studiofs_bench::{
    BenchmarkConfig, BenchmarkRunner, BenchmarkRunnerReport, TerminalUi, UiAction,
};

fn main() -> io::Result<()> {
    if !io::stdout().is_terminal() {
        println!("studiofs-bench");
        return Ok(());
    }

    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal) -> io::Result<()> {
    let mut ui = TerminalUi::default();
    let mut running: Option<RunningBenchmark> = None;

    while !ui.should_exit() {
        if let Some(run) = &running
            && let Ok(result) = run.done.try_recv()
        {
            ui.finish_run(match result {
                Ok(report) if report.stopped => String::from("Stopped"),
                Ok(report) => format!("Done - {} passes", report.passes.len()),
                Err(error) => format!("Error - {error}"),
            });
            running = None;
        }

        terminal.draw(|frame| ui.render(frame))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        let was_running = ui.is_running();
        if let Some(action) = key_action(key.code) {
            ui.handle_action(action);
        }

        if was_running && !ui.is_running() {
            if let Some(run) = &running {
                run.stop.store(true, Ordering::Relaxed);
            }
        } else if !was_running && ui.is_running() {
            running = Some(spawn_benchmark(ui.config().clone()));
        }
    }

    Ok(())
}

fn key_action(code: KeyCode) -> Option<UiAction> {
    match code {
        KeyCode::Up => Some(UiAction::MoveUp),
        KeyCode::Down => Some(UiAction::MoveDown),
        KeyCode::Left => Some(UiAction::PreviousValue),
        KeyCode::Right => Some(UiAction::NextValue),
        KeyCode::Enter => Some(UiAction::Submit),
        KeyCode::Esc => Some(UiAction::Cancel),
        KeyCode::Backspace => Some(UiAction::Backspace),
        KeyCode::Char(value) => Some(UiAction::InsertText(value.to_string())),
        _ => None,
    }
}

struct RunningBenchmark {
    stop: Arc<AtomicBool>,
    done: Receiver<Result<BenchmarkRunnerReport, String>>,
}

fn spawn_benchmark(config: BenchmarkConfig) -> RunningBenchmark {
    let stop = Arc::new(AtomicBool::new(false));
    let should_stop = Arc::clone(&stop);
    let (done_tx, done) = mpsc::channel();

    thread::spawn(move || {
        let result = BenchmarkRunner::default()
            .run(&config, |_| {}, || should_stop.load(Ordering::Relaxed))
            .map_err(|error| error.to_string());
        let _ = done_tx.send(result);
    });

    RunningBenchmark { stop, done }
}
