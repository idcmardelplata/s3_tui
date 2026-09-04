mod theme;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Padding, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table, TableState, Tabs, Wrap,
    },
};

use crate::app::{AppState, InputMode, LoadingState, Panel, StorageClass};

use theme::{Theme, panel_border};

/// The set of columns rendered in the objects table and their relative widths.
const OBJ_ICON_W: u16 = 3;
const OBJ_SIZE_W: u16 = 12;
const OBJ_STORAGE_W: u16 = 22;
const OBJ_DATE_W: u16 = 17;

fn storage_class_color(theme: &Theme, sc: &StorageClass) -> Color {
    match sc {
        StorageClass::Folder => theme.folder,
        StorageClass::Standard => theme.success,
        StorageClass::IntelligentTiering => Color::Cyan,
        StorageClass::Glacier | StorageClass::GlacierIr | StorageClass::DeepArchive => {
            Color::Magenta
        }
        StorageClass::StandardIa | StorageClass::OneZoneIa | StorageClass::ReducedRedundancy => {
            theme.warning
        }
        _ => theme.muted,
    }
}

// ---------------------------------------------------------------------------
// Move a "name" column rendered as a left-aligned cell with the storage class
// truncated to fit its column. These helpers keep the table layout stable.

fn centered_title(title: impl Into<String>) -> String {
    format!(" {} ", title.into())
}

pub fn render(frame: &mut Frame, state: &mut AppState) {
    let theme = Theme::dark();

    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    render_main(frame, state, &theme, chunks[0]);
    render_status_bar(frame, state, &theme, chunks[1]);
    render_tab_bar(frame, state, &theme, chunks[2]);

    if state.input_mode != InputMode::None {
        render_input_popup(frame, state, &theme);
    }
}

fn render_main(frame: &mut Frame, state: &mut AppState, theme: &Theme, area: Rect) {
    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    render_bucket_panel(frame, state, theme, chunks[0]);
    render_object_panel(frame, state, theme, chunks[1]);
}

fn render_bucket_panel(frame: &mut Frame, state: &mut AppState, theme: &Theme, area: Rect) {
    let active = state.current_panel == Panel::Buckets;

    let items: Vec<ListItem> = state
        .buckets
        .iter()
        .enumerate()
        .map(|(i, bucket)| {
            let is_selected = active && i == state.selected_index;
            let mut style = if is_selected {
                Style::default().fg(theme.highlight)
            } else if state.current_bucket.as_deref() == Some(&bucket.name) {
                Style::default().fg(theme.success)
            } else {
                Style::default().fg(theme.text)
            };
            if is_selected {
                style = style.add_modifier(Modifier::BOLD);
            }

            let date_str = bucket
                .creation_date
                .map(|d| format!("{:>10}", d.format("%Y-%m-%d")))
                .unwrap_or_else(|| format!("{:>10}", "-"));

            let num_objects = match state.current_bucket.as_deref() {
                Some(b) if bucket.name == b && state.current_prefix.is_empty() => {
                    format!("{:>6}", state.objects.len())
                }
                _ => format!("{:>6}", "-"),
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!("  {} ", bucket.name), style),
                Span::styled(date_str, Style::default().fg(theme.muted)),
                Span::styled(
                    format!(" {num_objects:>6} "),
                    Style::default().fg(theme.muted),
                ),
            ]))
        })
        .collect();

    let block = Block::default()
        .title(Line::from(Span::styled(
            centered_title("Buckets"),
            theme.panel_title(),
        )))
        .title_bottom(Line::from(Span::styled(
            if active {
                " Enter: open  /  h: back "
            } else {
                ""
            },
            Style::default().fg(theme.muted),
        )))
        .borders(Borders::ALL)
        .border_style(panel_border(theme, active))
        .padding(Padding::horizontal(1));

    let list = List::new(items)
        .block(block)
        .highlight_style(theme.selected_row());

    let mut list_state = ListState::default();
    if active {
        list_state.select(Some(state.selected_index));
    }

    if state.buckets.len() > (area.height as usize).saturating_sub(1) {
        render_scrollbar(
            frame,
            area,
            state.selected_index,
            state.buckets.len(),
            theme,
        );
    }

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_scrollbar(frame: &mut Frame, area: Rect, index: usize, total: usize, theme: &Theme) {
    let max = total.saturating_sub(1);
    let mut state = ScrollbarState::new(total).position(index.min(max));
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some(" "))
        .end_symbol(Some(" "))
        .track_symbol(Some(" "))
        .thumb_symbol(" ")
        .style(Style::default().fg(theme.muted_border))
        .thumb_style(Style::default().fg(theme.accent));

    let mut scroll_area = area;
    scroll_area.width = 2;

    frame.render_stateful_widget(scrollbar, scroll_area, &mut state);
}

fn render_object_panel(frame: &mut Frame, state: &mut AppState, theme: &Theme, area: Rect) {
    if state.current_panel == Panel::Preview {
        render_object_detail(frame, state, theme, area);
        return;
    }

    let active = state.current_panel == Panel::Objects;
    let bucket = state.current_bucket.clone();
    let prefix = state.current_prefix.clone();

    // Update the scroll offset so the selected row stays on screen.
    let visible_height = area.height.saturating_sub(3); // borders + table header
    update_scroll_offset(state, visible_height);

    let visible = state.visible_indices();
    let visible_total = visible.len();

    // ---- Build the table header -------------------------------------------
    let icon_hdr = Span::styled("", theme.panel_title());
    let name_hdr = Span::styled("Name", theme.panel_title());
    let size_hdr = Span::styled("Size", theme.panel_title());
    let storage_hdr = Span::styled("Storage", theme.panel_title());
    let date_hdr = Span::styled("Modified", theme.panel_title());
    let header = Row::new(vec![icon_hdr, name_hdr, size_hdr, storage_hdr, date_hdr])
        .style(Style::default().bg(Color::DarkGray));

    // ---- Build rows --------------------------------------------------------
    let rows: Vec<Row> = visible
        .iter()
        .enumerate()
        .map(|(pos, &real_idx)| {
            let obj = &state.objects[real_idx];
            let is_selected = active && pos == state.selected_index;
            let base = if obj.is_folder {
                Style::default().fg(theme.folder)
            } else {
                Style::default().fg(theme.text)
            };
            let style = if is_selected {
                base.add_modifier(Modifier::BOLD)
            } else {
                base
            };

            let icon = if obj.is_folder { "\u{25B8}" } else { "\u{B7}" };

            let name = obj.key.strip_prefix(&prefix).unwrap_or(&obj.key);
            let name = name.trim_start_matches('/').trim_end_matches('/');
            let name = if name.is_empty() { "/" } else { name };

            let size = obj
                .size
                .map(|s| bytesize::ByteSize(s).to_string())
                .unwrap_or_else(|| "-".to_string());
            let size = format!("{size:>width$}", width = OBJ_SIZE_W as usize - 1);

            let date = obj
                .last_modified
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "-".to_string());

            let sc = if obj.is_folder {
                String::new()
            } else {
                obj.storage_class.to_string()
            };
            let sc_style = Style::default().fg(storage_class_color(theme, &obj.storage_class));
            let sc = truncate(&sc, OBJ_STORAGE_W as usize - 1);

            Row::new(vec![
                Cell::from(Span::styled(format!(" {icon} "), style)),
                Cell::from(Span::styled(name.to_string(), style)),
                Cell::from(Span::styled(size, Style::default().fg(theme.muted))),
                Cell::from(Span::styled(sc, sc_style)),
                Cell::from(Span::styled(date, Style::default().fg(theme.muted))),
            ])
        })
        .collect();

    // ---- Panel title with metrics -----------------------------------------
    let mut title = if let Some(bucket) = &bucket {
        format!("s3://{bucket}/{prefix}")
    } else {
        "Objects".to_string()
    };
    if !state.filter.is_empty() {
        title = format!("{title}  [filter: '{}']", state.filter);
    }
    let title_span = Span::styled(centered_title(&title), theme.panel_title());

    let metrics = format!("{}/{} shown", visible_total, state.objects.len());
    let bottom_span = Span::styled(metrics, Style::default().fg(theme.muted));

    let block = Block::default()
        .title(Line::from(title_span))
        .title_bottom(Line::from(bottom_span))
        .borders(Borders::ALL)
        .border_style(panel_border(theme, active))
        .padding(Padding::horizontal(1));

    let widths = [
        Constraint::Length(OBJ_ICON_W),
        Constraint::Min(10),
        Constraint::Length(OBJ_SIZE_W),
        Constraint::Length(OBJ_STORAGE_W),
        Constraint::Length(OBJ_DATE_W),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(theme.selected_row());

    let mut table_state = TableState::default();
    if active {
        table_state.select(Some(state.selected_index));
    }
    *table_state.offset_mut() = state.scroll_offset.min(visible_total.saturating_sub(1));

    if visible_total > visible_height as usize && visible_total > 0 {
        render_scrollbar(frame, area, state.selected_index, visible_total, theme);
    }

    frame.render_stateful_widget(table, area, &mut table_state);
}

/// Keep `state.scroll_offset` so the selected row is visible within the area.
fn update_scroll_offset(state: &mut AppState, visible_height: u16) {
    if visible_height == 0 {
        return;
    }
    let sel = state.selected_index;
    let available = visible_height as usize;
    if sel < state.scroll_offset {
        state.scroll_offset = sel;
    } else if state.scroll_offset + available <= sel {
        state.scroll_offset = sel.saturating_sub(available - 1);
    }
}

fn render_object_detail(frame: &mut Frame, state: &mut AppState, theme: &Theme, area: Rect) {
    let detail = &state.selected_object_detail;
    let block = Block::default()
        .title(Line::from(Span::styled(
            centered_title("Object info"),
            theme.panel_title(),
        )))
        .borders(Borders::ALL)
        .border_style(panel_border(theme, true))
        .padding(Padding::horizontal(2));

    let lines: Vec<Line> = match detail {
        None => vec![Line::from(Span::styled(
            "No object selected / loading...",
            Style::default().fg(theme.muted),
        ))],
        Some(d) => {
            let size_label = d.size.map(|s| bytesize::ByteSize(s).to_string());
            let content_length_label = d.content_length.map(|s| bytesize::ByteSize(s).to_string());
            vec![
                Line::from(vec![
                    Span::styled("Bucket:        ", label_style(theme)),
                    Span::styled(&d.bucket, value_style(theme)),
                ]),
                Line::from(vec![
                    Span::styled("Key:           ", label_style(theme)),
                    Span::styled(&d.key, value_style(theme)),
                ]),
                Line::from(vec![
                    Span::styled("Size:          ", label_style(theme)),
                    Span::styled(
                        size_label.unwrap_or_else(|| "-".to_string()),
                        value_style(theme),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("ContentLength: ", label_style(theme)),
                    Span::styled(
                        content_length_label.unwrap_or_else(|| "-".to_string()),
                        value_style(theme),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("LastModified:  ", label_style(theme)),
                    Span::styled(
                        d.last_modified
                            .map(|m| m.format("%Y-%m-%d %H:%M:%S").to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        value_style(theme),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("StorageClass:  ", label_style(theme)),
                    Span::styled(
                        d.storage_class.to_string(),
                        value_style(theme).patch(
                            Style::default().fg(storage_class_color(theme, &d.storage_class)),
                        ),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("ETag:          ", label_style(theme)),
                    Span::styled(
                        d.etag.clone().unwrap_or_else(|| "-".to_string()),
                        value_style(theme),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("ContentType:   ", label_style(theme)),
                    Span::styled(
                        d.content_type.clone().unwrap_or_else(|| "-".to_string()),
                        value_style(theme),
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "Press i or Esc to go back",
                    Style::default().fg(theme.muted),
                )),
            ]
        }
    };

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn label_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD)
}

fn value_style(theme: &Theme) -> Style {
    Style::default().fg(theme.text)
}

fn render_tab_bar(frame: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    let buckets_active = state.current_panel == Panel::Buckets;
    let objects_active = matches!(state.current_panel, Panel::Objects | Panel::Preview);

    let labels = vec![
        Line::from(Span::styled(
            " Buckets ",
            Style::default().fg(if buckets_active {
                theme.accent
            } else {
                theme.muted
            }),
        )),
        Line::from(Span::styled(
            " Objects ",
            Style::default().fg(if objects_active {
                theme.accent
            } else {
                theme.muted
            }),
        )),
    ];
    let tabs = Tabs::new(labels)
        .select(match state.current_panel {
            Panel::Buckets => 0,
            _ => 1,
        })
        .divider("")
        .style(Style::default().fg(theme.muted))
        .highlight_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .padding(" ", " ");
    frame.render_widget(tabs, area);
}

fn render_status_bar(frame: &mut Frame, state: &AppState, theme: &Theme, area: Rect) {
    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    // Status (left): status message + loading state.
    let loading_text = match &state.loading_state {
        LoadingState::Idle => String::new(),
        LoadingState::Loading(msg) => format!(" {msg}"),
        LoadingState::Error(msg) => format!(" Error: {msg}"),
        LoadingState::Success(msg) => format!(" {msg}"),
    };
    let status_style = match state.loading_state {
        LoadingState::Error(_) => theme.error,
        LoadingState::Loading(_) => theme.warning,
        LoadingState::Success(_) => theme.success,
        LoadingState::Idle => theme.text,
    };

    let status = Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" {}", state.status_message),
            Style::default().fg(status_style),
        ),
        Span::styled(loading_text, Style::default().fg(theme.muted)),
    ]))
    .block(Block::default().borders(Borders::NONE))
    .wrap(Wrap { trim: false });

    // Build help as a single row of key hints so it stays within one line.
    let help_keys: &[(&str, &str)] = &[
        ("?", "help"),
        ("q", "quit"),
        ("u", "upload"),
        ("g", "down"),
        ("i", "info"),
        ("/", "filter"),
        ("d", "del"),
        ("Esc", "back"),
    ];
    let mut line: Vec<Span> = Vec::new();
    for (k, v) in help_keys {
        line.push(Span::styled(
            format!(" {k}:{v} "),
            Style::default().fg(theme.muted),
        ));
    }

    let help = Paragraph::new(Line::from(line))
        .alignment(Alignment::Right)
        .block(Block::default().borders(Borders::NONE))
        .wrap(Wrap { trim: false });

    frame.render_widget(status, chunks[0]);
    frame.render_widget(help, chunks[1]);
}

fn render_input_popup(frame: &mut Frame, state: &mut AppState, theme: &Theme) {
    let area = frame.area();
    let popup = centered_rect(70, 14, area);

    frame.render_widget(Clear, popup);

    let (title, border_color) = match state.input_mode {
        InputMode::Confirm => (" Confirm (y/n) ", Color::Red),
        InputMode::Path => (" Enter local file path to upload ", Color::Yellow),
        InputMode::Directory => (" Enter destination directory ", Color::Yellow),
        InputMode::Filter => (" Filter objects (type to search) ", Color::Cyan),
        InputMode::None => (" Input ", Color::Gray),
    };

    // Popup inner area to give breathing room around the input.
    let inner = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .margin(1)
        .split(popup);

    // A short descriptive line above the field.
    let helper = match state.input_mode {
        InputMode::Confirm => "Deleting this object cannot be undone. Type y to confirm.",
        InputMode::Path => "Enter the path of the local file you want to upload.",
        InputMode::Directory => "Enter the destination directory for the download.",
        InputMode::Filter => "Type to filter objects. Enter applies it, Esc clears it.",
        InputMode::None => "",
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {helper}"),
            Style::default().fg(theme.muted),
        ))),
        inner[0],
    );

    // Input field with a visible cursor for editable modes.
    let editable = matches!(
        state.input_mode,
        InputMode::Path | InputMode::Directory | InputMode::Filter
    );
    let field_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let input = Paragraph::new(state.input_buffer.as_str())
        .style(Style::default().fg(theme.text))
        .block(field_block);

    let input_area = inner[1];
    frame.render_widget(input, input_area);

    if editable {
        let cursor_x = input_area.x + 1 + state.input_buffer.len() as u16;
        let cursor_y = input_area.y + 1;
        frame.set_cursor_position((cursor_x.min(input_area.right().saturating_sub(2)), cursor_y));
    }

    // Outer bordered popup wrapping everything.
    let popup_block = Block::default()
        .title(Line::from(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        )))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    frame.render_widget(popup_block, popup);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Truncate `s` to at most `max` characters (with an ellipsis when cut).
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('\u{2026}');
        out
    }
}
