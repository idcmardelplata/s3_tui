use chrono::{DateTime, Utc};
use std::collections::HashSet;

use crate::filepicker::FilePicker;
use crate::ui::theme::{Theme, resolve};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Panel {
    Buckets,
    Objects,
    #[allow(dead_code)]
    Preview,
    #[allow(dead_code)]
    Help,
}

#[derive(Debug, Clone)]
pub enum LoadingState {
    Idle,
    Loading(String),
    Error(String),
    #[allow(dead_code)]
    Success(String),
}

#[derive(Debug, Clone)]
pub struct BucketInfo {
    pub name: String,
    pub creation_date: Option<DateTime<Utc>>,
    #[allow(dead_code)]
    pub region: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageClass {
    Folder,
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
    Directory,
    Filter,
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
}

/// Summary produced by a batch download of multiple objects.
#[derive(Debug, Clone, Default)]
pub struct DownloadReport {
    pub downloaded: Vec<String>,
    pub failures: Vec<(String, String)>,
}

/// Summary produced by a batch upload of multiple local files.
#[derive(Debug, Clone, Default)]
pub struct UploadReport {
    pub uploaded: Vec<String>,
    pub failures: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub enum PendingAction {
    None,
    DeleteObject { bucket: String, key: String },
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
    #[allow(dead_code)]
    pub scroll_offset: usize,
    pub loading_state: LoadingState,
    pub input_mode: InputMode,
    pub input_buffer: String,
    pub pending_action: PendingAction,
    pub status_message: String,
    pub should_quit: bool,
    #[allow(dead_code)]
    pub region: String,
    pub filter: String,
    pub selected_object_detail: Option<ObjectDetail>,
    /// Keys of objects currently multi-selected for batch operations.
    pub selected_keys: HashSet<String>,
    /// The active `ratatui-themekit` palette, resolved once at startup.
    pub theme: Theme,
    /// Local file browser used by the multi-file upload flow.
    pub file_picker: FilePicker,
}

impl AppState {
    pub fn new(region: String) -> Self {
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
            status_message: String::from("Press ? for help"),
            should_quit: false,
            region,
            filter: String::new(),
            selected_object_detail: None,
            selected_keys: HashSet::new(),
            theme: resolve(),
            file_picker: FilePicker::new(),
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

    /// Returns the currently selected object taking the filter into account.
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
            self.status_message = format!("Viewing s3://{name}/");
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
                    // Go back to buckets list
                    self.current_panel = Panel::Buckets;
                    self.current_bucket = None;
                    self.objects.clear();
                    self.selected_index = 0;
                    self.selected_keys.clear();
                    self.status_message = String::from("Press ? for help");
                    None
                } else {
                    // Go up one level in prefix
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

    /// Returns the keys currently marked for batch download.
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
