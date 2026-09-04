mod app;
mod cli;
mod config;
mod errors;
mod filepicker;
mod s3;
mod ui;

use anyhow::{Context, Result};
use app::{AppState, InputMode, LoadingState, Panel, PendingAction, TaskMessage};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{Terminal, backend::CrosstermBackend};
use s3::S3Client;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = cli::Cli::parse();
    let _ = config::ensure_default_config();
    let config = config::load().unwrap_or_default();
    let settings = cli::ResolvedSettings::from_parts(&cli, &config);

    let mut terminal = ratatui::init();
    let result = run_app(&mut terminal, &settings).await;
    ratatui::restore();

    if let Err(err) = result {
        eprintln!("Application error: {err:#}");
    }

    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    settings: &cli::ResolvedSettings,
) -> Result<()> {
    let mut state = AppState::new(settings.region.clone());
    let s3_client = S3Client::new(
        &settings.region,
        settings.endpoint.as_deref(),
        settings.profile.as_deref(),
        settings.force_path_style,
        settings.credentials.as_ref(),
    )
    .await
    .with_context(|| {
        let endpoint = settings
            .endpoint
            .as_deref()
            .unwrap_or("(AWS default addressing)");
        format!(
            "failed to create S3 client for region {} via {endpoint}",
            settings.region
        )
    })?;

    let (task_tx, mut task_rx) = tokio::sync::mpsc::unbounded_channel::<TaskMessage>();
    let bucket_tx = task_tx.clone();

    // Load buckets on startup
    state.loading_state = LoadingState::Loading("Loading buckets...".to_string());
    terminal.draw(|frame| ui::render(frame, &mut state))?;

    let client = s3_client.client.clone();
    let region_owned = settings.region.clone();
    tokio::spawn(async move {
        match S3Client::from_client(client, &region_owned)
            .list_buckets()
            .await
        {
            Ok(buckets) => {
                let _ = bucket_tx.send(TaskMessage::BucketsLoaded(Ok(buckets)));
            }
            Err(e) => {
                let _ = bucket_tx.send(TaskMessage::BucketsLoaded(Err(e.to_string())));
            }
        }
    });

    loop {
        terminal.draw(|frame| ui::render(frame, &mut state))?;

        // Drain task queue
        while let Ok(message) = task_rx.try_recv() {
            handle_task_message(&mut state, message, &s3_client, &task_tx);
        }

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            handle_key(&mut state, key.code, &s3_client, &task_tx).await?;
        }
        if state.should_quit {
            break;
        }
    }

    Ok(())
}

fn handle_task_message(
    state: &mut AppState,
    message: TaskMessage,
    s3_client: &S3Client,
    task_tx: &UnboundedSender<TaskMessage>,
) {
    match message {
        TaskMessage::BucketsLoaded(result) => match result {
            Ok(buckets) => {
                state.buckets = buckets;
                state.status_message = format!("Loaded {} bucket(s)", state.buckets.len());
            }
            Err(e) => {
                state.loading_state = LoadingState::Error(e.clone());
                state.status_message = format!("Failed to load buckets: {e}");
            }
        },
        TaskMessage::ObjectsLoaded {
            bucket,
            prefix,
            result,
        } => {
            match result {
                Ok(objects) => {
                    state.objects = objects;
                    state.current_bucket = Some(bucket.clone());
                    state.current_prefix = prefix.clone();
                    state.status_message =
                        format!("s3://{bucket}/{prefix} - {} item(s)", state.objects.len());
                }
                Err(e) => {
                    state.loading_state = LoadingState::Error(e.clone());
                    state.status_message = format!("Failed to load objects: {e}");
                }
            }
            state.loading_state = LoadingState::Idle;
        }
        TaskMessage::FilesUploaded { result } => {
            match result {
                Ok(report) => {
                    let ok = report.uploaded.len();
                    let fail = report.failures.len();
                    state.status_message = if fail == 0 {
                        format!("Uploaded {ok} file(s)")
                    } else {
                        format!(
                            "Uploaded {ok}, failed {fail}: {}",
                            report
                                .failures
                                .iter()
                                .map(|(path, _)| path.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                }
                Err(e) => state.status_message = format!("Upload failed: {e}"),
            }
            state.loading_state = LoadingState::Idle;
        }
        TaskMessage::ObjectDownloaded { result } => {
            match result {
                Ok(path) => state.status_message = format!("Downloaded to {path}"),
                Err(e) => state.status_message = format!("Download failed: {e}"),
            }
            state.loading_state = LoadingState::Idle;
        }
        TaskMessage::ObjectDeleted { result } => {
            match result {
                Ok(key) => {
                    state.status_message = format!("Deleted s3://{}", key);
                }
                Err(e) => state.status_message = format!("Delete failed: {e}"),
            }
            // Refresh the object list after a successful (or attempted) delete
            if let Some(bucket) = state.current_bucket.as_deref() {
                let prefix = state.current_prefix.clone();
                spawn_list_objects(s3_client, task_tx, bucket, &prefix);
                state.loading_state = LoadingState::Loading("Refreshing...".to_string());
            } else {
                state.loading_state = LoadingState::Idle;
            }
        }
        TaskMessage::ObjectDetailLoaded(result) => match result {
            Ok(detail) => {
                state.selected_object_detail = Some(detail.clone());
                state.current_panel = Panel::Preview;
                state.loading_state = LoadingState::Idle;
                state.status_message = "Press i to close info".to_string();
            }
            Err(e) => {
                state.loading_state = LoadingState::Idle;
                state.status_message = format!("Failed to load object info: {e}");
            }
        },
        TaskMessage::ObjectsDownloaded { result } => {
            match result {
                Ok(report) => {
                    let ok = report.downloaded.len();
                    let fail = report.failures.len();
                    state.status_message = if fail == 0 {
                        format!("Downloaded {ok} object(s)")
                    } else {
                        format!(
                            "Downloaded {ok}, failed {fail}: {}",
                            report
                                .failures
                                .iter()
                                .map(|(k, _)| k.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                }
                Err(e) => state.status_message = format!("Batch download failed: {e}"),
            }
            state.selected_keys.clear();
            state.loading_state = LoadingState::Idle;
        }
    }
}

async fn handle_key(
    state: &mut AppState,
    code: KeyCode,
    s3_client: &S3Client,
    task_tx: &UnboundedSender<TaskMessage>,
) -> Result<()> {
    if state.input_mode != InputMode::None {
        handle_input_key(state, code, s3_client, task_tx).await;
        return Ok(());
    }

    match code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            state.should_quit = true;
        }
        KeyCode::Char('?') => {
            state.status_message =
                "Help: q:quit | u:upload(multi) | g:download | d:delete | Space:select | a:all | c:clear | Enter:open | ←:back | ↑↓:nav"
                    .to_string();
        }
        KeyCode::Up | KeyCode::Char('k') => state.select_prev(),
        KeyCode::Down | KeyCode::Char('j') => state.select_next(),
        KeyCode::Enter => match state.current_panel {
            Panel::Buckets => {
                if let Some(bucket) = state.enter_bucket() {
                    spawn_list_objects(s3_client, task_tx, &bucket, "");
                    state.loading_state = LoadingState::Loading(format!("Loading {bucket}..."));
                }
            }
            Panel::Objects => {
                if let Some(prefix) = state.enter_folder() {
                    let bucket = state.current_bucket.clone().unwrap_or_default();
                    spawn_list_objects(s3_client, task_tx, &bucket, &prefix);
                    state.loading_state = LoadingState::Loading(format!("Loading {prefix}..."));
                }
            }
            _ => {}
        },
        KeyCode::Left | KeyCode::Char('h') => {
            if let Some((bucket, prefix)) = state.go_back() {
                spawn_list_objects(s3_client, task_tx, &bucket, &prefix);
                state.loading_state = LoadingState::Loading("Loading...".to_string());
            }
        }
        KeyCode::Char('u') | KeyCode::Char('U') => {
            if state.current_bucket.is_none() {
                state.status_message = "Select a bucket first (Enter) to upload files".to_string();
            } else {
                state.file_picker.reset();
                state.start_input(InputMode::FilePicker, "");
                state.pending_action = PendingAction::None;
                state.status_message = "Pick files to upload (Space marks, u uploads)".to_string();
            }
        }
        KeyCode::Char('g') | KeyCode::Char('G') => {
            if state.current_panel == Panel::Objects {
                let bucket = state.current_bucket.clone();
                let selected_keys = state.selected_keys_in_visible();
                let single = state
                    .selected_object()
                    .map(|o| (o.key.clone(), o.is_folder));

                if let Some(bucket) = bucket.as_deref()
                    && !selected_keys.is_empty()
                {
                    state.start_input(InputMode::Directory, "./");
                    state.pending_action = PendingAction::DownloadMany {
                        bucket: bucket.to_string(),
                        keys: selected_keys,
                    };
                } else if let (Some(bucket), Some((key, is_folder))) = (bucket.as_deref(), single)
                    && !is_folder
                {
                    state.start_input(InputMode::Directory, "./");
                    state.pending_action = PendingAction::Download {
                        bucket: bucket.to_string(),
                        key,
                    };
                }
            }
        }
        KeyCode::Char(' ') => {
            if state.current_panel == Panel::Objects && state.input_mode == InputMode::None {
                state.toggle_select_current();
            }
        }
        KeyCode::Char('a') | KeyCode::Char('A') => {
            if state.current_panel == Panel::Objects {
                state.select_all_visible();
                state.status_message = format!(
                    "Selected {} object(s)",
                    state.selected_keys_in_visible().len()
                );
            }
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if !state.selected_keys.is_empty() {
                state.clear_selection();
                state.status_message = "Selection cleared".to_string();
            }
        }
        KeyCode::Char('d') => {
            if state.current_panel == Panel::Objects {
                let bucket = state.current_bucket.clone();
                let selected = state
                    .selected_object()
                    .map(|o| (o.key.clone(), o.is_folder));
                if let (Some(bucket), Some((key, is_folder))) = (bucket, selected)
                    && !is_folder
                {
                    state.start_input(InputMode::Confirm, "");
                    state.pending_action = PendingAction::DeleteObject { bucket, key };
                }
            }
        }
        KeyCode::Char('i') | KeyCode::Char('I') => match state.current_panel {
            Panel::Objects => {
                let bucket = state.current_bucket.clone();
                let selected = state
                    .selected_object()
                    .map(|o| (o.key.clone(), o.is_folder));
                if let (Some(bucket), Some((key, is_folder))) = (bucket, selected)
                    && !is_folder
                {
                    state.loading_state =
                        LoadingState::Loading("Loading object info...".to_string());
                    spawn_get_object_info(s3_client, task_tx, &bucket, &key);
                }
            }
            Panel::Preview => {
                state.current_panel = Panel::Objects;
            }
            _ => {}
        },
        KeyCode::Char('/') if state.current_panel == Panel::Objects => {
            state.start_input(InputMode::Filter, "");
            state.status_message =
                "Filter: type to search, Enter to apply, Esc to clear".to_string();
        }
        _ => {}
    }

    Ok(())
}

async fn handle_input_key(
    state: &mut AppState,
    code: KeyCode,
    s3_client: &S3Client,
    task_tx: &UnboundedSender<TaskMessage>,
) {
    match state.input_mode {
        InputMode::Confirm => match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let action = std::mem::replace(&mut state.pending_action, PendingAction::None);
                state.cancel_input();
                if let PendingAction::DeleteObject { bucket, key } = action {
                    let tx = task_tx.clone();
                    let client = s3_client.client.clone();
                    let key_for_msg = key.clone();
                    tokio::spawn(async move {
                        let result = match S3Client::from_client(client, "")
                            .delete_object(&bucket, &key)
                            .await
                        {
                            Ok(()) => Ok(key),
                            Err(e) => Err(e.to_string()),
                        };
                        let _ = tx.send(TaskMessage::ObjectDeleted { result });
                    });
                    state.loading_state =
                        LoadingState::Loading(format!("Deleting {key_for_msg}..."));
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                state.cancel_input();
                state.status_message = "Action cancelled".to_string();
            }
            _ => {}
        },
        InputMode::FilePicker => match code {
            KeyCode::Up | KeyCode::Char('k') => state.file_picker.select_prev(),
            KeyCode::Down | KeyCode::Char('j') => state.file_picker.select_next(),
            KeyCode::Enter | KeyCode::Right => {
                if !state.file_picker.enter() {
                    state.file_picker.toggle_selected();
                }
            }
            KeyCode::Left => {
                if state.file_picker.filter.is_empty() {
                    state.file_picker.go_up();
                } else {
                    state.file_picker.filter.pop();
                    state.file_picker.selected_index = 0;
                }
            }
            KeyCode::Backspace => {
                if state.file_picker.filter.is_empty() {
                    state.file_picker.go_up();
                } else {
                    state.file_picker.filter.pop();
                    state.file_picker.selected_index = 0;
                }
            }
            KeyCode::Char(' ') => {
                let before = state.file_picker.selection_count();
                state.file_picker.toggle_selected();
                let after = state.file_picker.selection_count();
                state.status_message = if after > before {
                    format!("Marked ({after} selected) - press u to upload")
                } else if after < before {
                    format!("Unmarked ({after} selected)")
                } else {
                    format!("{after} selected (directories open with Enter)")
                };
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                state.file_picker.select_all_visible();
                let n = state.file_picker.selection_count();
                state.status_message = format!("Selected {n} file(s)");
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                state.file_picker.clear_selection();
                state.status_message = "Selection cleared".to_string();
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                if state.file_picker.selection_count() == 0 {
                    state.status_message =
                        "No files marked - press Space on a file first".to_string();
                } else {
                    let paths = state.file_picker.selected_paths();
                    state.start_metadata(paths);
                    state.status_message =
                        "Add metadata: 'key=value' Enter · '-key' removes · u to upload"
                            .to_string();
                }
            }
            KeyCode::Esc => {
                if state.file_picker.filter.is_empty() {
                    state.cancel_input();
                    state.status_message = "Upload cancelled".to_string();
                } else {
                    state.file_picker.filter.clear();
                    state.file_picker.selected_index = 0;
                    state.status_message = "Filter cleared".to_string();
                }
            }
            KeyCode::Char(c) => {
                state.file_picker.filter.push(c);
                state.file_picker.selected_index = 0;
            }
            _ => {}
        },
        InputMode::Metadata => match code {
            KeyCode::Up | KeyCode::Char('k') => state.metadata_editor.select_prev(),
            KeyCode::Down | KeyCode::Char('j') => state.metadata_editor.select_next(),
            KeyCode::Backspace => {
                state.input_buffer.pop();
            }
            KeyCode::Enter => {
                let line = state.input_buffer.trim().to_string();
                state.input_buffer.clear();
                match state.metadata_editor.apply_command(&line) {
                    Ok(()) => {
                        let entries = state
                            .metadata_editor
                            .selected_file()
                            .map(|(_, m)| m.len())
                            .unwrap_or(0);
                        state.status_message =
                            format!("Metadata updated ({entries} key(s) set for this file)");
                    }
                    Err(e) => state.status_message = e,
                }
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                let files = state.metadata_editor.to_upload();
                if files.is_empty() {
                    state.status_message = "No files marked for upload".to_string();
                } else if let Some(bucket) = state.current_bucket.clone() {
                    let prefix = state.current_prefix.clone();
                    let n = files.len();
                    state.cancel_input();
                    state.loading_state =
                        LoadingState::Loading(format!("Uploading {n} file(s)..."));
                    spawn_upload_many(s3_client, task_tx, &bucket, &prefix, files);
                } else {
                    state.status_message = "Select a bucket first".to_string();
                }
            }
            KeyCode::Esc => {
                if !state.input_buffer.is_empty() {
                    state.input_buffer.clear();
                } else {
                    state.start_input(InputMode::FilePicker, "");
                    state.pending_action = PendingAction::None;
                    state.status_message =
                        "Back to file picker - press u for metadata again".to_string();
                }
            }
            KeyCode::Char(c) => state.input_buffer.push(c),
            _ => {}
        },
        InputMode::Directory => match code {
            KeyCode::Char(c) => state.input_buffer.push(c),
            KeyCode::Backspace => {
                state.input_buffer.pop();
            }
            KeyCode::Enter => {
                let action = std::mem::replace(&mut state.pending_action, PendingAction::None);
                let dest_dir = state.input_buffer.trim().to_string();
                state.cancel_input();
                match action {
                    PendingAction::Download { bucket, key } => {
                        let base = dest_dir.trim_end_matches('/').to_string();
                        let dest_path = format!(
                            "{}/{}",
                            base,
                            Path::new(&key)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("download")
                        );
                        state.loading_state =
                            LoadingState::Loading(format!("Downloading {key}..."));
                        spawn_download(s3_client, task_tx, &bucket, &key, &dest_path);
                    }
                    PendingAction::DownloadMany { bucket, keys } => {
                        let n = keys.len();
                        state.loading_state = LoadingState::Loading(format!(
                            "Downloading {n} object(s) to {dest_dir}"
                        ));
                        spawn_download_many(s3_client, task_tx, &bucket, &keys, &dest_dir);
                    }
                    _ => {}
                }
            }
            KeyCode::Esc => {
                state.cancel_input();
                state.status_message = "Download cancelled".to_string();
            }
            _ => {}
        },
        InputMode::Filter => match code {
            KeyCode::Char(c) => {
                state.input_buffer.push(c);
                state.filter = state.input_buffer.clone();
                state.selected_index = 0;
            }
            KeyCode::Backspace => {
                state.input_buffer.pop();
                state.filter = state.input_buffer.clone();
                state.selected_index = 0;
            }
            KeyCode::Enter => {
                state.cancel_input();
                state.status_message = format!(
                    "Filter applied: '{}' - {} match(es)",
                    state.filter,
                    state.visible_count()
                );
            }
            KeyCode::Esc => {
                state.cancel_input();
                state.filter.clear();
                state.selected_index = 0;
                state.status_message = "Filter cleared".to_string();
            }
            _ => {}
        },
        InputMode::None => {}
    }
}

fn spawn_list_objects(
    s3_client: &S3Client,
    task_tx: &UnboundedSender<TaskMessage>,
    bucket: &str,
    prefix: &str,
) {
    let tx = task_tx.clone();
    let client = s3_client.client.clone();
    let bucket = bucket.to_string();
    let prefix = prefix.to_string();
    tokio::spawn(async move {
        let result = match S3Client::from_client(client, "")
            .list_objects(&bucket, &prefix)
            .await
        {
            Ok(objects) => Ok(objects),
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(TaskMessage::ObjectsLoaded {
            bucket,
            prefix,
            result,
        });
    });
}

fn spawn_upload_many(
    s3_client: &S3Client,
    task_tx: &UnboundedSender<TaskMessage>,
    bucket: &str,
    prefix: &str,
    files: Vec<(PathBuf, Vec<(String, String)>)>,
) {
    let tx = task_tx.clone();
    let client = s3_client.client.clone();
    let bucket = bucket.to_string();
    let prefix = prefix.to_string();
    tokio::spawn(async move {
        let report = S3Client::from_client(client, "")
            .upload_files(&bucket, &prefix, &files)
            .await;
        let _ = tx.send(TaskMessage::FilesUploaded { result: Ok(report) });
    });
}

fn spawn_download(
    s3_client: &S3Client,
    task_tx: &UnboundedSender<TaskMessage>,
    bucket: &str,
    key: &str,
    dest_path: &str,
) {
    let tx = task_tx.clone();
    let client = s3_client.client.clone();
    let bucket = bucket.to_string();
    let key = key.to_string();
    let dest_path = dest_path.to_string();
    tokio::spawn(async move {
        let result = match S3Client::from_client(client, "")
            .download_file(&bucket, &key, &dest_path)
            .await
        {
            Ok(()) => Ok(dest_path),
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(TaskMessage::ObjectDownloaded { result });
    });
}

fn spawn_download_many(
    s3_client: &S3Client,
    task_tx: &UnboundedSender<TaskMessage>,
    bucket: &str,
    keys: &[String],
    dest_dir: &str,
) {
    let tx = task_tx.clone();
    let client = s3_client.client.clone();
    let bucket = bucket.to_string();
    let keys: Vec<String> = keys.to_vec();
    let dest_dir = dest_dir.to_string();
    tokio::spawn(async move {
        let report = S3Client::from_client(client, "")
            .download_objects(&bucket, &keys, &dest_dir)
            .await;
        let _ = tx.send(TaskMessage::ObjectsDownloaded { result: Ok(report) });
    });
}

fn spawn_get_object_info(
    s3_client: &S3Client,
    task_tx: &UnboundedSender<TaskMessage>,
    bucket: &str,
    key: &str,
) {
    let tx = task_tx.clone();
    let client = s3_client.client.clone();
    let bucket = bucket.to_string();
    let key = key.to_string();
    tokio::spawn(async move {
        let result = match S3Client::from_client(client, "")
            .get_object_info(&bucket, &key)
            .await
        {
            Ok(detail) => Ok(detail),
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(TaskMessage::ObjectDetailLoaded(result));
    });
}

#[cfg(test)]
mod picker_flow_smoke {
    use super::*;
    use crate::config::StaticCredentials;

    #[tokio::test]
    async fn space_marks_and_u_uploads() {
        if std::env::var("S3_REGRESSION").as_deref() != Ok("1") {
            eprintln!("skipping (set S3_REGRESSION=1 to run)");
            return;
        }
        let dir = std::env::temp_dir().join(format!("s3tui-flow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("one.txt"), "1").unwrap();
        std::fs::write(dir.join("two.txt"), "2").unwrap();

        let s3_client = S3Client::new(
            "us-east-1",
            Some("http://pi:4566"),
            None,
            true,
            Some(&StaticCredentials {
                access_key_id: Some("test".to_string()),
                secret_access_key: Some("test".to_string()),
                session_token: None,
            }),
        )
        .await
        .unwrap();

        let bucket = "sm-picker-flow";
        if s3_client
            .client
            .head_bucket()
            .bucket(bucket)
            .send()
            .await
            .is_err()
        {
            s3_client
                .client
                .create_bucket()
                .bucket(bucket)
                .send()
                .await
                .ok();
        }

        let (task_tx, mut task_rx) = tokio::sync::mpsc::unbounded_channel::<TaskMessage>();
        let mut state = AppState::new("us-east-1".to_string());
        state.current_panel = Panel::Objects;
        state.current_bucket = Some(bucket.to_string());
        state.input_mode = InputMode::FilePicker;
        state.file_picker = crate::filepicker::FilePicker::new_at(dir.clone());

        // one.txt is entries[1], two.txt is entries[2].
        state.file_picker.selected_index = 1;
        handle_input_key(&mut state, KeyCode::Char(' '), &s3_client, &task_tx).await;
        state.file_picker.selected_index = 2;
        handle_input_key(&mut state, KeyCode::Char(' '), &s3_client, &task_tx).await;
        assert_eq!(
            state.file_picker.selection_count(),
            2,
            "space marks two files"
        );

        // 'u' now opens the metadata editor; add metadata to the first file,
        // then upload with 'u'.
        handle_input_key(&mut state, KeyCode::Char('u'), &s3_client, &task_tx).await;
        assert_eq!(
            state.input_mode,
            InputMode::Metadata,
            "u enters the metadata editor"
        );
        assert_eq!(state.metadata_editor.len(), 2, "both marked files listed");
        state.input_buffer = "env=prod".to_string();
        handle_input_key(&mut state, KeyCode::Enter, &s3_client, &task_tx).await;
        assert_eq!(
            state.metadata_editor.selected_file().unwrap().1,
            vec![("env".to_string(), "prod".to_string())],
            "metadata captured per file"
        );

        handle_input_key(&mut state, KeyCode::Char('u'), &s3_client, &task_tx).await;
        assert_eq!(
            state.input_mode,
            InputMode::None,
            "editor closed after upload"
        );

        let mut report = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while report.is_none() && std::time::Instant::now() < deadline {
            if let Ok(TaskMessage::FilesUploaded { result }) = task_rx.try_recv() {
                report = Some(result);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let result = report.expect("upload task reported back");
        let report = result.unwrap_or_else(|e| panic!("upload failed: {e}"));
        assert_eq!(report.uploaded.len(), 2, "both marked files uploaded");

        let head = s3_client
            .client
            .head_object()
            .bucket(bucket)
            .key("one.txt")
            .send()
            .await
            .unwrap();
        let meta = head
            .metadata()
            .and_then(|m| m.get("env"))
            .map(|v| v.as_str())
            .unwrap_or_default();
        assert_eq!(meta, "prod", "metadata landed on the uploaded object");

        let detail = s3_client
            .get_object_info(bucket, "one.txt")
            .await
            .expect("object info readable");
        let info_env = detail
            .metadata
            .iter()
            .find(|(k, _)| k == "env")
            .map(|(_, v)| v.as_str())
            .unwrap_or_default();
        assert_eq!(
            info_env, "prod",
            "ObjectDetail surfaces the upload metadata"
        );
        eprintln!(
            "FLOW OK: {report:?} meta.env={meta} info.metadata={:?}",
            detail.metadata
        );
    }
}
