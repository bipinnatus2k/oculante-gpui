#[cfg(feature = "file_open")]
use crate::appstate::OculanteState;
use crate::thumbnails::Thumbnails;

use log::debug;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A drive as shown in the browser's sidebar.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Disk {
    pub name: String,
    pub path: PathBuf,
}

/// The listing state of a file browser dialog.
#[derive(Debug, Clone)]
pub struct BrowserState {
    pub filename: String,
    pub thumbnails: Thumbnails,
    pub search_term: String,
    pub search_active: bool,
    pub listview_active: bool,
    pub path_active: bool,
    pub entries: Option<Vec<PathBuf>>,
    pub last_dir: BrowserDir,
    pub drives: Option<Vec<Disk>>,
}

impl Default for BrowserState {
    fn default() -> Self {
        Self {
            filename: "unnamed.png".into(),
            thumbnails: Default::default(),
            search_term: Default::default(),
            search_active: Default::default(),
            listview_active: Default::default(),
            path_active: Default::default(),
            entries: Default::default(),
            last_dir: BrowserDir::default(),
            drives: Default::default(),
        }
    }
}

impl BrowserState {
    /// Whether the listing must be re-read, based on the current [`BrowserDir`].
    ///
    /// The entries are stale if the CWD changed.
    /// Refresh the file browser state if the CWD changed.
    /// This can mean that the current image's dir is different from the last image's dir or
    /// that the user switched file browser modes.
    ///
    /// So, the path should refresh if the user shift clicked previously (current image
    /// mode) and clicked this time (last dir mode) or vice versa.
    ///
    /// OR we should refresh the directory if the user shift clicked previously AND this time
    /// BUT the two images are in different directories.
    pub fn check_refresh_entries(&self, last_dir: BrowserDir, this_parent: Option<&Path>) -> bool {
        let last_parent_eq = self
            .entries
            .as_deref()
            .and_then(|entries| entries.first())
            .and_then(|entry| entry.parent())
            .map(|last_parent| Some(last_parent) == this_parent)
            .unwrap_or_default();

        self.last_dir != last_dir || self.last_dir == BrowserDir::CurrentImageDir || !last_parent_eq
    }

    /// Drop the current listing and remember which directory mode it was opened with,
    /// so the entries are scanned again.
    pub fn reset_for_dir(&mut self, last_dir: BrowserDir) {
        *self = Self {
            last_dir,
            ..Default::default()
        };
    }

    /// The drives of this machine, scanned once and then cached.
    pub fn refresh_drives(&mut self) {
        if self.drives.is_none() {
            self.drives = Some(list_drives());
        }
    }

    /// The scanned entries that match the current search term.
    pub fn filtered_entries(&self) -> Vec<&PathBuf> {
        filter_entries(
            self.entries.as_deref().unwrap_or_default(),
            &self.search_term,
        )
    }
}

/// Enumerate the drives that should show up in the browser.
///
/// On Windows every mount point is interesting, elsewhere only removable drives are listed.
pub fn list_drives() -> Vec<Disk> {
    let mut disks: Vec<Disk> = sysinfo::Disks::new_with_refreshed_list()
        .iter()
        .inspect(|d| debug!("d {:?}", d))
        .filter(|d| {
            if cfg!(target_os = "windows") {
                true
            } else {
                d.is_removable()
            }
        })
        .map(|d| Disk {
            name: if cfg!(target_os = "windows") {
                d.mount_point().to_string_lossy().to_string()
            } else {
                d.name().to_string_lossy().to_string()
            },
            path: d.mount_point().to_path_buf(),
        })
        .collect();
    disks.sort();
    disks.dedup_by(|a, b| a.name == b.name);
    disks
}

/// Read a directory for the browser listing.
///
/// Hidden entries (those starting with a dot) are skipped, as are files whose extension is
/// not in `filter`. Directories are sorted first, everything else case-insensitively by name.
/// `None` means the directory could not be read.
pub fn read_dir_entries(path: &Path, filter: &[&str]) -> Option<Vec<PathBuf>> {
    match fs::read_dir(path) {
        Ok(contents) => {
            debug!("Successfully read {}", path.display());
            let mut contents = contents
                .into_iter()
                .flatten()
                .filter(|de| !de.file_name().to_string_lossy().starts_with("."))
                .filter(|de| {
                    de.path().is_dir() || filter.contains(&de.path().ext().to_lowercase().as_str())
                })
                .map(|d| d.path())
                .collect::<Vec<_>>();

            contents.sort_by(|a, b| {
                a.file_name()
                    .map(|f| f.to_string_lossy().to_string())
                    .unwrap_or_default()
                    .to_lowercase()
                    .cmp(
                        &b.file_name()
                            .map(|f| f.to_string_lossy().to_string())
                            .unwrap_or_default()
                            .to_lowercase(),
                    )
            });
            contents.sort_by_key(|b| std::cmp::Reverse(b.is_dir()));
            Some(contents)
        }
        Err(_e) => None,
    }
}

/// Entries whose file name contains `search_term`, ignoring case.
pub fn filter_entries<'a>(entries: &'a [PathBuf], search_term: &str) -> Vec<&'a PathBuf> {
    entries
        .iter()
        .filter(|e| {
            e.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default()
                .to_lowercase()
                .contains(&search_term.to_lowercase())
        })
        .collect()
}

/// Make sure a save filename carries an extension the dialog offers.
///
/// When the current extension is not part of `filter`, the first filter item is used
/// instead (e.g. when saving a LUT as `.cube` rather than `.png`).
/// Returns the corrected filename together with the extension in use.
pub fn validated_filename(filename: &str, filter: &[&str]) -> (String, String) {
    let mut ext = Path::new(filename).ext();
    let mut filename = filename.to_string();

    if !filter.contains(&ext.as_str()) {
        ext = filter.first().map(|e| e.to_string()).unwrap_or(ext.clone());
        filename = Path::new(&filename)
            .with_extension(&ext)
            .to_string_lossy()
            .to_string();
    }

    (filename, ext)
}

trait PathExt {
    fn ext(&self) -> String;
}

impl PathExt for PathBuf {
    fn ext(&self) -> String {
        self.extension()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

impl PathExt for Path {
    fn ext(&self) -> String {
        self.extension()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default()
    }
}

// the native file dialog

/// Default directory for file browser.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BrowserDir {
    #[default]
    LastOpenDir,
    CurrentImageDir,
}

// Show file browser to select image to load
#[cfg(feature = "file_open")]
pub fn browse_for_image_path(state: &mut OculanteState) {
    let start_directory = state.filebrowser_path();
    let load_sender = state.load_channel.0.clone();
    state.redraw = true;
    std::thread::spawn(move || {
        let uppercase_lowercase_ext = [
            crate::utils::SUPPORTED_EXTENSIONS
                .iter()
                .map(|e| e.to_ascii_lowercase())
                .collect::<Vec<_>>(),
            crate::utils::SUPPORTED_EXTENSIONS
                .iter()
                .map(|e| e.to_ascii_uppercase())
                .collect::<Vec<_>>(),
        ]
        .concat();
        let file_dialog_result = rfd::FileDialog::new()
            .add_filter("All Supported Image Types", &uppercase_lowercase_ext)
            .add_filter("All File Types", &["*"])
            .set_directory(start_directory)
            .pick_file();
        if let Some(file_path) = file_dialog_result {
            let _ = load_sender.send(file_path);
        }
    });
}
