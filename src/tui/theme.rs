// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;

// Graphite surfaces, cyan controls, and semantic status accents. Text has at
// least 7:1 contrast on SURFACE and 4.5:1 on SURFACE_RAISED. Every state also
// has a text label; FAINT is for separators, never supporting text.
pub(super) const BACKGROUND: Color = Color::Rgb(14, 18, 23);
pub(super) const INK: Color = Color::Rgb(232, 238, 245);
pub(super) const MUTED: Color = Color::Rgb(160, 175, 193);
pub(super) const FAINT: Color = Color::Rgb(58, 72, 87);
pub(super) const SURFACE: Color = Color::Rgb(23, 29, 36);
pub(super) const SURFACE_RAISED: Color = Color::Rgb(36, 58, 73);
pub(super) const MINT: Color = Color::Rgb(136, 205, 174);
pub(super) const BLUE: Color = Color::Rgb(114, 196, 221);
pub(super) const AMBER: Color = Color::Rgb(237, 190, 112);
pub(super) const CORAL: Color = Color::Rgb(239, 147, 139);
pub(super) const ORCHID: Color = Color::Rgb(146, 174, 192);
/// The one accent reserved for model output, so AI text is never mistaken for a measurement.
pub(super) const VIOLET: Color = Color::Rgb(195, 176, 238);

pub(super) fn panel<'a>(app: &App, title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(app.color(FAINT)))
        .style(Style::default().bg(app.color(SURFACE)))
        .title(title)
        .title_style(Style::default().fg(app.color(MUTED)))
        .padding(Padding::horizontal(1))
}

pub(super) fn popup_panel<'a>(app: &App, title: impl Into<Line<'a>>, accent: Color) -> Block<'a> {
    panel(app, title)
        .border_style(Style::default().fg(app.color(accent)))
        .shadow(
            Shadow::dark_shade()
                .style(Style::default().fg(app.color(FAINT)))
                .offset(Offset::new(1, 1)),
        )
}

pub(super) fn selected_row_style(app: &App) -> Style {
    Style::default()
        .fg(app.color(INK))
        .bg(app.color(SURFACE_RAISED))
        .add_modifier(Modifier::BOLD)
}

pub(super) fn keycap<'a>(
    app: &App,
    key: impl Into<String>,
    label: impl Into<String>,
) -> Vec<Span<'a>> {
    vec![
        Span::styled(
            key.into(),
            Style::default()
                .fg(app.color(BLUE))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {}   ", label.into()),
            Style::default().fg(app.color(MUTED)),
        ),
    ]
}

pub(super) fn render_command_bar(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    commands: &[(&str, &str)],
) {
    let mut spans = vec![Span::raw(" ")];
    let mut occupied = 1;
    for (key, label) in commands {
        let needed = key.chars().count() + label.chars().count() + 4;
        if occupied + needed > area.width as usize {
            break;
        }
        occupied += needed;
        spans.extend(keycap(app, *key, *label));
    }
    if area.width as usize > occupied + 12 {
        spans.extend(keycap(app, "?", "help"));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(app.color(FAINT))),
        ),
        area,
    );
}

pub(super) fn label<'a>(app: &App, value: impl Into<String>) -> Line<'a> {
    Line::from(Span::styled(
        value.into(),
        Style::default().fg(app.color(MUTED)),
    ))
}

pub(super) fn heading<'a>(app: &App, value: impl Into<String>) -> Line<'a> {
    Line::from(Span::styled(
        value.into(),
        Style::default()
            .fg(app.color(INK))
            .add_modifier(Modifier::BOLD),
    ))
}

pub(super) fn disk_usage_color(app: &App, ratio: f64) -> Color {
    app.color(if ratio >= 0.95 {
        CORAL
    } else if ratio >= 0.85 {
        AMBER
    } else {
        MINT
    })
}
