use std::path::PathBuf;

/// A single filesystem entry shown in the picker.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: Option<u64>,
}

/// Popup file browser used to pick one or more local files to upload.
///
/// Browsing keeps an explicit `selection` of full paths so files chosen in
/// different directories can be uploaded together. A fuzzy `filter` matches
/// entry names by subsequence and ranks the results by relevance.
#[derive(Debug, Clone)]
pub struct FilePicker {
    /// Directory currently listed.
    pub dir: PathBuf,
    /// All entries of `dir` (including a `..` parent marker when available).
    pub entries: Vec<FileEntry>,
    /// Index into the filtered, "visible" list.
    pub selected_index: usize,
    pub scroll_offset: usize,
    /// Full paths of the files chosen for upload (in selection order).
    pub selection: Vec<PathBuf>,
    /// Fuzzy query. Empty shows every entry.
    pub filter: String,
    /// One-line status message (errors, etc.).
    pub hint: String,
}

impl FilePicker {
    /// Open the picker in the process working directory.
    pub fn new() -> Self {
        let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::new_at(dir)
    }

    /// Open the picker in `dir`.
    pub fn new_at(dir: PathBuf) -> Self {
        let mut picker = Self {
            dir,
            entries: Vec::new(),
            selected_index: 0,
            scroll_offset: 0,
            selection: Vec::new(),
            filter: String::new(),
            hint: String::new(),
        };
        picker.load();
        picker
    }

    /// Prepare a fresh session: working directory, no selection, no filter.
    pub fn reset(&mut self) {
        self.dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.selection.clear();
        self.filter.clear();
        self.hint.clear();
        self.load();
    }

    /// Reload the current directory, keeping the selection.
    fn load(&mut self) {
        self.entries.clear();

        if let Some(parent) = self.dir.parent()
            && parent != self.dir.as_path()
        {
            self.entries.push(FileEntry {
                name: "..".to_string(),
                path: parent.to_path_buf(),
                is_dir: true,
                size: None,
            });
        }

        let mut listed: Vec<FileEntry> = Vec::new();
        match std::fs::read_dir(&self.dir) {
            Ok(rd) => {
                for entry in rd.flatten() {
                    let path = entry.path();
                    let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                    let size = (!is_dir)
                        .then(|| entry.metadata().ok().map(|m| m.len()))
                        .flatten();
                    listed.push(FileEntry {
                        name: entry.file_name().to_string_lossy().into_owned(),
                        path,
                        is_dir,
                        size,
                    });
                }
                self.hint.clear();
            }
            Err(e) => {
                self.hint = format!("Could not read directory: {e}");
            }
        }

        listed.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });
        self.entries.extend(listed);
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    /// Indices into `self.entries` that match the fuzzy filter, best matches
    /// first. An empty filter returns all entries in display order.
    pub fn visible_indices(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.entries.len()).collect();
        }
        let mut scored: Vec<(u32, usize)> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| fuzzy_score(&self.filter, &e.name).map(|s| (s, i)))
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| self.entries[a.1].name.cmp(&self.entries[b.1].name))
        });
        scored.into_iter().map(|(_, i)| i).collect()
    }

    pub fn visible_count(&self) -> usize {
        self.visible_indices().len()
    }

    fn clamp_index(&mut self) {
        let max = self.visible_count().saturating_sub(1);
        self.selected_index = self.selected_index.min(max);
    }

    pub fn select_next(&mut self) {
        self.clamp_index();
        if self.selected_index + 1 < self.visible_count() {
            self.selected_index += 1;
        }
    }

    pub fn select_prev(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
        }
    }

    /// The currently highlighted entry (taking the filter into account).
    pub fn selected_entry(&self) -> Option<&FileEntry> {
        let visible = self.visible_indices();
        visible
            .get(self.selected_index.min(visible.len().saturating_sub(1)))
            .map(|&i| &self.entries[i])
    }

    /// Add/remove the highlighted file (directories are ignored) from the
    /// upload selection. Returns `true` if the entry is currently selected.
    pub fn toggle_selected(&mut self) -> bool {
        let Some(entry) = self.selected_entry().cloned() else {
            return false;
        };
        if entry.is_dir {
            return false;
        }
        if let Some(pos) = self.selection.iter().position(|p| p == &entry.path) {
            self.selection.remove(pos);
            false
        } else {
            self.selection.push(entry.path);
            true
        }
    }

    /// Add every visible non-directory entry to the selection.
    pub fn select_all_visible(&mut self) {
        for idx in self.visible_indices() {
            let entry = &self.entries[idx];
            if !entry.is_dir && !self.selection.contains(&entry.path) {
                self.selection.push(entry.path.clone());
            }
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection.clear();
    }

    /// Full paths of the files chosen for upload.
    pub fn selected_paths(&self) -> Vec<PathBuf> {
        self.selection.clone()
    }

    pub fn selection_count(&self) -> usize {
        self.selection.len()
    }

    /// Enter the highlighted entry. Returns `true` when it was a directory
    /// (including the `..` parent marker) and the view changed.
    pub fn enter(&mut self) -> bool {
        let Some(entry) = self.selected_entry().cloned() else {
            return false;
        };
        if !entry.is_dir {
            return false;
        }
        if entry.name == ".." {
            self.go_up()
        } else {
            self.dir = entry.path;
            self.load();
            true
        }
    }

    /// Move up one level. Returns `false` when already at the filesystem root.
    pub fn go_up(&mut self) -> bool {
        let Some(parent) = self.dir.parent() else {
            return false;
        };
        if parent == self.dir.as_path() {
            return false;
        }
        self.dir = parent.to_path_buf();
        self.load();
        true
    }
}

impl Default for FilePicker {
    fn default() -> Self {
        Self::new()
    }
}

/// Score `query` against `text` as a case-insensitive fuzzy match.
///
/// Returns `Some(score)` when every character of `query` appears in `text` in
/// order; consecutive characters and word-start hits score higher. Returns
/// `None` otherwise.
pub fn fuzzy_score(query: &str, text: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(0);
    }
    let q: Vec<char> = query
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let t: Vec<char> = text
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();

    let mut query_idx = 0;
    let mut score: u32 = 0;
    let mut last_match: i32 = -2;

    for (i, &c) in t.iter().enumerate() {
        let Some(&qc) = q.get(query_idx) else {
            break;
        };
        if c == qc {
            if i as i32 == last_match + 1 {
                score += 3; // consecutive bonus
            } else {
                score += 1;
            }
            if i == 0 || !t[i - 1].is_alphanumeric() {
                score += 2; // word-start bonus
            }
            last_match = i as i32;
            query_idx += 1;
        }
    }

    (query_idx == q.len()).then_some(score)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("s3tui-fp-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_dirs_first_with_parent_marker() {
        let tmp = temp_dir("list");
        fs::create_dir_all(tmp.join("sub")).unwrap();
        fs::write(tmp.join("appa.txt"), "x").unwrap();
        fs::write(tmp.join("alpha.log"), "x").unwrap();

        let picker = FilePicker::new_at(tmp.clone());
        assert_eq!(picker.entries[0].name, "..");
        let names: Vec<&str> = picker.entries.iter().map(|e| e.name.as_str()).collect();
        // directories before files, each group alphabetical
        assert_eq!(names, vec!["..", "sub", "alpha.log", "appa.txt"]);
        assert!(picker.entries[0].is_dir);
        assert_eq!(picker.visible_count(), 4);
    }

    #[test]
    fn fuzzy_matches_subsequence_case_insensitive() {
        assert!(fuzzy_score("mk", "Makefile.txt").is_some());
        assert!(fuzzy_score("cargo", "src/Cargo.toml").is_some());
        assert_eq!(fuzzy_score("xyz", "Makefile.txt"), None);
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }

    #[test]
    fn fuzzy_prefers_consecutive_matches() {
        let consecutive = fuzzy_score("mk", "mk.txt").unwrap();
        let spread = fuzzy_score("mk", "myk.txt").unwrap();
        assert!(consecutive > spread);
    }

    #[test]
    fn filter_reranks_visible() {
        let tmp = temp_dir("filter");
        fs::write(tmp.join("mk.txt"), "x").unwrap();
        fs::write(tmp.join("myk.txt"), "x").unwrap();
        fs::write(tmp.join("nomatch.txt"), "x").unwrap();

        let mut picker = FilePicker::new_at(tmp.clone());
        picker.filter = "mk".to_string();
        let visible = picker.visible_indices();
        let first: Vec<&str> = visible
            .iter()
            .map(|&i| picker.entries[i].name.as_str())
            .collect();
        assert_eq!(first.len(), 2);
        assert_eq!(first[0], "mk.txt", "best match first");
        assert!(first.contains(&"myk.txt"));
    }

    #[test]
    fn toggling_adds_removes_and_dedupes() {
        let tmp = temp_dir("toggle");
        fs::write(tmp.join("a.txt"), "x").unwrap();
        fs::write(tmp.join("b.log"), "x").unwrap();

        let mut picker = FilePicker::new_at(tmp.clone());
        picker.selected_index = 1; // a.txt (idx 0 is "..")

        assert!(picker.toggle_selected(), "added");
        assert_eq!(picker.selection_count(), 1);
        assert!(!picker.toggle_selected(), "removed");
        assert_eq!(picker.selection_count(), 0);

        picker.toggle_selected();
        picker.select_all_visible();
        assert_eq!(picker.selection_count(), 2);
    }

    #[test]
    fn enter_navigates_into_subdirectory_and_up() {
        let tmp = temp_dir("nav");
        fs::create_dir_all(tmp.join("sub/inner")).unwrap();
        fs::write(tmp.join("sub/file.txt"), "x").unwrap();

        let mut picker = FilePicker::new_at(tmp.clone());
        picker.selected_index = 1; // "sub"
        assert!(picker.enter());
        assert_eq!(picker.dir, tmp.join("sub"));
        assert!(picker.entries.iter().any(|e| e.name == "file.txt"));

        assert!(picker.go_up());
        assert_eq!(picker.dir, tmp);

        assert!(picker.entries.iter().any(|e| e.name == ".."));
    }

    #[test]
    fn load_failure_sets_hint() {
        let missing = temp_dir("missing").join("nope");
        let picker = FilePicker::new_at(missing);
        // The ".." parent marker is still available to escape an unreadable dir.
        assert_eq!(picker.entries.len(), 1);
        assert_eq!(picker.entries[0].name, "..");
        assert!(!picker.hint.is_empty());
    }
}
