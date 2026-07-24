//! All drawing. Color carries the state: green = holding locks,
//! yellow = deliberately paused, red = wanted locks but failed.

use crate::app::{App, Mode};
use crate::art;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let active = app.active();
    let pulse_bright = app.pulse && active && app.tick % 4 < 2;
    let art_style = if !active {
        Style::new().fg(Color::DarkGray)
    } else if pulse_bright {
        Style::new().fg(Color::LightGreen).bold()
    } else {
        Style::new().fg(Color::Green).bold()
    };

    let mut lines: Vec<Line> = Vec::new();
    match art::pick(area.width, area.height) {
        Some(rows) => lines.extend(rows.into_iter().map(|r| Line::styled(r, art_style))),
        None => lines.push(Line::styled("I CAN'T GET NO SLEEP", art_style)),
    }
    lines.push(Line::default());

    let (label, style) = banner(app);
    lines.push(Line::styled(label, style));

    match (&app.guard, &app.inhibit_err) {
        (Some(g), _) => {
            lines.push(Line::styled(
                format!("inhibiting: {}", g.status().describe()),
                Style::new().fg(Color::Gray),
            ));
            for note in &g.status().notes {
                lines.push(Line::styled(
                    format!("note: {note}"),
                    Style::new().fg(Color::Yellow),
                ));
            }
        }
        (None, Some(e)) => {
            lines.push(Line::styled(
                format!("error: {e}"),
                Style::new().fg(Color::Red),
            ));
        }
        (None, None) => {
            lines.push(Line::styled(
                "inhibitors released",
                Style::new().fg(Color::DarkGray),
            ));
        }
    }

    lines.push(Line::styled(
        app.power.describe(),
        Style::new().fg(Color::Gray),
    ));
    lines.push(Line::default());
    lines.push(footer(app));

    let text = Text::from(lines);
    let h = (text.height() as u16).min(area.height);
    let [centered] = Layout::vertical([Constraint::Length(h)])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(Paragraph::new(text).alignment(Alignment::Center), centered);
}

fn banner(app: &App) -> (String, Style) {
    match (app.active(), app.mode, app.power.on_ac) {
        (true, Mode::Always, _) => (
            " ● AWAKE — ALWAYS ".into(),
            Style::new().bg(Color::Green).fg(Color::Black).bold(),
        ),
        (true, Mode::PluggedOnly, _) => (
            " ● AWAKE — WHILE PLUGGED IN ".into(),
            Style::new().bg(Color::Green).fg(Color::Black).bold(),
        ),
        (false, Mode::PluggedOnly, false) => (
            " ◌ PAUSED — ON BATTERY ".into(),
            Style::new().bg(Color::Yellow).fg(Color::Black).bold(),
        ),
        (false, _, _) => (
            " ✗ NOT INHIBITING ".into(),
            Style::new().bg(Color::Red).fg(Color::White).bold(),
        ),
    }
}

fn footer(app: &App) -> Line<'static> {
    let key = Style::new().fg(Color::White).bold();
    let dim = Style::new().fg(Color::DarkGray);
    let mut spans = vec![
        Span::styled("[m]", key),
        Span::styled(format!(" mode: {}   ", app.mode.label()), dim),
        Span::styled("[l]", key),
        Span::styled(
            format!(" lid block: {}   ", if app.lid { "on" } else { "off" }),
            dim,
        ),
        Span::styled("[q]", key),
        Span::styled(" quit", dim),
    ];
    if let Some(note) = &app.tray_note {
        spans.push(Span::styled(format!("   · {note}"), dim));
    }
    Line::from(spans)
}
