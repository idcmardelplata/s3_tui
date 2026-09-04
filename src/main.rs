mod app;
mod errors;
mod s3;
mod ui;

use anyhow::{Context, Result};
use app::{AppState, InputMode, LoadingState, Panel, PendingAction, TaskMessage};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{Terminal, backend::CrosstermBackend};
use s3::S3Client;
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

const DEFAULT_ENDPOINT: &str = "http://pi:4566";

#[tokio::main]
async fn main() -> Result<()> {
    let region = std::env::var("AWS_REGION").unwrap_or_else(|_| "us-east-1".to_string());
    let endpoint = std::env::var("S3_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());

    let mut terminal = ratatui::init();
    let result = run_app(&mut terminal, &region, &endpoint).await;
    ratatui::restore();

    if let Err(err) = result {
        eprintln!("Application error: {err:#}");
    }

    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    region: &str,
    endpoint: &str,
) -> Result<()> {
    let mut state = AppState::new(region.to_string());
    let s3_client = S3Client::new_with_endpoint(region, Some(endpoint))
        .await
        .with_context(|| format!("failed to create S3 client with endpoint {endpoint}"))?;

    let (task_tx, mut task_rx) = tokio::sync::mpsc::unbounded_channel::<TaskMessage>();
    let bucket_tx = task_tx.clone();

    // Load buckets on startup
    state.loading_state = LoadingState::Loading("Loading buckets...".to_string());
    terminal.draw(|frame| ui::render(frame, &mut state))?;

    let client = s3_client.client.clone();
    let region_owned = region.to_string();
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
        TaskMessage::ObjectUploaded { result } => {
            match result {
                Ok(key) => state.status_message = format!("Uploaded s3://{}", key),
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
                "Help: q:quit | u:upload | g:download | d:delete | Enter:open | ←:back | ↑↓:nav"
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
            if state.current_panel == Panel::Objects && state.current_bucket.is_some() {
                state.start_input(InputMode::Path, "");
                state.pending_action = PendingAction::None;
            }
        }
        KeyCode::Char('g') | KeyCode::Char('G') => {
            if state.current_panel == Panel::Objects {
                let bucket = state.current_bucket.clone();
                let selected = state
                    .selected_object()
                    .map(|o| (o.key.clone(), o.is_folder));
                if let (Some(bucket), Some((key, is_folder))) = (bucket, selected)
                    && !is_folder
                {
                    state.start_input(InputMode::Directory, "./");
                    state.pending_action = PendingAction::Download { bucket, key };
                }
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
        InputMode::Path => match code {
            KeyCode::Char(c) => state.input_buffer.push(c),
            KeyCode::Backspace => {
                state.input_buffer.pop();
            }
            KeyCode::Enter => {
                let local_path = state.input_buffer.trim().to_string();
                state.cancel_input();
                if local_path.is_empty() {
                    state.status_message = "No file path provided".to_string();
                } else if let Some(bucket) = state.current_bucket.clone() {
                    let prefix = state.current_prefix.clone();
                    state.loading_state =
                        LoadingState::Loading(format!("Uploading {local_path}..."));
                    spawn_upload(s3_client, task_tx, &bucket, &prefix, &local_path);
                }
            }
            KeyCode::Esc => {
                state.cancel_input();
                state.status_message = "Upload cancelled".to_string();
            }
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
                if let PendingAction::Download { bucket, key } = action {
                    let base = dest_dir.trim_end_matches('/').to_string();
                    let dest_path = format!(
                        "{}/{}",
                        base,
                        Path::new(&key)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("download")
                    );
                    state.loading_state = LoadingState::Loading(format!("Downloading {key}..."));
                    spawn_download(s3_client, task_tx, &bucket, &key, &dest_path);
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

fn spawn_upload(
    s3_client: &S3Client,
    task_tx: &UnboundedSender<TaskMessage>,
    bucket: &str,
    prefix: &str,
    local_path: &str,
) {
    let tx = task_tx.clone();
    let client = s3_client.client.clone();
    let bucket = bucket.to_string();
    let prefix = prefix.to_string();
    let local_path = local_path.to_string();
    tokio::spawn(async move {
        let result = match S3Client::from_client(client, "")
            .upload_file(&bucket, &prefix, &local_path)
            .await
        {
            Ok(key) => Ok(format!("{bucket}/{key}")),
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(TaskMessage::ObjectUploaded { result });
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
