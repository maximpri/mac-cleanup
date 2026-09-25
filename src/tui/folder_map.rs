// SPDX-License-Identifier: GPL-3.0-or-later
//! Selected-folder map. Geometry represents bytes, never cleanup eligibility.
use super::*;

const PALETTE: [Color; 6] = [
    // Muted blue, teal, and slate distinguish branches without implying risk.
    // Pale labels retain at least 7:1 contrast on every tile.
    Color::Rgb(36, 72, 91),
    Color::Rgb(37, 79, 83),
    Color::Rgb(53, 66, 88),
    Color::Rgb(63, 75, 87),
    Color::Rgb(43, 64, 76),
    Color::Rgb(53, 75, 73),
];

struct Tile<'a> {
    item: Option<&'a StorageItem>,
    size: u64,
    name: String,
}

fn name(item: &StorageItem) -> String {
    let value = item
        .path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| item.path.display().to_string());
    crate::ai::display_text(&value)
}

fn size_label(size: u64, width: u16) -> String {
    let full = format_kb(size);
    if full.len() <= usize::from(width) {
        return full;
    }
    let compact = full
        .replace(".0", "")
        .replace(" GiB", "G")
        .replace(" MiB", "M")
        .replace(" KiB", "K");
    if compact.len() <= usize::from(width) {
        compact
    } else {
        String::new()
    }
}

fn tiles(children: &[StorageItem], parent_size: u64) -> Vec<Tile<'_>> {
    let mut sorted: Vec<_> = children.iter().filter(|i| i.size_kb > 0).collect();
    sorted.sort_by(|a, b| b.size_kb.cmp(&a.size_kb).then_with(|| a.path.cmp(&b.path)));
    let measured = sorted.iter().fold(0u64, |n, i| n.saturating_add(i.size_kb));
    let mut result: Vec<_> = sorted
        .iter()
        .take(10)
        .map(|i| Tile {
            item: Some(i),
            size: i.size_kb,
            name: name(i),
        })
        .collect();
    if sorted.len() > 10 {
        result.push(Tile {
            item: None,
            size: sorted[10..].iter().map(|i| i.size_kb).sum(),
            name: format!("{} smaller items", sorted.len() - 10),
        });
    }
    if parent_size > measured {
        result.push(Tile {
            item: None,
            size: parent_size - measured,
            name: "Other / unmeasured".into(),
        });
    }
    result
}

/// Split by weight along the longest physical axis (terminal cells are tall).
/// No minimum-size inflation: sub-cell items stay in the list / folder browser.
pub(super) fn map_rects(weights: &[u64], area: Rect) -> Vec<Rect> {
    fn split(weights: &[u64], area: Rect, out: &mut Vec<Rect>) {
        if weights.is_empty() {
            return;
        }
        if weights.len() == 1 {
            out.push(if weights[0] == 0 {
                Rect::default()
            } else {
                area
            });
            return;
        }
        let total: u128 = weights.iter().map(|n| u128::from(*n)).sum();
        if total == 0 {
            out.extend(vec![Rect::default(); weights.len()]);
            return;
        }
        let mut left = 0u128;
        let mut pivot = 1;
        let mut best = u128::MAX;
        for (i, w) in weights.iter().enumerate().take(weights.len() - 1) {
            left += u128::from(*w);
            let distance = (2 * left).abs_diff(total);
            if distance < best {
                best = distance;
                pivot = i + 1;
            }
        }
        let weight: u128 = weights[..pivot].iter().map(|n| u128::from(*n)).sum();
        let horizontal = area.width >= area.height.saturating_mul(2);
        let length = if horizontal { area.width } else { area.height };
        let part = ((u128::from(length) * weight + total / 2) / total) as u16;
        let (a, b) = if horizontal {
            (
                Rect::new(area.x, area.y, part, area.height),
                Rect::new(area.x + part, area.y, area.width - part, area.height),
            )
        } else {
            (
                Rect::new(area.x, area.y, area.width, part),
                Rect::new(area.x, area.y + part, area.width, area.height - part),
            )
        };
        split(&weights[..pivot], a, out);
        split(&weights[pivot..], b, out);
    }
    let mut result = Vec::new();
    split(weights, area, &mut result);
    result
}

fn hit(app: &App, area: Rect, item: &StorageItem) {
    let mut paths = app.map_paths.borrow_mut();
    let index = paths.len();
    paths.push(item.path.clone());
    app.hit_regions
        .borrow_mut()
        .push((area, HitTarget::MapNode(index)));
}

fn draw_tiles(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    nodes: &[Tile<'_>],
    depth: usize,
    branch: usize,
) {
    let weights: Vec<_> = nodes.iter().map(|n| n.size).collect();
    let rects = map_rects(&weights, area);
    for (index, (node, rect)) in nodes.iter().zip(rects).enumerate() {
        if rect.width == 0 || rect.height == 0 {
            continue;
        }
        let branch = if depth == 0 { index } else { branch };
        let color = PALETTE[branch % PALETTE.len()];
        let block = (if rect.width >= 6 && rect.height >= 4 {
            Block::bordered()
        } else {
            Block::default()
        })
        .border_style(Style::default().fg(app.color(BACKGROUND)))
        .style(
            Style::default()
                .bg(app.color(if node.item.is_some() { color } else { FAINT }))
                .fg(app.color(INK)),
        );
        let inner = block.inner(rect);
        frame.render_widget(block, rect);
        let Some(item) = node.item else {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(truncate_middle(&node.name, inner.width as usize)),
                    Line::from(size_label(node.size, inner.width)),
                ]),
                inner,
            );
            continue;
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    truncate_middle(&node.name, inner.width as usize),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Line::from(size_label(node.size, inner.width)),
            ]),
            inner,
        );
        // Labels and borders open the parent; nested tiles open their own paths.
        if depth < 2
            && inner.width >= 14
            && inner.height >= 7
            && let Some(children) = app
                .inventory
                .as_ref()
                .and_then(|i| i.children.get(&item.path))
            && children.iter().any(|c| c.size_kb > 0)
        {
            let nested = tiles(children, item.size_kb);
            let nested_area = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
            draw_tiles(frame, nested_area, app, &nested, depth + 1, branch);
        }
        // Deeper hit regions come first, so they win pointer selection.
        hit(app, rect, item);
    }
}

pub(super) fn render_folder_map(frame: &mut Frame<'_>, area: Rect, app: &App, item: &StorageItem) {
    render_map(frame, area, app, item, false);
}
pub(super) fn render_care_map(
    frame: &mut Frame<'_>,
    area: Rect,
    app: &App,
    item: &StorageItem,
    grouped_children: Option<&[StorageItem]>,
) {
    let mut block = panel(
        app,
        if area.width < 40 {
            " HEATMAP "
        } else {
            " CONTENTS HEATMAP · area = size "
        },
    );
    if area.height >= 8 {
        block = block.title_bottom(Line::from(" Click a child · Enter / → browse "));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }
    if item.kind != StorageItemKind::Directory {
        frame.render_widget(
            Paragraph::new("This is a file, with no child folders. o reveals it in Finder.")
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }
    let children = grouped_children.or_else(|| {
        app.inventory
            .as_ref()
            .and_then(|inventory| inventory.children.get(&item.path))
            .map(Vec::as_slice)
    });
    match children {
        Some(children) if children.iter().any(|child| child.size_kb > 0) => {
            // Exact sizes remain readable even when a small tile cannot hold a label.
            let count = if inner.height >= 8 {
                children.len().min(4) as u16
            } else {
                0
            };
            for (row, child) in children.iter().take(count as usize).enumerate() {
                let rect = Rect::new(inner.x, inner.y + row as u16, inner.width, 1);
                let size = format_kb(child.size_kb);
                let label = format!(
                    "{} {}  {}",
                    if child.kind == StorageItemKind::Directory {
                        "▸"
                    } else {
                        "·"
                    },
                    truncate_middle(
                        &name(child),
                        inner.width.saturating_sub(size.len() as u16 + 4) as usize
                    ),
                    size
                );
                frame.render_widget(
                    Paragraph::new(label).style(Style::default().fg(app.color(INK))),
                    rect,
                );
                hit(app, rect, child);
            }
            let tiles_area = Rect::new(inner.x, inner.y + count, inner.width, inner.height - count);
            draw_tiles(frame, tiles_area, app, &tiles(children, item.size_kb), 0, 0);
        }
        _ => {
            frame.render_widget(Paragraph::new("No measured contents. The folder may be empty or unreadable; v shows coverage.").wrap(Wrap { trim: true }), inner);
        }
    }
}
fn render_map(frame: &mut Frame<'_>, area: Rect, app: &App, item: &StorageItem, care: bool) {
    let block = panel(
        app,
        if care {
            " FOLDER MAP · area = measured size "
        } else if app.storage_tab == StorageTab::Heatmap {
            " STORAGE MAP · e back to list "
        } else {
            " SELECTED ITEM · h expand "
        },
    );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 4 {
        return;
    }
    let mut title = vec![
        heading(app, format!("{}   {}", name(item), format_kb(item.size_kb))),
        label(
            app,
            truncate_middle(
                &crate::ai::display_text(&item.path.display().to_string()),
                inner.width as usize,
            ),
        ),
        label(app, "Area = disk space · colors identify folders"),
    ];
    let header_height = if inner.height < 12 { 1 } else { 3 };
    title.truncate(header_height as usize);
    frame.render_widget(
        Paragraph::new(title),
        Rect::new(inner.x, inner.y, inner.width, header_height),
    );
    let children = app
        .inventory
        .as_ref()
        .and_then(|i| i.children.get(&item.path));
    let footer_height = if inner.height >= 22 {
        8
    } else if inner.height >= 12 {
        3
    } else {
        1
    };
    let map_area = Rect::new(
        inner.x,
        inner.y + header_height,
        inner.width,
        inner.height.saturating_sub(header_height + footer_height),
    );
    if item.kind != StorageItemKind::Directory {
        frame.render_widget(
            Paragraph::new("File selected. Press o to reveal it in Finder.")
                .wrap(Wrap { trim: true }),
            map_area,
        );
    } else if let Some(children) = children.filter(|c| c.iter().any(|i| i.size_kb > 0)) {
        draw_tiles(frame, map_area, app, &tiles(children, item.size_kb), 0, 0);
    } else {
        frame.render_widget(Paragraph::new("No measured contents. This folder may be empty or unreadable. v shows scan coverage.").wrap(Wrap {trim:true}),map_area);
    }
    let footer = Rect::new(
        inner.x,
        map_area.bottom(),
        inner.width,
        footer_height.min(inner.bottom().saturating_sub(map_area.bottom())),
    );
    let mut lines = vec![
        heading(
            app,
            if care {
                "e  Explore    o  Finder · click a folder"
            } else if item.kind == StorageItemKind::Directory {
                "Enter  Open folder    i  Details"
            } else {
                "o  Reveal file    i  Details"
            },
        ),
        label(app, "Click a rectangle to explore it. ← goes back."),
    ];
    if footer_height >= 8 {
        if let Some(children) = children {
            let mut children: Vec<_> = children.iter().filter(|i| i.size_kb > 0).collect();
            children.sort_by_key(|c| Reverse(c.size_kb));
            for child in children.iter().take(3) {
                lines.push(Line::from(format!(
                    "{}  {}",
                    format_kb(child.size_kb),
                    name(child)
                )));
            }
        }
        lines.push(Line::from(""));
    }
    let matched = app.entries.iter().find(|e| e.spec.path == item.path);
    let decision = match matched {
        Some(entry) => format!("{} · f Review cleanup impact", entry.status.label()),
        None => {
            let count = app
                .entries
                .iter()
                .filter(|e| e.spec.path.starts_with(&item.path))
                .count();
            if count > 0 {
                format!("{count} cleanup findings inside · f Review")
            } else {
                "No cleanup rule · review contents before removing".into()
            }
        }
    };
    if footer_height == 1 {
        lines.truncate(1);
    } else {
        lines.push(label(app, decision));
    }
    frame.render_widget(Paragraph::new(lines), footer);
}

impl App {
    pub(super) fn open_map_path(&mut self, path: &Path) {
        let item = self
            .inventory
            .as_ref()
            .and_then(|i| {
                path.parent()
                    .and_then(|p| i.children.get(p))
                    .and_then(|children| children.iter().find(|c| c.path == path))
                    .or_else(|| i.top_level.iter().find(|c| c.path == path))
                    .or_else(|| i.children.values().flatten().find(|c| c.path == path))
            })
            .cloned();
        let Some(item) = item else {
            return;
        };
        self.explorer_history
            .push((self.explorer_path.clone(), self.explorer_cursor));
        if item.kind == StorageItemKind::Directory {
            self.explorer_path = Some(item.path);
            self.explorer_cursor = 0;
            self.explorer_details = false;
        } else {
            self.explorer_path = path.parent().map(Path::to_path_buf);
            self.explorer_cursor = self
                .explorer_items()
                .iter()
                .position(|i| i.path == path)
                .unwrap_or(0);
            self.explorer_details = true;
            self.dialog_scroll = 0;
        }
        self.status_message = None;
    }
}
