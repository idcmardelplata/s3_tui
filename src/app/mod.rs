use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::path::PathBuf;

use crate::filepicker::FilePicker;
use crate::ui::theme::{Theme, resolve};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Panel {
    Buckets,
    Objects,
    Preview,
}

#[derive(Debug, Clone)]
pub enum LoadingState {
    Idle,
    Loading(String),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct BucketInfo {
    pub name: String,
    pub creation_date: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum StorageClass {
    Folder,
    #[default]
    Standard,
    ReducedRedundancy,
    IntelligentTiering,
    Glacier,
    GlacierIr,
    StandardIa,
    OneZoneIa,
    ExpressOnezone,
    Outposts,
    DeepArchive,
    Snow,
    Unknown(String),
}

impl std::fmt::Display for StorageClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            StorageClass::Folder => "FOLDER",
            StorageClass::Standard => "STANDARD",
            StorageClass::ReducedRedundancy => "REDUCED_REDUNDANCY",
            StorageClass::IntelligentTiering => "INTELLIGENT_TIERING",
            StorageClass::Glacier => "GLACIER",
            StorageClass::GlacierIr => "GLACIER_IR",
            StorageClass::StandardIa => "STANDARD_IA",
            StorageClass::OneZoneIa => "ONEZONE_IA",
            StorageClass::ExpressOnezone => "EXPRESS_ONEZONE",
            StorageClass::Outposts => "OUTPOSTS",
            StorageClass::DeepArchive => "DEEP_ARCHIVE",
            StorageClass::Snow => "SNOW",
            StorageClass::Unknown(s) => s.as_str(),
        };
        f.write_str(label)
    }
}

impl StorageClass {
    /// Storage classes a batch upload can write objects with, in display order.
    pub fn uploadable() -> Vec<StorageClass> {
        vec![
            StorageClass::Standard,
            StorageClass::StandardIa,
            StorageClass::IntelligentTiering,
            StorageClass::OneZoneIa,
            StorageClass::Glacier,
            StorageClass::GlacierIr,
            StorageClass::DeepArchive,
            StorageClass::ReducedRedundancy,
        ]
    }
}

#[derive(Debug, Clone)]
pub struct ObjectInfo {
    pub key: String,
    pub size: Option<u64>,
    pub last_modified: Option<DateTime<Utc>>,
    pub is_folder: bool,
    pub storage_class: StorageClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputMode {
    None,
    Confirm,
    FilePicker,
    Metadata,
    Directory,
    Filter,
    StorageClass,
}

#[derive(Debug, Clone)]
pub struct ObjectDetail {
    pub bucket: String,
    pub key: String,
    pub size: Option<u64>,
    pub last_modified: Option<DateTime<Utc>>,
    pub storage_class: StorageClass,
    pub etag: Option<String>,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub metadata: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct DownloadReport {
    pub downloaded: Vec<String>,
    pub failures: Vec<(String, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct UploadReport {
    pub uploaded: Vec<String>,
    pub failures: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetadataField {
    #[default]
    Key,
    Value,
}

/// Per-file upload metadata editor: each marked file owns a small table of
/// `key`/`value` rows that is edited cell by cell before the upload runs.
#[derive(Debug, Clone, Default)]
pub struct MetadataEditor {
    pub files: Vec<(PathBuf, Vec<(String, String)>)>,
    pub selected: usize,
    pub row: usize,
    pub field: MetadataField,
    /// Storage class applied to every object of the batch upload.
    pub storage_class: StorageClass,
    /// Cursor index inside the storage-class picker popup.
    pub storage_class_index: usize,
}

impl MetadataEditor {
    pub fn from_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            files: paths.into_iter().map(|p| (p, Vec::new())).collect(),
            selected: 0,
            row: 0,
            field: MetadataField::Key,
            storage_class: StorageClass::Standard,
            storage_class_index: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn selected_file(&self) -> Option<&(PathBuf, Vec<(String, String)>)> {
        self.files.get(self.selected)
    }

    pub fn entries_len(&self) -> usize {
        self.files
            .get(self.selected)
            .map(|(_, e)| e.len())
            .unwrap_or(0)
    }

    /// Move the cursor onto the next/previous marked file.
    pub fn select_next(&mut self) {
        if self.selected + 1 < self.files.len() {
            self.selected += 1;
        }
        self.row = 0;
        self.field = MetadataField::Key;
    }

    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
        self.row = 0;
        self.field = MetadataField::Key;
    }

    /// Write `buffer` into the active cell. An empty buffer leaves it untouched.
    pub fn commit_cell(&mut self, buffer: &str) {
        if buffer.is_empty() {
            return;
        }
        if let Some((_, entries)) = self.files.get_mut(self.selected)
            && self.row < entries.len()
        {
            let value = buffer.to_string();
            match self.field {
                MetadataField::Key => entries[self.row].0 = value,
                MetadataField::Value => entries[self.row].1 = value,
            }
        }
    }

    pub fn toggle_field(&mut self) {
        self.field = match self.field {
            MetadataField::Key => MetadataField::Value,
            MetadataField::Value => MetadataField::Key,
        };
    }

    /// Move the row cursor by `delta` rows (clamped), keeping the active field.
    pub fn move_row(&mut self, delta: isize) {
        let len = self.entries_len();
        if len == 0 {
            return;
        }
        let next = (self.row as isize + delta).clamp(0, len as isize - 1) as usize;
        self.row = next;
    }

    /// Append a fresh empty row and move the cursor onto it.
    pub fn add_row(&mut self) {
        if let Some((_, entries)) = self.files.get_mut(self.selected) {
            entries.push((String::new(), String::new()));
            self.row = entries.len() - 1;
            self.field = MetadataField::Key;
        }
    }

    /// True when the row under the cursor has no committed key or value.
    pub fn cursor_row_is_empty(&self) -> bool {
        self.files
            .get(self.selected)
            .and_then(|(_, entries)| entries.get(self.row))
            .map(|(k, v)| k.is_empty() && v.is_empty())
            .unwrap_or(true)
    }

    /// After committing a value, move onto the next row; a fresh empty row is
    /// appended when the committed row was the last one and already holds
    /// content, so several metadata can be typed in sequence.
    pub fn advance_after_value(&mut self) {
        self.field = MetadataField::Key;
        if self.entries_len() == 0 {
            return;
        }
        if !self.cursor_row_is_empty() && self.row == self.entries_len() - 1 {
            self.add_row();
        } else {
            self.move_row(1);
        }
    }

    /// Remove the row under the cursor. Returns `false` when there is nothing to remove.
    pub fn delete_row(&mut self) -> bool {
        let Some((_, entries)) = self.files.get_mut(self.selected) else {
            return false;
        };
        if self.row >= entries.len() {
            return false;
        }
        entries.remove(self.row);
        if self.row >= entries.len() && !entries.is_empty() {
            self.row = entries.len() - 1;
        }
        self.field = MetadataField::Key;
        true
    }

    /// List of (path, metadata) pairs to hand to the uploader. Rows with an
    /// empty key are skipped (S3 rejects them).
    pub fn to_upload(&self) -> Vec<(PathBuf, Vec<(String, String)>)> {
        self.files
            .iter()
            .map(|(p, e)| {
                let kept = e.iter().filter(|(k, _)| !k.is_empty()).cloned().collect();
                (p.clone(), kept)
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    None,
    DeleteObject { bucket: String, key: String },
    DeleteMany { bucket: String, keys: Vec<String> },
    Download { bucket: String, key: String },
    DownloadMany { bucket: String, keys: Vec<String> },
}

#[derive(Debug)]
pub enum TaskMessage {
    BucketsLoaded(Result<Vec<BucketInfo>, String>),
    ObjectsLoaded {
        bucket: String,
        prefix: String,
        result: Result<Vec<ObjectInfo>, String>,
    },
    FilesUploaded {
        result: Result<UploadReport, String>,
    },
    ObjectDownloaded {
        result: Result<String, String>,
    },
    ObjectDeleted {
        result: Result<String, String>,
    },
    ObjectsDeleted {
        result: Result<Vec<String>, String>,
    },
    ObjectDetailLoaded(Result<ObjectDetail, String>),
    ObjectsDownloaded {
        result: Result<DownloadReport, String>,
    },
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub current_panel: Panel,
    pub buckets: Vec<BucketInfo>,
    pub objects: Vec<ObjectInfo>,
    pub current_bucket: Option<String>,
    pub current_prefix: String,
    pub selected_index: usize,
    pub scroll_offset: usize,
    pub loading_state: LoadingState,
    pub input_mode: InputMode,
    pub input_buffer: String,
    pub pending_action: PendingAction,
    pub status_message: String,
    pub should_quit: bool,
    pub filter: String,
    pub selected_object_detail: Option<ObjectDetail>,
    /// Keys of objects currently multi-selected for batch operations.
    pub selected_keys: HashSet<String>,
    /// The active `ratatui-themekit` palette, resolved once at startup.
    pub theme: Theme,
    /// Local file browser used by the multi-file upload flow.
    pub file_picker: FilePicker,
    /// Per-file metadata gathered right before the upload is started.
    pub metadata_editor: MetadataEditor,
    /// Whether the keyboard-shortcuts help panel is on screen.
    pub show_help: bool,
    /// Scroll offset used inside the help panel.
    pub help_scroll: usize,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            current_panel: Panel::Buckets,
            buckets: Vec::new(),
            objects: Vec::new(),
            current_bucket: None,
            current_prefix: String::new(),
            selected_index: 0,
            scroll_offset: 0,
            loading_state: LoadingState::Idle,
            input_mode: InputMode::None,
            input_buffer: String::new(),
            pending_action: PendingAction::None,
            status_message: String::from("Presioná ? para la ayuda"),
            should_quit: false,
            filter: String::new(),
            selected_object_detail: None,
            selected_keys: HashSet::new(),
            theme: resolve(),
            file_picker: FilePicker::new(),
            metadata_editor: MetadataEditor::default(),
            show_help: false,
            help_scroll: 0,
        }
    }

    /// Returns the indices (into `self.objects`) of objects matching the current filter.
    pub fn visible_indices(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            (0..self.objects.len()).collect()
        } else {
            let needle = self.filter.to_lowercase();
            self.objects
                .iter()
                .enumerate()
                .filter(|(_, o)| {
                    let name = o
                        .key
                        .strip_prefix(&self.current_prefix)
                        .unwrap_or(&o.key)
                        .to_lowercase();
                    name.contains(&needle) || o.key.to_lowercase().contains(&needle)
                })
                .map(|(i, _)| i)
                .collect()
        }
    }

    pub fn visible_count(&self) -> usize {
        self.visible_indices().len()
    }

    /// The currently selected object taking the filter into account.
    pub fn selected_object(&self) -> Option<&ObjectInfo> {
        let visible = self.visible_indices();
        visible.get(self.selected_index).map(|&i| &self.objects[i])
    }

    pub fn select_next(&mut self) {
        match self.current_panel {
            Panel::Buckets => {
                if !self.buckets.is_empty() && self.selected_index + 1 < self.buckets.len() {
                    self.selected_index += 1;
                }
            }
            Panel::Objects => {
                let count = self.visible_count();
                if count > 0 && self.selected_index + 1 < count {
                    self.selected_index += 1;
                }
            }
            _ => {}
        }
    }

    pub fn select_prev(&mut self) {
        match self.current_panel {
            Panel::Buckets => {
                if self.selected_index > 0 {
                    self.selected_index -= 1;
                }
            }
            Panel::Objects if self.selected_index > 0 => {
                self.selected_index -= 1;
            }
            _ => {}
        }
    }

    pub fn enter_bucket(&mut self) -> Option<String> {
        if self.current_panel == Panel::Buckets
            && let Some(bucket) = self.buckets.get(self.selected_index)
        {
            let name = bucket.name.clone();
            self.current_bucket = Some(name.clone());
            self.current_prefix.clear();
            self.objects.clear();
            self.selected_index = 0;
            self.selected_keys.clear();
            self.current_panel = Panel::Objects;
            self.status_message = format!("Viendo s3://{name}/");
            return Some(name);
        }
        None
    }

    pub fn enter_folder(&mut self) -> Option<String> {
        if self.current_panel == Panel::Objects
            && let Some(obj) = self.selected_object()
        {
            let key = obj.key.clone();
            if obj.is_folder {
                self.current_prefix = key.clone();
                self.objects.clear();
                self.selected_index = 0;
                self.selected_keys.clear();
                self.status_message = format!(
                    "s3://{}/{}/",
                    self.current_bucket.as_deref().unwrap_or(""),
                    self.current_prefix
                );
                return Some(key);
            }
        }
        None
    }

    pub fn go_back(&mut self) -> Option<(String, String)> {
        match self.current_panel {
            Panel::Objects => {
                if self.current_prefix.is_empty() {
                    self.current_panel = Panel::Buckets;
                    self.current_bucket = None;
                    self.objects.clear();
                    self.selected_index = 0;
                    self.selected_keys.clear();
                    self.status_message = String::from("Presioná ? para la ayuda");
                    None
                } else {
                    let old_prefix = self.current_prefix.clone();
                    if let Some(parent) = self.current_prefix.trim_end_matches('/').rfind('/') {
                        self.current_prefix = self.current_prefix[..=parent].to_string();
                    } else {
                        self.current_prefix.clear();
                    }
                    self.objects.clear();
                    self.selected_index = 0;
                    self.selected_keys.clear();
                    self.status_message = format!(
                        "s3://{}/{}/",
                        self.current_bucket.as_deref().unwrap_or(""),
                        self.current_prefix
                    );
                    Some((self.current_bucket.clone().unwrap_or_default(), old_prefix))
                }
            }
            Panel::Preview => {
                self.current_panel = Panel::Objects;
                None
            }
            _ => None,
        }
    }

    pub fn start_input(&mut self, mode: InputMode, initial: &str) {
        self.input_mode = mode;
        self.input_buffer = initial.to_string();
    }

    pub fn cancel_input(&mut self) {
        self.input_mode = InputMode::None;
        self.input_buffer.clear();
        self.pending_action = PendingAction::None;
    }

    /// Move the marked files from the picker into the metadata editor.
    pub fn start_metadata(&mut self, paths: Vec<PathBuf>) {
        self.metadata_editor = MetadataEditor::from_paths(paths);
        self.input_mode = InputMode::Metadata;
        self.input_buffer.clear();
        self.pending_action = PendingAction::None;
    }

    /// Toggle multi-selection for the currently selected object (files only).
    pub fn toggle_select_current(&mut self) {
        if self.current_panel == Panel::Objects
            && let Some(obj) = self.selected_object()
            && !obj.is_folder
        {
            let key = obj.key.clone();
            if !self.selected_keys.remove(&key) {
                self.selected_keys.insert(key);
            }
        }
    }

    /// Select all visible (non-folder) objects.
    pub fn select_all_visible(&mut self) {
        if self.current_panel == Panel::Objects {
            for idx in self.visible_indices() {
                if !self.objects[idx].is_folder {
                    self.selected_keys.insert(self.objects[idx].key.clone());
                }
            }
        }
    }

    /// Keys currently marked for batch download.
    pub fn selected_keys_in_visible(&self) -> Vec<String> {
        self.visible_indices()
            .iter()
            .filter_map(|&i| {
                let obj = &self.objects[i];
                if !obj.is_folder && self.selected_keys.contains(&obj.key) {
                    Some(obj.key.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Drop the whole multi-selection.
    pub fn clear_selection(&mut self) {
        self.selected_keys.clear();
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_editor_chains_multiple_rows() {
        let mut ed = MetadataEditor::from_paths(vec![PathBuf::from("/tmp/a.txt")]);
        ed.add_row();
        assert_eq!(ed.entries_len(), 1);
        assert!(ed.cursor_row_is_empty());

        ed.commit_cell("env");
        ed.toggle_field();
        ed.commit_cell("prod");
        ed.advance_after_value();

        assert_eq!(ed.entries_len(), 2, "a fresh row opens after the value");
        assert_eq!(ed.row, 1);
        assert!(ed.cursor_row_is_empty());
        assert_eq!(
            ed.to_upload()[0].1,
            vec![("env".to_string(), "prod".to_string())]
        );

        ed.commit_cell("team");
        ed.toggle_field();
        ed.commit_cell("infra");
        ed.advance_after_value();

        assert_eq!(ed.entries_len(), 3);
        assert_eq!(
            ed.to_upload()[0].1,
            vec![
                ("env".to_string(), "prod".to_string()),
                ("team".to_string(), "infra".to_string()),
            ]
        );
    }

    #[test]
    fn metadata_editor_does_not_grow_on_empty_commit() {
        let mut ed = MetadataEditor::from_paths(vec![PathBuf::from("/tmp/a.txt")]);
        ed.add_row();
        ed.commit_cell("env");
        ed.toggle_field();
        ed.commit_cell("prod");
        ed.advance_after_value();
        let rows = ed.entries_len();

        ed.advance_after_value();
        assert_eq!(ed.entries_len(), rows, "empty row does not create another");
        assert!(ed.cursor_row_is_empty());
    }

    #[test]
    fn metadata_editor_defaults_to_standard_storage_class() {
        let ed = MetadataEditor::from_paths(vec![PathBuf::from("/tmp/a.txt")]);
        assert_eq!(
            ed.storage_class,
            StorageClass::Standard,
            "uploads start on STANDARD"
        );
        assert_eq!(ed.storage_class_index, 0);
        assert!(
            StorageClass::uploadable()
                .iter()
                .any(|sc| *sc == ed.storage_class),
            "default class is in the picker list"
        );
    }
}
