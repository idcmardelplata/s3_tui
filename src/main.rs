mod app;
mod cache;
mod cli;
mod config;
mod filepicker;
mod s3;
mod ui;

use anyhow::{Context, Result};
use app::{
    AppState, InputMode, LoadingState, MetadataField, Panel, PendingAction, StorageClass,
    TaskMessage,
};
use cache::ObjectCache;
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{Terminal, backend::CrosstermBackend};
use s3::S3Client;
use std::path::{Path, PathBuf};
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
        eprintln!("Error de la aplicación: {err:#}");
    }

    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    settings: &cli::ResolvedSettings,
) -> Result<()> {
    let mut state = AppState::new();
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

    ObjectCache::open_default().context("failed to open object cache (SQLite)")?;
    let cache_path = cache::cache_db_path();

    state.loading_state = LoadingState::Loading("Cargando buckets...".to_string());
    terminal.draw(|frame| ui::render(frame, &mut state))?;

    let client = s3_client.client.clone();
    tokio::spawn(async move {
        match S3Client::from_client(client).list_buckets().await {
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

        while let Ok(message) = task_rx.try_recv() {
            handle_task_message(&mut state, message, &s3_client, &task_tx, &cache_path);
        }

        if event::poll(Duration::from_millis(50))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            handle_key(&mut state, key.code, &s3_client, &task_tx, &cache_path).await?;
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
    cache_path: &Path,
) {
    match message {
        TaskMessage::BucketsLoaded(result) => match result {
            Ok(buckets) => {
                state.buckets = buckets;
                state.status_message = format!("Cargados {} bucket(s)", state.buckets.len());
            }
            Err(e) => {
                state.loading_state = LoadingState::Error(e.clone());
                state.status_message = format!("Error al cargar buckets: {e}");
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
                    state.status_message = format!(
                        "s3://{bucket}/{prefix} - {} elemento(s)",
                        state.objects.len()
                    );
                }
                Err(e) => {
                    state.loading_state = LoadingState::Error(e.clone());
                    state.status_message = format!("Error al cargar objetos: {e}");
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
                        format!("Subidos {ok} archivo(s)")
                    } else {
                        format!(
                            "Subidos {ok}, fallaron {fail}: {}",
                            report
                                .failures
                                .iter()
                                .map(|(path, _)| path.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                }
                Err(e) => state.status_message = format!("Error al subir: {e}"),
            }
            if let Some(bucket) = state.current_bucket.as_deref() {
                let prefix = state.current_prefix.clone();
                spawn_list_objects(s3_client, task_tx, cache_path, bucket, &prefix);
                state.loading_state = LoadingState::Loading("Actualizando...".to_string());
            } else {
                state.loading_state = LoadingState::Idle;
            }
        }
        TaskMessage::ObjectDownloaded { result } => {
            match result {
                Ok(path) => state.status_message = format!("Descargado en {path}"),
                Err(e) => state.status_message = format!("Error al descargar: {e}"),
            }
            state.loading_state = LoadingState::Idle;
        }
        TaskMessage::ObjectDeleted { result } => {
            match result {
                Ok(key) => {
                    state.status_message = format!("Eliminado s3://{}", key);
                }
                Err(e) => state.status_message = format!("Error al borrar: {e}"),
            }
            if let Some(bucket) = state.current_bucket.as_deref() {
                let prefix = state.current_prefix.clone();
                spawn_list_objects(s3_client, task_tx, cache_path, bucket, &prefix);
                state.loading_state = LoadingState::Loading("Actualizando...".to_string());
            } else {
                state.loading_state = LoadingState::Idle;
            }
        }
        TaskMessage::ObjectsDeleted { result } => {
            match result {
                Ok(keys) => {
                    state.status_message = format!("Eliminados {} objeto(s)", keys.len());
                }
                Err(e) => state.status_message = format!("Error en el borrado por lotes: {e}"),
            }
            state.clear_selection();
            if let Some(bucket) = state.current_bucket.as_deref() {
                let prefix = state.current_prefix.clone();
                spawn_list_objects(s3_client, task_tx, cache_path, bucket, &prefix);
                state.loading_state = LoadingState::Loading("Actualizando...".to_string());
            } else {
                state.loading_state = LoadingState::Idle;
            }
        }
        TaskMessage::ObjectDetailLoaded(result) => match result {
            Ok(detail) => {
                state.selected_object_detail = Some(detail.clone());
                state.current_panel = Panel::Preview;
                state.loading_state = LoadingState::Idle;
                state.status_message = "Presioná i para cerrar la info".to_string();
            }
            Err(e) => {
                state.loading_state = LoadingState::Idle;
                state.status_message = format!("Error al cargar la información del objeto: {e}");
            }
        },
        TaskMessage::ObjectsDownloaded { result } => {
            match result {
                Ok(report) => {
                    let ok = report.downloaded.len();
                    let fail = report.failures.len();
                    state.status_message = if fail == 0 {
                        format!("Descargados {ok} objeto(s)")
                    } else {
                        format!(
                            "Descargados {ok}, fallaron {fail}: {}",
                            report
                                .failures
                                .iter()
                                .map(|(k, _)| k.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                }
                Err(e) => state.status_message = format!("Error en la descarga por lotes: {e}"),
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
    cache_path: &Path,
) -> Result<()> {
    if state.show_help {
        match code {
            KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                state.show_help = false;
                state.help_scroll = 0;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                state.help_scroll = state.help_scroll.saturating_add(1);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                state.help_scroll = state.help_scroll.saturating_sub(1);
            }
            _ => {}
        }
        return Ok(());
    }

    if state.input_mode != InputMode::None {
        handle_input_key(state, code, s3_client, task_tx, cache_path).await;
        return Ok(());
    }

    match code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            state.should_quit = true;
        }
        KeyCode::Char('?') => {
            state.show_help = true;
            state.help_scroll = 0;
            state.status_message = "?:cierra la ayuda · ↑↓/jk:desplazar".to_string();
        }
        KeyCode::Up | KeyCode::Char('k') => state.select_prev(),
        KeyCode::Down | KeyCode::Char('j') => state.select_next(),
        KeyCode::Enter => match state.current_panel {
            Panel::Buckets => {
                if let Some(bucket) = state.enter_bucket()
                    && !prime_objects_from_cache(state, cache_path, &bucket, "")
                {
                    spawn_list_objects(s3_client, task_tx, cache_path, &bucket, "");
                    state.loading_state = LoadingState::Loading(format!("Cargando {bucket}..."));
                }
            }
            Panel::Objects => {
                if let Some(prefix) = state.enter_folder()
                    && let Some(bucket) = state.current_bucket.clone()
                    && !prime_objects_from_cache(state, cache_path, &bucket, &prefix)
                {
                    spawn_list_objects(s3_client, task_tx, cache_path, &bucket, &prefix);
                    state.loading_state = LoadingState::Loading(format!("Cargando {prefix}..."));
                }
            }
            _ => {}
        },
        KeyCode::Left | KeyCode::Char('h') => {
            if let Some((bucket, prefix)) = state.go_back() {
                spawn_list_objects(s3_client, task_tx, cache_path, &bucket, &prefix);
                state.loading_state = LoadingState::Loading("Cargando...".to_string());
            }
        }
        KeyCode::Char('u') | KeyCode::Char('U') => {
            if state.current_bucket.is_none() {
                state.status_message =
                    "Seleccioná un bucket primero (Enter) para subir archivos".to_string();
            } else {
                state.file_picker.reset();
                state.start_input(InputMode::FilePicker, "");
                state.pending_action = PendingAction::None;
                state.status_message =
                    "Elegí los archivos a subir (Espacio marca, u sube)".to_string();
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
                    "Seleccionados {} objeto(s)",
                    state.selected_keys_in_visible().len()
                );
            }
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            if !state.selected_keys.is_empty() {
                state.clear_selection();
                state.status_message = "Selección limpiada".to_string();
            }
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            if state.current_panel == Panel::Objects {
                let bucket = state.current_bucket.clone();
                if let Some(bucket) = bucket.as_deref() {
                    let prefix = state.current_prefix.clone();
                    spawn_list_objects(s3_client, task_tx, cache_path, bucket, &prefix);
                    state.loading_state =
                        LoadingState::Loading("Sincronizando desde S3...".to_string());
                }
            }
        }
        KeyCode::Char('d') => {
            if state.current_panel == Panel::Objects {
                let bucket = state.current_bucket.clone();
                let selected_keys = state.selected_keys_in_visible();
                let single = state
                    .selected_object()
                    .map(|o| (o.key.clone(), o.is_folder));

                if let Some(bucket) = bucket.as_deref()
                    && !selected_keys.is_empty()
                {
                    let n = selected_keys.len();
                    state.start_input(InputMode::Confirm, "");
                    state.status_message =
                        format!("¿Borrar {n} objeto(s) seleccionados? Presioná s para confirmar");
                    state.pending_action = PendingAction::DeleteMany {
                        bucket: bucket.to_string(),
                        keys: selected_keys,
                    };
                } else if let (Some(bucket), Some((key, is_folder))) = (bucket.as_deref(), single)
                    && !is_folder
                {
                    state.start_input(InputMode::Confirm, "");
                    state.pending_action = PendingAction::DeleteObject {
                        bucket: bucket.to_string(),
                        key,
                    };
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
                        LoadingState::Loading("Cargando información del objeto...".to_string());
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
                "Filtro: escribí para buscar, Enter aplica, Esc limpia".to_string();
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
    cache_path: &Path,
) {
    match state.input_mode {
        InputMode::Confirm => match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let action = std::mem::replace(&mut state.pending_action, PendingAction::None);
                state.cancel_input();
                match action {
                    PendingAction::DeleteObject { bucket, key } => {
                        let tx = task_tx.clone();
                        let client = s3_client.client.clone();
                        let key_for_msg = key.clone();
                        tokio::spawn(async move {
                            let result = match S3Client::from_client(client)
                                .delete_object(&bucket, &key)
                                .await
                            {
                                Ok(()) => Ok(key),
                                Err(e) => Err(e.to_string()),
                            };
                            let _ = tx.send(TaskMessage::ObjectDeleted { result });
                        });
                        state.loading_state =
                            LoadingState::Loading(format!("Borrando {key_for_msg}..."));
                    }
                    PendingAction::DeleteMany { bucket, keys } => {
                        let n = keys.len();
                        let tx = task_tx.clone();
                        let client = s3_client.client.clone();
                        tokio::spawn(async move {
                            let result = match S3Client::from_client(client)
                                .delete_objects(&bucket, &keys)
                                .await
                            {
                                Ok(deleted) => Ok(deleted),
                                Err(e) => Err(e.to_string()),
                            };
                            let _ = tx.send(TaskMessage::ObjectsDeleted { result });
                        });
                        state.clear_selection();
                        state.loading_state =
                            LoadingState::Loading(format!("Borrando {n} objeto(s)..."));
                    }
                    _ => {}
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                state.cancel_input();
                state.status_message = "Acción cancelada".to_string();
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
            KeyCode::Left | KeyCode::Backspace => {
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
                    format!("Marcado ({after} seleccionados) - presioná u para subir")
                } else if after < before {
                    format!("Desmarcado ({after} seleccionados)")
                } else {
                    format!("{after} seleccionados (las carpetas abren con Enter)")
                };
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                state.file_picker.select_all_visible();
                let n = state.file_picker.selection_count();
                state.status_message = format!("Seleccionados {n} elemento(s)");
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                state.file_picker.clear_selection();
                state.status_message = "Selección limpiada".to_string();
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                if state.file_picker.selection_count() == 0 {
                    state.status_message =
                        "No marcaste nada - presioná Espacio sobre un archivo o carpeta"
                            .to_string();
                } else {
                    let paths = state.file_picker.selected_paths();
                    state.start_metadata(paths);
                    state.status_message =
                        "Agregá metadatos en la tabla (A:fila nueva · Tab:cambia campo)"
                            .to_string();
                }
            }
            KeyCode::Esc => {
                if state.file_picker.filter.is_empty() {
                    state.cancel_input();
                    state.status_message = "Subida cancelada".to_string();
                } else {
                    state.file_picker.filter.clear();
                    state.file_picker.selected_index = 0;
                    state.status_message = "Filtro limpiado".to_string();
                }
            }
            KeyCode::Char(c) => {
                state.file_picker.filter.push(c);
                state.file_picker.selected_index = 0;
            }
            _ => {}
        },
        InputMode::Metadata => match code {
            KeyCode::Down => {
                state.metadata_editor.commit_cell(&state.input_buffer);
                state.input_buffer.clear();
                state.metadata_editor.move_row(1);
            }
            KeyCode::Up => {
                state.metadata_editor.commit_cell(&state.input_buffer);
                state.input_buffer.clear();
                state.metadata_editor.move_row(-1);
            }
            KeyCode::Tab => {
                state.metadata_editor.commit_cell(&state.input_buffer);
                state.input_buffer.clear();
                state.metadata_editor.toggle_field();
            }
            KeyCode::Enter => {
                state.metadata_editor.commit_cell(&state.input_buffer);
                state.input_buffer.clear();
                if state.metadata_editor.field == MetadataField::Key {
                    state.metadata_editor.field = MetadataField::Value;
                } else {
                    state.metadata_editor.advance_after_value();
                }
            }
            KeyCode::Char('A') => {
                state.metadata_editor.add_row();
                state.input_buffer.clear();
                state.status_message =
                    "Fila nueva - escribí la clave, Enter, y luego el valor".to_string();
            }
            KeyCode::Char('D') | KeyCode::Delete => {
                if state.metadata_editor.delete_row() {
                    state.input_buffer.clear();
                    state.status_message = "Fila eliminada".to_string();
                } else {
                    state.status_message =
                        "No hay nada para borrar - usá A para agregar una fila".to_string();
                }
            }
            KeyCode::Char('N') | KeyCode::BackTab => {
                state.metadata_editor.commit_cell(&state.input_buffer);
                state.input_buffer.clear();
                state.metadata_editor.select_next();
            }
            KeyCode::Char('P') => {
                state.metadata_editor.commit_cell(&state.input_buffer);
                state.input_buffer.clear();
                state.metadata_editor.select_prev();
            }
            KeyCode::Char('U') => {
                state.metadata_editor.commit_cell(&state.input_buffer);
                state.input_buffer.clear();
                let files = state.metadata_editor.to_upload();
                if files.is_empty() {
                    state.status_message = "No marcaste archivos para subir".to_string();
                } else if let Some(bucket) = state.current_bucket.clone() {
                    let prefix = state.current_prefix.clone();
                    let storage_class = state.metadata_editor.storage_class.clone();
                    let n = files.len();
                    state.cancel_input();
                    state.loading_state =
                        LoadingState::Loading(format!("Subiendo {n} archivo(s)..."));
                    spawn_upload_many(
                        s3_client,
                        task_tx,
                        cache_path,
                        &bucket,
                        &prefix,
                        files,
                        storage_class,
                    );
                } else {
                    state.status_message = "Seleccioná un bucket primero".to_string();
                }
            }
            KeyCode::Char('C') => {
                state.metadata_editor.commit_cell(&state.input_buffer);
                state.input_buffer.clear();
                state.metadata_editor.storage_class_index = StorageClass::uploadable()
                    .iter()
                    .position(|sc| *sc == state.metadata_editor.storage_class)
                    .unwrap_or(0);
                state.input_mode = InputMode::StorageClass;
                state.status_message = format!(
                    "Elegí la clase de almacenamiento (actual: {}) · ↑↓:elegir · Enter:confirmar · Esc:cancelar",
                    state.metadata_editor.storage_class
                );
            }
            KeyCode::Esc => {
                if !state.input_buffer.is_empty() {
                    state.input_buffer.clear();
                } else {
                    state.start_input(InputMode::FilePicker, "");
                    state.pending_action = PendingAction::None;
                    state.status_message =
                        "Volviste al selector de archivos - presioná u para metadatos".to_string();
                }
            }
            KeyCode::Char(c) => state.input_buffer.push(c),
            KeyCode::Backspace => {
                state.input_buffer.pop();
            }
            _ => {}
        },
        InputMode::StorageClass => match code {
            KeyCode::Up | KeyCode::Char('k') => {
                if state.metadata_editor.storage_class_index > 0 {
                    state.metadata_editor.storage_class_index -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let opts = StorageClass::uploadable();
                if state.metadata_editor.storage_class_index + 1 < opts.len() {
                    state.metadata_editor.storage_class_index += 1;
                }
            }
            KeyCode::Enter => {
                let opts = StorageClass::uploadable();
                if let Some(sc) = opts.get(state.metadata_editor.storage_class_index) {
                    state.metadata_editor.storage_class = sc.clone();
                }
                state.input_mode = InputMode::Metadata;
                state.status_message = format!(
                    "Clase de almacenamiento: {}",
                    state.metadata_editor.storage_class
                );
            }
            KeyCode::Esc => {
                state.input_mode = InputMode::Metadata;
                state.status_message = "Clase sin cambios".to_string();
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
                            LoadingState::Loading(format!("Descargando {key}..."));
                        spawn_download(s3_client, task_tx, &bucket, &key, &dest_path);
                    }
                    PendingAction::DownloadMany { bucket, keys } => {
                        let n = keys.len();
                        state.loading_state = LoadingState::Loading(format!(
                            "Descargando {n} objeto(s) a {dest_dir}"
                        ));
                        spawn_download_many(s3_client, task_tx, &bucket, &keys, &dest_dir);
                    }
                    _ => {}
                }
            }
            KeyCode::Esc => {
                state.cancel_input();
                state.status_message = "Descarga cancelada".to_string();
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
                    "Filtro aplicado: '{}' - {} coincidencia(s)",
                    state.filter,
                    state.visible_count()
                );
            }
            KeyCode::Esc => {
                state.cancel_input();
                state.filter.clear();
                state.selected_index = 0;
                state.status_message = "Filtro limpiado".to_string();
            }
            _ => {}
        },
        InputMode::None => {}
    }
}

/// Load the cached listing of `(bucket, prefix)` straight into the panel.
/// Returns `true` when the cache had an entry (so the caller can skip asking
/// S3 again); `false` when a remote first-list is needed.
fn prime_objects_from_cache(
    state: &mut AppState,
    cache_path: &Path,
    bucket: &str,
    prefix: &str,
) -> bool {
    if let Ok(db) = ObjectCache::open(cache_path)
        && db.listing_exists(bucket, prefix).unwrap_or(false)
    {
        state.objects = db
            .get_prefix(bucket, prefix)
            .ok()
            .flatten()
            .unwrap_or_default();
        state.status_message = format!("s3://{bucket}/{prefix} - desde cache local");
        return true;
    }
    false
}

fn spawn_list_objects(
    s3_client: &S3Client,
    task_tx: &UnboundedSender<TaskMessage>,
    cache_path: &Path,
    bucket: &str,
    prefix: &str,
) {
    let tx = task_tx.clone();
    let client = s3_client.client.clone();
    let bucket = bucket.to_string();
    let prefix = prefix.to_string();
    let cache_path = cache_path.to_path_buf();
    tokio::spawn(async move {
        let result = match S3Client::from_client(client)
            .list_objects(&bucket, &prefix)
            .await
        {
            Ok(remote) => {
                if let Ok(db) = ObjectCache::open(&cache_path) {
                    let _ = db.sync_listing(&bucket, &prefix, &remote);
                }
                Ok(remote)
            }
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
    cache_path: &Path,
    bucket: &str,
    prefix: &str,
    files: Vec<(PathBuf, Vec<(String, String)>)>,
    storage_class: StorageClass,
) {
    let tx = task_tx.clone();
    let client = s3_client.client.clone();
    let bucket = bucket.to_string();
    let prefix = prefix.to_string();
    let cache_path = cache_path.to_path_buf();
    tokio::spawn(async move {
        let report = S3Client::from_client(client)
            .upload_files(&bucket, &prefix, &files, &storage_class)
            .await;

        if let Ok(db) = ObjectCache::open(&cache_path) {
            let uploaded: std::collections::HashSet<String> = report
                .uploaded
                .iter()
                .filter_map(|s| s.strip_prefix(&format!("{bucket}/")))
                .map(str::to_string)
                .collect();
            for (path, key_suffix, _) in crate::s3::expand_upload_sources(&files) {
                let key = format!("{prefix}{key_suffix}");
                if !uploaded.contains(&key) {
                    continue;
                }
                let size = std::fs::metadata(&path).ok().map(|m| m.len()).unwrap_or(0);
                let _ = db.upsert_object(&bucket, &key, size, &storage_class);
            }
        }

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
        let result = match S3Client::from_client(client)
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
        let report = S3Client::from_client(client)
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
        let result = match S3Client::from_client(client)
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

    fn temp_cache() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "s3tui-test-cache-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        ObjectCache::open(&dir.join("cache.db")).unwrap();
        dir.join("cache.db")
    }

    #[tokio::test]
    async fn space_marks_and_u_uploads() {
        if std::env::var("S3_REGRESSION").as_deref() != Ok("1") {
            eprintln!("skipping (set S3_REGRESSION=1 to run)");
            return;
        }
        let cache_path = temp_cache();
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
        let mut state = AppState::new();
        state.current_panel = Panel::Objects;
        state.current_bucket = Some(bucket.to_string());
        state.input_mode = InputMode::FilePicker;
        state.file_picker = crate::filepicker::FilePicker::new_at(dir.clone());

        state.file_picker.selected_index = 1;
        handle_input_key(
            &mut state,
            KeyCode::Char(' '),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        state.file_picker.selected_index = 2;
        handle_input_key(
            &mut state,
            KeyCode::Char(' '),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        assert_eq!(
            state.file_picker.selection_count(),
            2,
            "space marks two files"
        );

        handle_input_key(
            &mut state,
            KeyCode::Char('u'),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        assert_eq!(
            state.input_mode,
            InputMode::Metadata,
            "u enters the metadata editor"
        );
        assert_eq!(state.metadata_editor.len(), 2, "both marked files listed");

        handle_input_key(
            &mut state,
            KeyCode::Char('A'),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        for c in "env".chars() {
            handle_input_key(
                &mut state,
                KeyCode::Char(c),
                &s3_client,
                &task_tx,
                &cache_path,
            )
            .await;
        }
        handle_input_key(
            &mut state,
            KeyCode::Enter,
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        for c in "prod".chars() {
            handle_input_key(
                &mut state,
                KeyCode::Char(c),
                &s3_client,
                &task_tx,
                &cache_path,
            )
            .await;
        }
        handle_input_key(
            &mut state,
            KeyCode::Enter,
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        assert_eq!(
            state.metadata_editor.selected_file().unwrap().1,
            vec![("env".to_string(), "prod".to_string())],
            "metadata captured per file"
        );

        handle_input_key(
            &mut state,
            KeyCode::Char('U'),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
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

    #[tokio::test]
    async fn storage_class_picker_opens_and_applies() {
        let client = aws_sdk_s3::Client::from_conf(
            aws_sdk_s3::config::Builder::new()
                .region(aws_config::Region::new("us-east-1"))
                .behavior_version_latest()
                .build(),
        );
        let s3_client = S3Client::from_client(client);
        let cache_path = temp_cache();
        let (task_tx, _task_rx) = tokio::sync::mpsc::unbounded_channel::<TaskMessage>();

        let mut state = AppState::new();
        state.input_mode = InputMode::Metadata;
        state.metadata_editor =
            crate::app::MetadataEditor::from_paths(vec![std::path::PathBuf::from("/tmp/a.txt")]);

        handle_input_key(
            &mut state,
            KeyCode::Char('C'),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        assert_eq!(
            state.input_mode,
            InputMode::StorageClass,
            "C opens the storage class picker"
        );
        assert_eq!(state.metadata_editor.storage_class, StorageClass::Standard);

        for _ in 0..4 {
            handle_input_key(&mut state, KeyCode::Down, &s3_client, &task_tx, &cache_path).await;
        }
        handle_input_key(
            &mut state,
            KeyCode::Enter,
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        assert_eq!(
            state.input_mode,
            InputMode::Metadata,
            "Enter returns to the metadata editor"
        );
        assert_eq!(
            state.metadata_editor.storage_class,
            StorageClass::Glacier,
            "scrolling 4 options reaches GLACIER"
        );
    }

    #[tokio::test]
    async fn upload_directory_tree() {
        if std::env::var("S3_REGRESSION").as_deref() != Ok("1") {
            eprintln!("skipping (set S3_REGRESSION=1 to run)");
            return;
        }
        let cache_path = temp_cache();
        let base = std::env::temp_dir().join(format!("s3tui-flow-tree-{}", std::process::id()));
        let docs = base.join("docs");
        std::fs::create_dir_all(docs.join("sub/deep")).unwrap();
        std::fs::write(docs.join("one.txt"), "1").unwrap();
        std::fs::write(docs.join("sub/two.log"), "2").unwrap();
        std::fs::write(docs.join("sub/deep/three.txt"), "3").unwrap();

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

        let bucket = "sm-dirupl-flow";
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
        let mut state = AppState::new();
        state.current_panel = Panel::Objects;
        state.current_bucket = Some(bucket.to_string());
        state.input_mode = InputMode::FilePicker;
        state.file_picker = crate::filepicker::FilePicker::new_at(base.clone());

        state.file_picker.selected_index = 1;
        handle_input_key(
            &mut state,
            KeyCode::Char(' '),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        assert_eq!(
            state.file_picker.selection_count(),
            1,
            "space marks the directory itself"
        );

        handle_input_key(
            &mut state,
            KeyCode::Char('u'),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        assert_eq!(
            state.input_mode,
            InputMode::Metadata,
            "u enters the metadata editor"
        );
        assert_eq!(state.metadata_editor.len(), 1, "single directory listed");

        handle_input_key(
            &mut state,
            KeyCode::Char('A'),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        for c in "env".chars() {
            handle_input_key(
                &mut state,
                KeyCode::Char(c),
                &s3_client,
                &task_tx,
                &cache_path,
            )
            .await;
        }
        handle_input_key(
            &mut state,
            KeyCode::Enter,
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        for c in "prod".chars() {
            handle_input_key(
                &mut state,
                KeyCode::Char(c),
                &s3_client,
                &task_tx,
                &cache_path,
            )
            .await;
        }
        handle_input_key(
            &mut state,
            KeyCode::Enter,
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        assert_eq!(
            state.metadata_editor.selected_file().unwrap().1,
            vec![("env".to_string(), "prod".to_string())],
            "directory metadata captured"
        );

        handle_input_key(
            &mut state,
            KeyCode::Char('U'),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await;
        assert_eq!(
            state.input_mode,
            InputMode::None,
            "editor closed after upload"
        );

        let mut report = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while report.is_none() && std::time::Instant::now() < deadline {
            if let Ok(TaskMessage::FilesUploaded { result }) = task_rx.try_recv() {
                report = Some(result);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let result = report.expect("upload task reported back");
        let report = result.unwrap_or_else(|e| panic!("upload failed: {e}"));
        assert_eq!(
            report.uploaded.len(),
            3,
            "the whole tree is uploaded, folder relative keys kept"
        );

        for key in [
            "docs/one.txt",
            "docs/sub/two.log",
            "docs/sub/deep/three.txt",
        ] {
            let head = s3_client
                .client
                .head_object()
                .bucket(bucket)
                .key(key)
                .send()
                .await
                .unwrap_or_else(|_| panic!("expected key {key}"));
            let meta = head
                .metadata()
                .and_then(|m| m.get("env"))
                .map(|v| v.as_str())
                .unwrap_or_default();
            assert_eq!(meta, "prod", "metadata inherited at {key}");
        }
        eprintln!("TREE-UPLOAD OK: {report:?}");
    }

    #[tokio::test]
    async fn select_all_and_batch_delete() {
        if std::env::var("S3_REGRESSION").as_deref() != Ok("1") {
            eprintln!("skipping (set S3_REGRESSION=1 to run)");
            return;
        }
        let cache_path = temp_cache();
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

        let bucket = "sm-multidel-flow";
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
        for key in ["alpha.txt", "beta.txt", "gamma.txt"] {
            s3_client
                .client
                .put_object()
                .bucket(bucket)
                .key(key)
                .body(aws_sdk_s3::primitives::ByteStream::from_static(
                    key.as_bytes(),
                ))
                .send()
                .await
                .expect("seed object");
        }
        let seeded = s3_client
            .list_objects(bucket, "")
            .await
            .expect("objects listed");

        let (task_tx, mut task_rx) = tokio::sync::mpsc::unbounded_channel::<TaskMessage>();
        let mut state = AppState::new();
        state.current_panel = Panel::Objects;
        state.current_bucket = Some(bucket.to_string());
        state.input_mode = InputMode::None;
        state.objects = seeded;

        handle_key(
            &mut state,
            KeyCode::Char('a'),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await
        .unwrap();
        assert_eq!(
            state.selected_keys.len(),
            3,
            "select-all marks every object"
        );
        handle_key(
            &mut state,
            KeyCode::Char('d'),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await
        .unwrap();
        assert_eq!(
            state.input_mode,
            InputMode::Confirm,
            "d with a non-empty selection opens confirm"
        );
        handle_key(
            &mut state,
            KeyCode::Char('y'),
            &s3_client,
            &task_tx,
            &cache_path,
        )
        .await
        .unwrap();
        assert_eq!(state.input_mode, InputMode::None, "confirm closes");
        assert!(state.selected_keys.is_empty(), "selection cleared");

        let mut deleted = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while deleted.is_none() && std::time::Instant::now() < deadline {
            if let Ok(TaskMessage::ObjectsDeleted { result }) = task_rx.try_recv() {
                deleted = Some(result);
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let deleted = deleted
            .expect("delete task reported back")
            .unwrap_or_else(|e| panic!("batch delete failed: {e}"));
        assert_eq!(deleted.len(), 3, "all three objects deleted");

        let listed = s3_client
            .list_objects(bucket, "")
            .await
            .expect("objects listed after delete");
        let remaining: Vec<&str> = listed
            .iter()
            .filter(|o| !o.is_folder)
            .map(|o| o.key.as_str())
            .collect();
        assert!(
            remaining.is_empty(),
            "bucket empty after batch delete: {remaining:?}"
        );
        eprintln!("MULTI-DELETE OK: removed {deleted:?}");
    }
}
