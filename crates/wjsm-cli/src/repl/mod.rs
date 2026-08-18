//! REPL 行编辑：TTY 走 raw mode，非 TTY 保持 `read_line`。

mod buffer;
mod editor;
mod history;
mod keys;
mod terminal;
mod width;

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use anyhow::Result;

use self::editor::{Editor, ReadOutcome};
use self::terminal::RawModeGuard;

/// 与历史文档一致的提示符。
pub(crate) const PROMPT: &str = "wjsm> ";

pub(crate) fn run(
    eval: Option<&str>,
    mut evaluate: impl FnMut(&str) -> Result<ExitCode>,
) -> Result<ExitCode> {
    if let Some(code) = eval {
        return evaluate(code);
    }
    if can_use_raw_editor() {
        run_raw(&mut evaluate)
    } else {
        run_fallback(&mut evaluate)
    }
}

fn can_use_raw_editor() -> bool {
    cfg!(unix) && io::stdin().is_terminal() && io::stdout().is_terminal()
}

fn run_raw(evaluate: &mut impl FnMut(&str) -> Result<ExitCode>) -> Result<ExitCode> {
    let _raw = RawModeGuard::enter()?;
    let mut editor = Editor::new();
    loop {
        match editor.read_line(&mut io::stdin(), &mut io::stdout())? {
            ReadOutcome::Eof => break,
            ReadOutcome::Line(line) => {
                if !dispatch_line(&line, evaluate)? {
                    break;
                }
            }
        }
    }
    Ok(ExitCode::from(0))
}

fn run_fallback(evaluate: &mut impl FnMut(&str) -> Result<ExitCode>) -> Result<ExitCode> {
    let mut line = String::new();
    loop {
        if io::stdin().is_terminal() {
            print!("{PROMPT}");
            io::stdout().flush()?;
        }
        line.clear();
        if io::stdin().read_line(&mut line)? == 0 {
            break;
        }
        if !dispatch_line(&line, evaluate)? {
            break;
        }
    }
    Ok(ExitCode::from(0))
}

fn dispatch_line(line: &str, evaluate: &mut impl FnMut(&str) -> Result<ExitCode>) -> Result<bool> {
    let source = line.trim();
    if source.is_empty() {
        return Ok(true);
    }
    if matches!(source, ".exit" | ".quit") {
        return Ok(false);
    }
    if let Err(error) = evaluate(source) {
        eprintln!("Error: {error:#}");
    }
    Ok(true)
}
