// SPDX-License-Identifier: GPL-3.0-or-later
use super::*;
use unicode_segmentation::UnicodeSegmentation;

/// Fit terminal cells without splitting wide or multi-codepoint characters.
pub(super) fn truncate_middle(value: &str, width: usize) -> String {
    if Span::raw(value).width() <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".into();
    }
    let mut used = 0;
    let prefix: String = value
        .graphemes(true)
        .take_while(|g| {
            used += Span::raw(*g).width();
            used <= (width - 1) / 2
        })
        .collect();
    let remaining = width - 1 - Span::raw(prefix.as_str()).width();
    used = 0;
    let suffix = value
        .graphemes(true)
        .rev()
        .take_while(|g| {
            used += Span::raw(*g).width();
            used <= remaining
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("{prefix}…{suffix}")
}

/// Cut a sentence at the end with an ellipsis; paths use `truncate_middle`.
pub(super) fn truncate_end(value: &str, width: usize) -> String {
    if Span::raw(value).width() <= width {
        return value.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut used = 0;
    let mut result: String = value
        .graphemes(true)
        .take_while(|g| {
            used += Span::raw(*g).width();
            used < width
        })
        .collect();
    result.push('…');
    result
}

pub(super) fn previous_grapheme_boundary(text: &str, cursor: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(index, _)| index)
        .take_while(|index| *index < cursor)
        .last()
        .unwrap_or(0)
}

pub(super) fn next_grapheme_boundary(text: &str, cursor: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(index, _)| index)
        .find(|index| *index > cursor)
        .unwrap_or(text.len())
}

/// Keep the insertion point visible, with nearby text on both sides when possible.
pub(super) fn input_window(text: &str, cursor: usize, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let cursor = cursor.min(text.len());
    let room = width - 1; // The visible insertion marker occupies one cell.
    let after = &text[cursor..];
    let before_room = room.saturating_sub(Span::raw(after).width().min(room / 3));
    let mut used = 0;
    let before: String = text[..cursor]
        .graphemes(true)
        .rev()
        .take_while(|g| {
            used += Span::raw(*g).width();
            used <= before_room
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let remaining = room - Span::raw(before.as_str()).width();
    format!("{before}▏{}", truncate_end(after, remaining))
}

pub(super) fn render_vertical_scrollbar(
    frame: &mut Frame<'_>,
    area: Rect,
    content_length: usize,
    position: usize,
    app: &App,
) {
    let viewport = area.height.saturating_sub(3) as usize;
    if content_length == 0 || content_length <= viewport || area.width == 0 {
        return;
    }
    let mut state = ScrollbarState::new(content_length).position(position);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("·"))
            .thumb_symbol("█")
            .track_style(Style::default().fg(app.color(MUTED)))
            .thumb_style(
                Style::default()
                    .fg(app.color(BLUE))
                    .add_modifier(Modifier::BOLD),
            ),
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut state,
    );
}

pub(super) const MAX_RELOCATION_SOURCES: usize = 20;

pub(super) fn relocation_sources(
    inventory: &StorageInventory,
    account_home: &Path,
) -> Vec<StorageItem> {
    let account_home = account_home
        .canonicalize()
        .unwrap_or_else(|_| account_home.to_path_buf());
    let temp_root = Path::new("/private/tmp")
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from("/private/tmp"));
    let items = if inventory.largest.is_empty() {
        &inventory.top_level
    } else {
        &inventory.largest
    };
    let mut selected = Vec::with_capacity(MAX_RELOCATION_SOURCES);
    for item in items {
        if item.kind != StorageItemKind::Directory || item.size_kb == 0 {
            continue;
        }
        let Some(path) = relocation_inventory_path(&item.path) else {
            continue;
        };
        let in_home = path.starts_with(&account_home) && path != account_home;
        let in_temp = path.starts_with(&temp_root) && path != temp_root;
        if !in_home && !in_temp {
            continue;
        }
        if selected
            .iter()
            .any(|chosen: &StorageItem| path.starts_with(&chosen.path))
        {
            continue;
        }
        let mut candidate = item.clone();
        candidate.path = path;
        selected.push(candidate);
        if selected.len() == MAX_RELOCATION_SOURCES {
            break;
        }
    }
    selected
}

pub(super) fn relocation_inventory_path(path: &Path) -> Option<PathBuf> {
    let path = path.canonicalize().ok()?;
    let data_root = Path::new("/System/Volumes/Data");
    if let Ok(relative) = path.strip_prefix(data_root) {
        let public_view = Path::new("/").join(relative);
        if public_view.is_dir() {
            return public_view.canonicalize().ok().or(Some(public_view));
        }
    }
    Some(path)
}

pub(super) fn expand_user_path(input: &str, account_home: &Path) -> PathBuf {
    if input == "~" {
        account_home.to_path_buf()
    } else if let Some(relative) = input.strip_prefix("~/") {
        account_home.join(relative)
    } else {
        PathBuf::from(input)
    }
}

pub(super) fn format_elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 60 {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// Smoothly fills the active category's slice while preserving the exact
/// completed-category boundaries. Directory traversal cannot know its total
/// item count without doing an expensive first pass, so the active slice eases
/// toward (but never reaches) its boundary until the category really finishes.
pub(super) fn smooth_scan_ratio(
    completed: usize,
    total: usize,
    active_elapsed: Option<Duration>,
) -> f64 {
    let total = total.max(1);
    if completed >= total {
        return 1.0;
    }

    let active_fraction = active_elapsed.map_or(0.0, |elapsed| {
        const ACTIVE_CAP: f64 = 0.94;
        const EASING_SECONDS: f64 = 24.0;
        let eased = 1.0 - (-elapsed.as_secs_f64() / EASING_SECONDS).exp();
        ACTIVE_CAP * eased
    });
    ((completed as f64 + active_fraction) / total as f64).clamp(0.0, 1.0)
}

/// Moves the rendered gauge toward measured progress without allowing fast
/// filesystem checks to jump across several categories in a single frame.
pub(super) fn animate_scan_ratio(current: f64, target: f64, elapsed: Duration) -> f64 {
    const RESPONSE_SECONDS: f64 = 0.12;
    const MAX_PROGRESS_PER_SECOND: f64 = 0.8;
    const MAX_FRAME_TIME: Duration = Duration::from_millis(50);

    let current = current.clamp(0.0, 1.0);
    let target = target.clamp(current, 1.0);
    let seconds = elapsed.min(MAX_FRAME_TIME).as_secs_f64();
    if seconds == 0.0 || target == current {
        return current;
    }

    let eased_step = (target - current) * (1.0 - (-seconds / RESPONSE_SECONDS).exp());
    let rate_limited_step = MAX_PROGRESS_PER_SECOND * seconds;
    (current + eased_step.min(rate_limited_step)).min(target)
}

pub(super) fn scan_spinner(elapsed: Duration) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let frame = (elapsed.as_millis() / 80) as usize % FRAMES.len();
    FRAMES[frame]
}

/// Register the rows that Ratatui actually displayed, including its scroll offset.
pub(super) fn table_hits(
    app: &App,
    area: Rect,
    offset: usize,
    count: usize,
    row_height: u16,
    header_height: u16,
    make: impl Fn(usize) -> HitTarget,
) {
    let mut y = area.y + 1 + header_height;
    for index in offset..count {
        if y >= area.bottom().saturating_sub(1) {
            break;
        }
        app.hit_regions.borrow_mut().push((
            Rect::new(
                area.x + 1,
                y,
                area.width.saturating_sub(2),
                row_height.min(area.bottom() - 1 - y),
            ),
            make(index),
        ));
        y += row_height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_respects_terminal_cells_and_complete_graphemes() {
        let text = "日本語 re\u{301}sume\u{301} 👩‍💻.txt";
        for width in 0..40 {
            for result in [truncate_middle(text, width), truncate_end(text, width)] {
                assert!(
                    Span::raw(result.as_str()).width() <= width,
                    "{width}: {result}"
                );
                assert!(
                    result
                        .graphemes(true)
                        .all(|g| g == "…" || text.graphemes(true).any(|original| original == g)),
                    "{result}"
                );
            }
        }
        assert_eq!(truncate_middle("日本語 résumé.txt", 15), "日本語 …umé.txt");
        assert_eq!(truncate_end("e\u{301}xy", 2), "e\u{301}…");
        assert_eq!(truncate_middle("abcdef", 0), "");
    }

    #[test]
    fn input_window_keeps_cursor_and_nearby_unicode_visible() {
        let text = "前".repeat(30) + "middle" + &"後".repeat(30);
        for cursor in text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain([text.len()])
        {
            for width in [1, 12, 45] {
                let result = input_window(&text, cursor, width);
                assert!(result.contains('▏'), "{cursor}: {result}");
                assert!(Span::raw(result.as_str()).width() <= width);
            }
        }
        assert!(input_window(&text, "前".repeat(30).len(), 45).contains("▏middle"));
        assert!(input_window(&text, text.len(), 45).ends_with("後▏"));
    }
}
