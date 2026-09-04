use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{AppState, InputMode, LoadingState, Panel, StorageClass};

fn storage_class_color(sc: &StorageClass) -> Color {
    match sc {
        StorageClass::Folder => Color::Blue,
        StorageClass::Standard => Color::Green,
        StorageClass::IntelligentTiering => Color::Cyan,
        StorageClass::Glacier | StorageClass::GlacierIr | StorageClass::DeepArchive => {
            Color::Magenta
        }
        StorageClass::StandardIa | StorageClass::OneZoneIa | StorageClass::ReducedRedundancy => {
            Color::Yellow
        }
        _ => Color::DarkGray,
    }
}

pub fn render(frame: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(frame.area());

    render_main(frame, state, chunks[0]);
    render_status_bar(frame, state, chunks[1]);
}

fn render_main(frame: &mut Frame, state: &AppState, area: Rect) {
    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    render_bucket_panel(frame, state, chunks[0]);
    render_object_panel(frame, state, chunks[1]);

    if state.input_mode != InputMode::None {
        render_input_popup(frame, state);
    }
}

fn render_bucket_panel(frame: &mut Frame, state: &AppState, area: Rect) {
    let items: Vec<ListItem> = state
        .buckets
        .iter()
        .enumerate()
        .map(|(i, bucket)| {
            let style = if state.current_panel == Panel::Buckets && i == state.selected_index {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if state.current_bucket.as_deref() == Some(&bucket.name) {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };

            let date_str = bucket
                .creation_date
                .map(|d| format!("  {}", d.format("%Y-%m-%d")))
                .unwrap_or_default();

            ListItem::new(Line::from(vec![
                Span::styled(format!("  {} ", bucket.name), style),
                Span::styled(date_str, Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let title = format!(" Buckets ({}) ", state.buckets.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    let mut list_state = ListState::default();
    if state.current_panel == Panel::Buckets {
        list_state.select(Some(state.selected_index));
    }

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_object_panel(frame: &mut Frame, state: &AppState, area: Rect) {
    if state.current_panel == Panel::Preview {
        render_object_detail(frame, state, area);
        return;
    }

    let title = if let Some(bucket) = &state.current_bucket {
        let mut t = format!(" s3://{bucket}/{} ", state.current_prefix);
        if !state.filter.is_empty() {
            t = format!("{t} [filter: '{}'] ", state.filter);
        }
        t
    } else {
        " Objects ".to_string()
    };

    let visible = state.visible_indices();
    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .map(|(pos, &real_idx)| {
            let obj = &state.objects[real_idx];
            let style = if state.current_panel == Panel::Objects && pos == state.selected_index {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if obj.is_folder {
                Style::default().fg(Color::Blue)
            } else {
                Style::default()
            };

            let icon = if obj.is_folder { "▸" } else { "·" };
            let name = obj
                .key
                .strip_prefix(&state.current_prefix)
                .unwrap_or(&obj.key)
                .trim_start_matches('/')
                .trim_end_matches('/');

            let size_str = obj
                .size
                .map(|s| {
                    let size = bytesize::ByteSize(s);
                    format!(" [{size}]")
                })
                .unwrap_or_default();

            let date_str = obj
                .last_modified
                .map(|d| format!("  {}", d.format("%Y-%m-%d %H:%M")))
                .unwrap_or_default();

            let sc_label = if obj.is_folder {
                String::new()
            } else {
                format!("  [{}]", obj.storage_class)
            };
            let sc_style =
                style.patch(Style::default().fg(storage_class_color(&obj.storage_class)));

            ListItem::new(Line::from(vec![
                Span::styled(format!(" {icon} "), style),
                Span::styled(format!("{name}{size_str}"), style),
                Span::styled(sc_label, sc_style),
                Span::styled(date_str, Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    let mut list_state = ListState::default();
    if state.current_panel == Panel::Objects {
        list_state.select(Some(state.selected_index));
    }

    frame.render_stateful_widget(list, area, &mut list_state);
}

fn render_object_detail(frame: &mut Frame, state: &AppState, area: Rect) {
    let detail = &state.selected_object_detail;
    let block = Block::default()
        .title(" Object info ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let lines: Vec<Line> = match detail {
        None => vec![Line::from(Span::styled(
            "No object selected / loading...",
            Style::default().fg(Color::DarkGray),
        ))],
        Some(d) => {
            let size_label = d.size.map(|s| bytesize::ByteSize(s).to_string());
            let content_length_label = d.content_length.map(|s| bytesize::ByteSize(s).to_string());
            vec![
                Line::from(vec![
                    Span::styled("Bucket:        ", label_style()),
                    Span::styled(&d.bucket, value_style()),
                ]),
                Line::from(vec![
                    Span::styled("Key:           ", label_style()),
                    Span::styled(&d.key, value_style()),
                ]),
                Line::from(vec![
                    Span::styled("Size:          ", label_style()),
                    Span::styled(size_label.unwrap_or_else(|| "-".to_string()), value_style()),
                ]),
                Line::from(vec![
                    Span::styled("ContentLength: ", label_style()),
                    Span::styled(
                        content_length_label.unwrap_or_else(|| "-".to_string()),
                        value_style(),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("LastModified:  ", label_style()),
                    Span::styled(
                        d.last_modified
                            .map(|m| m.format("%Y-%m-%d %H:%M:%S").to_string())
                            .unwrap_or_else(|| "-".to_string()),
                        value_style(),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("StorageClass:  ", label_style()),
                    Span::styled(
                        d.storage_class.to_string(),
                        value_style()
                            .patch(Style::default().fg(storage_class_color(&d.storage_class))),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("ETag:          ", label_style()),
                    Span::styled(
                        d.etag.clone().unwrap_or_else(|| "-".to_string()),
                        value_style(),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("ContentType:   ", label_style()),
                    Span::styled(
                        d.content_type.clone().unwrap_or_else(|| "-".to_string()),
                        value_style(),
                    ),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    "Press i or Esc to go back",
                    Style::default().fg(Color::DarkGray),
                )),
            ]
        }
    };

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}

fn label_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

fn value_style() -> Style {
    Style::default().fg(Color::White)
}

fn render_status_bar(frame: &mut Frame, state: &AppState, area: Rect) {
    let chunks = Layout::default()
        .direction(ratatui::layout::Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(30)])
        .split(area);

    let loading_text = match &state.loading_state {
        LoadingState::Idle => String::new(),
        LoadingState::Loading(msg) => msg.clone(),
        LoadingState::Error(msg) => format!("Error: {msg}"),
        LoadingState::Success(msg) => msg.clone(),
    };

    let status_line = Line::from(vec![
        Span::styled(
            format!(" {} ", state.status_message),
            Style::default().fg(Color::White),
        ),
        Span::styled(format!(" {loading_text}"), Style::default().fg(Color::Gray)),
    ]);

    let status = Paragraph::new(status_line)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: false });

    let help_line = Line::from(vec![
        Span::styled(" ?:help", Style::default().fg(Color::DarkGray)),
        Span::styled(" q:quit", Style::default().fg(Color::DarkGray)),
        Span::styled(" u:upload", Style::default().fg(Color::DarkGray)),
        Span::styled(" g:download", Style::default().fg(Color::DarkGray)),
        Span::styled(" i:info", Style::default().fg(Color::DarkGray)),
        Span::styled(" /:filter", Style::default().fg(Color::DarkGray)),
        Span::styled(" d:delete", Style::default().fg(Color::DarkGray)),
    ]);

    let help = Paragraph::new(help_line).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    frame.render_widget(status, chunks[0]);
    frame.render_widget(help, chunks[1]);
}

fn render_input_popup(frame: &mut Frame, state: &AppState) {
    let area = frame.area();
    let popup = centered_rect(70, 25, area);

    frame.render_widget(Clear, popup);

    let (title, border_color) = match state.input_mode {
        InputMode::Confirm => (" Confirm (y/n) ", Color::Red),
        InputMode::Path => (" Enter local file path to upload ", Color::Yellow),
        InputMode::Directory => (" Enter destination directory ", Color::Yellow),
        InputMode::Filter => (" Filter objects (type to search) ", Color::Cyan),
        InputMode::None => (" Input ", Color::Gray),
    };

    let input = Paragraph::new(state.input_buffer.as_str())
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        );

    frame.render_widget(input, popup);
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
