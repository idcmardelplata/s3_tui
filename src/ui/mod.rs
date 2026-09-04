pub mod theme;

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Padding, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table, TableState, Tabs, Wrap,
    },
};

use crate::app::{AppState, InputMode, LoadingState, Panel, StorageClass};

use theme::{Theme, base, folder_color, panel_border, panel_title, selected_row};

/// The set of columns rendered in the objects table and their relative widths.
const OBJ_SEL_W: u16 = 4;
const OBJ_ICON_W: u16 = 3;
const OBJ_SIZE_W: u16 = 12;
const OBJ_STORAGE_W: u16 = 22;
const OBJ_DATE_W: u16 = 17;

fn storage_class_color(theme: &Theme, sc: &StorageClass) -> Style {
    match sc {
        StorageClass::Folder => folder_color(theme),
        StorageClass::Standard => theme.success,
        StorageClass::IntelligentTiering => theme.accent,
        StorageClass::Glacier | StorageClass::GlacierIr | StorageClass::DeepArchive => {
            theme.accent_dim
        }
        StorageClass::StandardIa | StorageClass::OneZoneIa | StorageClass::ReducedRedundancy => {
            theme.warning
        }
        _ => theme.text_dim,
    }
    .into()
}

// ---------------------------------------------------------------------------
// Move a "name" column rendered as a left-aligned cell with the storage class
// truncated to fit its column. These helpers keep the table layout stable.

fn centered_title(title: impl Into<String>) -> String {
    format!(" {} ", title.into())
}

pub fn render(frame: &mut Frame, state: &mut AppState) {
    let theme = state.theme;

    frame.render_widget(Block::default().style(base(&theme)), frame.area());

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

    match state.input_mode {
        InputMode::FilePicker => render_file_picker(frame, state, &theme),
        InputMode::None => {}
        _ => render_input_popup(frame, state, &theme),
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
                Style::default().fg(theme.accent)
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
                Span::styled(date_str, Style::default().fg(theme.text_dim)),
                Span::styled(
                    format!(" {num_objects:>6} "),
                    Style::default().fg(theme.text_dim),
                ),
            ]))
        })
        .collect();

    let block = Block::default()
        .title(Line::from(Span::styled(
            centered_title("Buckets"),
            panel_title(theme),
        )))
        .title_bottom(Line::from(Span::styled(
            if active {
                " Enter: open  /  h: back "
            } else {
                ""
            },
            Style::default().fg(theme.text_dim),
        )))
        .borders(Borders::ALL)
        .border_style(panel_border(theme, active))
        .padding(Padding::horizontal(1));

    let list = List::new(items)
        .block(block)
        .highlight_style(selected_row(theme));

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
        .style(Style::default().fg(theme.border))
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
    let sel_hdr = Span::styled("Sel", panel_title(theme));
    let icon_hdr = Span::styled("", panel_title(theme));
    let name_hdr = Span::styled("Name", panel_title(theme));
    let size_hdr = Span::styled("Size", panel_title(theme));
    let storage_hdr = Span::styled("Storage", panel_title(theme));
    let date_hdr = Span::styled("Modified", panel_title(theme));
    let header = Row::new(vec![
        sel_hdr,
        icon_hdr,
        name_hdr,
        size_hdr,
        storage_hdr,
        date_hdr,
    ])
    .style(theme.surface);

    // ---- Build rows --------------------------------------------------------
    let rows: Vec<Row> = visible
        .iter()
        .enumerate()
        .map(|(pos, &real_idx)| {
            let obj = &state.objects[real_idx];
            let is_selected = active && pos == state.selected_index;
            let is_marked = state.selected_keys.contains(&obj.key);
            let base = if obj.is_folder {
                Style::default().fg(folder_color(theme))
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
            let sc_style = storage_class_color(theme, &obj.storage_class);
            let sc = truncate(&sc, OBJ_STORAGE_W as usize - 1);

            let checkbox = if obj.is_folder {
                String::new()
            } else if is_marked {
                "\u{2713}".to_string()
            } else {
                " ".to_string()
            };
            let checkbox_style = if is_marked {
                Style::default()
                    .fg(theme.success)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_dim)
            };

            Row::new(vec![
                Cell::from(Span::styled(format!("[{checkbox}]"), checkbox_style)),
                Cell::from(Span::styled(format!(" {icon} "), style)),
                Cell::from(Span::styled(name.to_string(), style)),
                Cell::from(Span::styled(size, Style::default().fg(theme.text_dim))),
                Cell::from(Span::styled(sc, sc_style)),
                Cell::from(Span::styled(date, Style::default().fg(theme.text_dim))),
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
    let title_span = Span::styled(centered_title(&title), panel_title(theme));

    let metrics = if state.selected_keys.is_empty() {
        format!("{}/{} shown", visible_total, state.objects.len())
    } else {
        format!(
            "{} selected | {}/{} shown",
            state.selected_keys.len(),
            visible_total,
            state.objects.len()
        )
    };
    let bottom_span = Span::styled(metrics, Style::default().fg(theme.text_dim));

    let block = Block::default()
        .title(Line::from(title_span))
        .title_bottom(Line::from(bottom_span))
        .borders(Borders::ALL)
        .border_style(panel_border(theme, active))
        .padding(Padding::horizontal(1));

    let widths = [
        Constraint::Length(OBJ_SEL_W),
        Constraint::Length(OBJ_ICON_W),
        Constraint::Min(10),
        Constraint::Length(OBJ_SIZE_W),
        Constraint::Length(OBJ_STORAGE_W),
        Constraint::Length(OBJ_DATE_W),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(selected_row(theme));

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
            panel_title(theme),
        )))
        .borders(Borders::ALL)
        .border_style(panel_border(theme, true))
        .padding(Padding::horizontal(2));

    let lines: Vec<Line> = match detail {
        None => vec![Line::from(Span::styled(
            "No object selected / loading...",
            Style::default().fg(theme.text_dim),
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
                        value_style(theme).patch(storage_class_color(theme, &d.storage_class)),
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
                    Style::default().fg(theme.text_dim),
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
                theme.text_dim
            }),
        )),
        Line::from(Span::styled(
            " Objects ",
            Style::default().fg(if objects_active {
                theme.accent
            } else {
                theme.text_dim
            }),
        )),
    ];
    let tabs = Tabs::new(labels)
        .select(match state.current_panel {
            Panel::Buckets => 0,
            _ => 1,
        })
        .divider("")
        .style(Style::default().fg(theme.text_dim))
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
        Span::styled(loading_text, Style::default().fg(theme.text_dim)),
    ]))
    .block(Block::default().borders(Borders::NONE))
    .wrap(Wrap { trim: false });

    // Build help as a single row of key hints so it stays within one line.
    let help_keys: &[(&str, &str)] = &[
        ("?", "help"),
        ("q", "quit"),
        ("u", "upload"),
        ("g", "down"),
        ("space", "sel"),
        ("a", "all"),
        ("c", "clear"),
        ("i", "info"),
        ("/", "filter"),
        ("d", "del"),
        ("Esc", "back"),
    ];
    let mut line: Vec<Span> = Vec::new();
    for (k, v) in help_keys {
        line.push(Span::styled(
            format!(" {k}:{v} "),
            Style::default().fg(theme.text_dim),
        ));
    }

    let help = Paragraph::new(Line::from(line))
        .alignment(Alignment::Right)
        .block(Block::default().borders(Borders::NONE))
        .wrap(Wrap { trim: false });

    frame.render_widget(status, chunks[0]);
    frame.render_widget(help, chunks[1]);
}

/// Full-screen overlay listing the current directory so one or more local
/// files can be picked (with fuzzy filtering) and uploaded in a batch.
fn render_file_picker(frame: &mut Frame, state: &mut AppState, theme: &Theme) {
    let area = frame.area();
    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    {
        let picker = &mut state.file_picker;
        let visible_total = picker.visible_count();
        let inner_height = chunks[0].height.saturating_sub(2);
        if visible_total > 0 {
            let sel = picker.selected_index.min(visible_total - 1);
            if sel < picker.scroll_offset {
                picker.scroll_offset = sel;
            } else if picker.scroll_offset + inner_height as usize <= sel {
                picker.scroll_offset = sel.saturating_sub(inner_height as usize - 1);
            }
        }

        let visible = picker.visible_indices();

        let items: Vec<ListItem> = visible
            .iter()
            .enumerate()
            .map(|(pos, &real_idx)| {
                let entry = &picker.entries[real_idx];
                let is_selected = pos == picker.selected_index;
                let is_marked = picker.selection.iter().any(|p| p == &entry.path);

                let base = if entry.is_dir {
                    Style::default().fg(theme.info)
                } else {
                    Style::default().fg(theme.text)
                };
                let style = if is_selected {
                    base.add_modifier(Modifier::BOLD)
                } else {
                    base
                };

                let check = if entry.is_dir {
                    " "
                } else if is_marked {
                    "\u{2713}"
                } else {
                    " "
                };
                let check_style = if is_marked {
                    Style::default()
                        .fg(theme.success)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text_dim)
                };

                let display = if entry.is_dir {
                    format!("{}/", entry.name)
                } else {
                    entry.name.clone()
                };
                let size = entry
                    .size
                    .map(|s| format!("{:>10}", bytesize::ByteSize(s).to_string()))
                    .unwrap_or_else(|| format!("{:>10}", "-"));

                ListItem::new(Line::from(vec![
                    Span::styled(format!("[{check}]"), check_style),
                    Span::styled(format!(" {display} "), style),
                    Span::styled(size, Style::default().fg(theme.text_dim)),
                ]))
            })
            .collect();

        let mut title = format!("Select files · {}", picker.dir.display());
        if !picker.filter.is_empty() {
            title = format!("{title}  [filter: '{}']", picker.filter);
        }
        let block = Block::default()
            .title(Line::from(Span::styled(
                centered_title(&title),
                panel_title(theme),
            )))
            .title_bottom(Line::from(Span::styled(
                format!(
                    "{} selected | {}/{} shown",
                    picker.selection_count(),
                    visible.len(),
                    picker.entries.len().saturating_sub(1)
                ),
                Style::default().fg(theme.text_dim),
            )))
            .borders(Borders::ALL)
            .border_style(panel_border(theme, true))
            .padding(Padding::horizontal(1));

        let list = List::new(items)
            .block(block)
            .highlight_style(selected_row(theme));

        let mut list_state = ListState::default();
        if !visible.is_empty() {
            list_state.select(Some(picker.selected_index.min(visible.len() - 1)));
        }
        *list_state.offset_mut() = picker.scroll_offset.min(visible.len().saturating_sub(1));

        if visible.len() > inner_height as usize && !visible.is_empty() {
            render_scrollbar(
                frame,
                chunks[0],
                picker.selected_index,
                visible.len(),
                theme,
            );
        }

        frame.render_stateful_widget(list, chunks[0], &mut list_state);
    }

    // Filter line: shows the fuzzy query, the path and any directory error.
    let filter_text = if picker_is_loading_error(state) {
        format!(" {}", state.file_picker.hint)
    } else if state.file_picker.filter.is_empty() {
        " Type to fuzzy-filter files (subsequence match)".to_string()
    } else {
        format!(" /{}", state.file_picker.filter)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            filter_text,
            Style::default().fg(theme.info),
        )))
        .block(Block::default().borders(Borders::NONE)),
        chunks[1],
    );

    // Key hints (second-to-last line is the path, still in the list block).
    let hints = format!(
        " {} | ↑↓:move · Enter/→:open dir · Space:mark/unmark · a:all · c:clear · u:upload · Esc:back ",
        state.file_picker.dir.display()
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(theme.text_dim),
        )))
        .block(Block::default().borders(Borders::NONE))
        .wrap(Wrap { trim: false }),
        chunks[2],
    );
}

/// True while the picker failed to read its directory (hint is set).
fn picker_is_loading_error(state: &AppState) -> bool {
    !state.file_picker.hint.is_empty()
}

fn render_input_popup(frame: &mut Frame, state: &mut AppState, theme: &Theme) {
    let area = frame.area();
    let popup = centered_rect(70, 10, area);

    frame.render_widget(Clear, popup);

    let (title, border_color) = match state.input_mode {
        InputMode::Confirm => (" Confirm (y/n) ", theme.error),
        InputMode::Directory => (" Enter destination directory ", theme.warning),
        InputMode::Filter => (" Filter objects (type to search) ", theme.accent),
        InputMode::FilePicker => (" Select files to upload ", theme.info),
        InputMode::None => (" Input ", theme.text_dim),
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
        InputMode::Directory => "Enter the destination directory for the download.",
        InputMode::Filter => "Type to filter objects. Enter applies it, Esc clears it.",
        InputMode::FilePicker => "Select one or more files. u starts the upload.",
        InputMode::None => "",
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {helper}"),
            Style::default().fg(theme.text_dim),
        ))),
        inner[0],
    );

    // Input field with a visible cursor for editable modes.
    let editable = matches!(state.input_mode, InputMode::Directory | InputMode::Filter);
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
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(theme.surface));
    frame.render_widget(popup_block, popup);
}

/// Returns a rect centered in `r` with a fixed number of rows and a width that
/// is `percent_x` of the full width. Using a fixed height (instead of a
/// percentage of the terminal) guarantees the input field always has room to
/// render its content.
fn centered_rect(percent_x: u16, height: u16, r: Rect) -> Rect {
    let height = height.min(r.height.saturating_sub(2));
    let vertical = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            Constraint::Length((r.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(r);

    let width = (r.width * percent_x) / 100;
    let horizontal = Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([
            Constraint::Length(r.width.saturating_sub(width) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(vertical[1]);

    horizontal[1]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppState, BucketInfo, ObjectInfo};
    use crate::ui::theme;
    use chrono::TimeZone;
    use ratatui::{Terminal, backend::TestBackend};

    /// Render a view into a `TestBackend` and return the frame's buffer.
    fn draw(state: &mut AppState, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render(frame, state))
            .expect("render must not panic");
        terminal.backend().buffer().clone()
    }

    /// Rebuild each terminal row into a string and search it for `needle`,
    /// returning the `(x, y)` coordinates of the first matching cell.
    fn locate(buffer: &ratatui::buffer::Buffer, needle: &str) -> Option<(u16, u16)> {
        let area = buffer.area();
        for y in 0..area.height {
            let mut offsets = Vec::with_capacity(area.width as usize);
            let mut row = String::new();
            for x in 0..area.width {
                offsets.push(row.len());
                row.push_str(buffer.cell((x, y)).map_or(" ", |c| c.symbol()));
            }
            if let Some(byte) = row.find(needle) {
                let x = offsets.iter().position(|&o| o == byte).unwrap_or(0) as u16;
                return Some((x, y));
            }
        }
        None
    }

    fn fg_at(buffer: &ratatui::buffer::Buffer, needle: &str) -> ratatui::style::Color {
        let (x, y) = locate(buffer, needle).unwrap_or_else(|| {
            panic!("'{needle}' not found in rendered output");
        });
        buffer.cell((x, y)).unwrap().fg
    }

    fn sample_state() -> AppState {
        let mut state = AppState::new("us-east-1".to_string());
        state.current_panel = Panel::Objects;
        state.current_bucket = Some("demo".to_string());
        state.buckets = vec![BucketInfo {
            name: "demo".to_string(),
            creation_date: Some(chrono::Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap()),
            region: "us-east-1".to_string(),
        }];
        state.objects = vec![
            ObjectInfo {
                key: "demo/myfile.txt".to_string(),
                size: Some(1024),
                last_modified: None,
                is_folder: false,
                storage_class: StorageClass::Standard,
            },
            ObjectInfo {
                key: "demo/".to_string(),
                size: None,
                last_modified: None,
                is_folder: true,
                storage_class: StorageClass::Folder,
            },
            ObjectInfo {
                key: "demo/cold.log".to_string(),
                size: Some(42),
                last_modified: None,
                is_folder: false,
                storage_class: StorageClass::Glacier,
            },
        ];
        state.selected_index = 0;
        // Force a known palette so the test is independent of the environment.
        state.theme = theme::from_id("dracula");
        state
    }

    #[test]
    fn renders_main_view_with_theme_canvas() {
        let buffer = draw(&mut sample_state(), 120, 30);
        let theme = theme::from_id("dracula");
        assert_eq!(buffer.cell((0, 0)).unwrap().bg, theme.background);
        assert_eq!(buffer.cell((60, 15)).unwrap().bg, theme.background);
    }

    #[test]
    fn renders_storage_class_colors_from_theme() {
        let buffer = draw(&mut sample_state(), 120, 30);
        let theme = theme::from_id("dracula");
        assert_eq!(fg_at(&buffer, "cold"), theme.text);
        assert_eq!(
            fg_at(&buffer, "GLACIER"),
            theme.accent_dim,
            "GLACIER uses accent_dim"
        );
    }

    #[test]
    fn renders_input_popup() {
        let mut state = sample_state();
        state.input_mode = InputMode::Directory;
        state.input_buffer = "/tmp/downloads".to_string();
        let buffer = draw(&mut state, 120, 30);
        assert!(locate(&buffer, "Enter destination directory").is_some());
    }

    #[test]
    fn confirm_popup_uses_error_color() {
        let mut state = sample_state();
        state.input_mode = InputMode::Confirm;
        let theme = theme::from_id("dracula");
        let buffer = draw(&mut state, 120, 30);
        assert_eq!(fg_at(&buffer, "Confirm (y/n)"), theme.error);
    }
}
