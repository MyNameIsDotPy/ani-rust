use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind,
        KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Terminal,
};
use std::{
    io::{self, IsTerminal},
    time::Duration,
};

pub fn enabled(plain: bool, debug: bool) -> bool {
    !plain && !debug && io::stdout().is_terminal() && io::stdin().is_terminal()
}
/// Restores terminal state on successful return, errors and cancellation.
pub struct Screen {
    pub(crate) terminal: Terminal<CrosstermBackend<io::Stdout>>,
}
impl Screen {
    pub fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(e) = execute!(stdout, EnterAlternateScreen, EnableBracketedPaste) {
            let _ = disable_raw_mode();
            return Err(e.into());
        }
        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(e) => {
                let _ = disable_raw_mode();
                let _ = execute!(io::stdout(), LeaveAlternateScreen);
                Err(e.into())
            }
        }
    }
    pub fn downloads(&mut self, bars: &[indicatif::ProgressBar]) -> Result<()> {
        self.terminal.draw(|f| {
            let areas = Layout::vertical([
                Constraint::Length(3),
                Constraint::Min(2),
                Constraint::Length(3),
            ])
            .split(f.area());
            let done = bars.iter().filter(|b| b.is_finished()).count();
            f.render_widget(
                Paragraph::new(format!(
                    "ani-rust  /  Descargas    {done}/{} finalizados",
                    bars.len()
                ))
                .style(Style::default().fg(Color::Cyan))
                .block(Block::default().borders(Borders::ALL)),
                areas[0],
            );
            let items: Vec<_> = bars
                .iter()
                .filter(|b| !b.is_finished())
                .chain(bars.iter().filter(|b| b.is_finished()))
                .map(|b| {
                    ListItem::new(format!(
                        "{}   {}/{} segmentos",
                        b.message(),
                        b.position(),
                        b.length().unwrap_or(0)
                    ))
                })
                .collect();
            f.render_widget(
                List::new(items).block(
                    Block::default()
                        .title(" Cola de episodios ")
                        .borders(Borders::ALL),
                ),
                areas[1],
            );
            f.render_widget(
                Paragraph::new("q / Esc / Ctrl-C: cancelar descargas pendientes")
                    .block(Block::default().borders(Borders::ALL)),
                areas[2],
            );
        })?;
        Ok(())
    }
}
impl Drop for Screen {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableBracketedPaste,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}
pub fn cancel_pressed() -> Result<bool> {
    if event::poll(Duration::ZERO)? {
        if let Event::Key(k) = event::read()? {
            if k.kind == KeyEventKind::Press {
                return Ok(matches!(k.code, KeyCode::Esc | KeyCode::Char('q'))
                    || (k.code == KeyCode::Char('c')
                        && k.modifiers.contains(KeyModifiers::CONTROL)));
            }
        }
    }
    Ok(false)
}
