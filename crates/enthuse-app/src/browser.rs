//! Folder browser: lists sources, navigates folders and hands videos to the
//! player. Listing runs on a worker thread so a slow or offline share does
//! not freeze the UI.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use enthuse_core::resume::{Marker, marker};
use enthuse_core::settings::cycle;
use enthuse_core::source::{Entry, EntryKind, LocalFs, Source, SourceError, SourcePath};
use enthuse_core::storage::{SourceId, Store};
use enthuse_core::time::format_time;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};

use crate::{AppWindow, BrowserItem, BrowserWindow, bg};

const KIND_SOURCE: i32 = 0;
const KIND_FOLDER: i32 = 1;
const KIND_VIDEO: i32 = 2;

/// Stored `kind` for [`LocalFs`] sources.
const LOCAL: &str = "local";

/// A source and its id in the database.
#[derive(Clone)]
pub struct SourceRef {
    pub id: SourceId,
    pub source: Arc<dyn Source>,
}

/// Called when the user picks a video.
pub type PlayHandler = Box<dyn Fn(SourceRef, SourcePath)>;

struct SourceSlot {
    source: SourceRef,
    /// Shown under the name, e.g. the folder path.
    location: String,
}

/// Where the browser is: the source list, or a folder inside a source.
#[derive(Clone)]
enum Location {
    Sources,
    Folder { source: usize, path: SourcePath },
}

/// The window the browser shows in: the main window, or its own window
/// while the main window is the PiP player.
#[derive(Clone)]
pub enum View {
    Main(Weak<AppWindow>),
    Side(Weak<BrowserWindow>),
}

/// A live handle to the window of a [`View`].
enum Ui {
    Main(AppWindow),
    Side(BrowserWindow),
}

/// Both window types have the same `browser-*` properties; forward to
/// whichever one shows the browser.
macro_rules! ui_setters {
    ($($name:ident: $ty:ty),* $(,)?) => {
        impl Ui {
            $(fn $name(&self, value: $ty) {
                match self {
                    Ui::Main(w) => w.$name(value),
                    Ui::Side(w) => w.$name(value),
                }
            })*
        }
    };
}

ui_setters!(
    set_browser_error: SharedString,
    set_browser_loading: bool,
    set_browser_heading: SharedString,
    set_browser_can_go_up: bool,
    set_browser_at_sources: bool,
    set_browser_items: ModelRc<BrowserItem>,
    set_browser_resume_label: SharedString,
);

/// Connects a window's browser callbacks to `$browser`.
macro_rules! wire_callbacks {
    ($window:expr, $browser:expr) => {{
        let window = $window;
        let b = $browser.clone();
        window.on_browser_activate(move |i| b.activate(i as usize));
        let b = $browser.clone();
        window.on_browser_up(move || b.up());
        let b = $browser.clone();
        window.on_browser_remove(move |i| b.remove_source(i as usize));
        let b = $browser.clone();
        window.on_browser_cycle_resume(move || b.cycle_resume());
        let b = $browser.clone();
        window.on_add_source(move |path| {
            if let Some(index) = b.add_local(path.trim()) {
                b.navigate(Location::Folder {
                    source: index,
                    path: SourcePath::root(),
                });
            }
        });
    }};
}

pub struct Browser {
    view: RefCell<View>,
    store: Rc<Store>,
    sources: RefCell<Vec<SourceSlot>>,
    location: RefCell<Location>,
    entries: RefCell<Vec<Entry>>,
    /// Bumped on every navigation so late results from an earlier listing
    /// are ignored.
    generation: RefCell<u64>,
    on_play: RefCell<Option<PlayHandler>>,
}

impl Browser {
    /// Shows the sources saved in `store`.
    pub fn new(window: &AppWindow, store: Rc<Store>) -> Rc<Self> {
        let this = Rc::new(Self {
            view: RefCell::new(View::Main(window.as_weak())),
            store,
            sources: Default::default(),
            location: RefCell::new(Location::Sources),
            entries: Default::default(),
            generation: Default::default(),
            on_play: Default::default(),
        });
        this.load_sources();

        wire_callbacks!(window, this);

        this.navigate(Location::Sources);
        this
    }

    /// Lets `window` show the browser too, once [`Self::set_view`] picks it.
    pub fn attach_side(self: &Rc<Self>, window: &BrowserWindow) {
        wire_callbacks!(window, self);
    }

    /// Moves the browser to another window and shows the current folder
    /// there.
    pub fn set_view(self: &Rc<Self>, view: View) {
        *self.view.borrow_mut() = view;
        self.refresh();
    }

    fn ui(&self) -> Option<Ui> {
        match &*self.view.borrow() {
            View::Main(w) => w.upgrade().map(Ui::Main),
            View::Side(w) => w.upgrade().map(Ui::Side),
        }
    }

    pub fn set_play_handler(&self, handler: PlayHandler) {
        *self.on_play.borrow_mut() = Some(handler);
    }

    /// Adds and saves a local folder, mount or UNC path as a source, unless
    /// it is already there. Returns its index in the source list.
    pub fn add_local(&self, path: &str) -> Option<usize> {
        let path = normalize_location(path)?;
        let path = path.as_str();
        if let Some(i) = self
            .sources
            .borrow()
            .iter()
            .position(|s| s.location == path)
        {
            return Some(i);
        }
        let source = LocalFs::new(path);
        let id = match self.store.add_source(LOCAL, &source.display_name(), path) {
            Ok(id) => id,
            Err(err) => {
                self.show_error(&format!("Cannot save this source: {err}"));
                return None;
            }
        };
        self.sources.borrow_mut().push(SourceSlot {
            source: SourceRef {
                id,
                source: Arc::new(source),
            },
            location: path.to_owned(),
        });
        if matches!(*self.location.borrow(), Location::Sources) {
            self.show_sources();
        }
        Some(self.sources.borrow().len() - 1)
    }

    /// Lists the current folder again, e.g. to update watched markers after
    /// playback.
    pub fn refresh(self: &Rc<Self>) {
        let location = self.location.borrow().clone();
        self.navigate(location);
    }

    fn load_sources(&self) {
        let saved = match self.store.sources() {
            Ok(saved) => saved,
            Err(err) => {
                self.show_error(&format!("Cannot load saved sources: {err}"));
                return;
            }
        };
        let mut sources = self.sources.borrow_mut();
        for s in saved.into_iter().filter(|s| s.kind == LOCAL) {
            sources.push(SourceSlot {
                source: SourceRef {
                    id: s.id,
                    source: Arc::new(LocalFs::new(Path::new(&s.location))),
                },
                location: s.location,
            });
        }
    }

    fn remove_source(&self, index: usize) {
        if !matches!(*self.location.borrow(), Location::Sources) {
            return;
        }
        let Some(id) = self.sources.borrow().get(index).map(|s| s.source.id) else {
            return;
        };
        if let Err(err) = self.store.remove_source(id) {
            self.show_error(&format!("Cannot remove this source: {err}"));
            return;
        }
        self.sources.borrow_mut().remove(index);
        self.show_sources();
    }

    fn activate(self: &Rc<Self>, index: usize) {
        let location = self.location.borrow().clone();
        match location {
            Location::Sources => {
                if index < self.sources.borrow().len() {
                    self.navigate(Location::Folder {
                        source: index,
                        path: SourcePath::root(),
                    });
                }
            }
            Location::Folder { source, .. } => {
                let Some(entry) = self.entries.borrow().get(index).cloned() else {
                    return;
                };
                match entry.kind {
                    EntryKind::Folder => self.navigate(Location::Folder {
                        source,
                        path: entry.path,
                    }),
                    EntryKind::Video => {
                        let src = self.sources.borrow()[source].source.clone();
                        if let Some(play) = self.on_play.borrow().as_ref() {
                            play(src, entry.path);
                        }
                    }
                }
            }
        }
    }

    fn up(self: &Rc<Self>) {
        let location = self.location.borrow().clone();
        if let Location::Folder { source, path } = location {
            self.navigate(match path.parent() {
                Some(parent) => Location::Folder {
                    source,
                    path: parent,
                },
                None => Location::Sources,
            });
        }
    }

    fn navigate(self: &Rc<Self>, location: Location) {
        *self.location.borrow_mut() = location.clone();
        self.entries.borrow_mut().clear();
        let generation = {
            let mut g = self.generation.borrow_mut();
            *g += 1;
            *g
        };
        let Some(w) = self.ui() else {
            return;
        };
        w.set_browser_error(SharedString::new());
        self.update_resume_label();

        match location {
            Location::Sources => {
                w.set_browser_loading(false);
                self.show_sources();
            }
            Location::Folder { source, path } => {
                let sources = self.sources.borrow();
                let Some(slot) = sources.get(source) else {
                    return;
                };
                w.set_browser_heading(heading(&slot.source.source.display_name(), &path).into());
                w.set_browser_can_go_up(true);
                w.set_browser_at_sources(false);
                w.set_browser_loading(true);
                w.set_browser_items(ModelRc::default());

                let src = slot.source.clone();
                let id = src.id;
                let this = Rc::downgrade(self);
                let folder = path.clone();
                bg::spawn(
                    move || src.source.list(&folder),
                    move |result| {
                        // Ignore a listing the user already navigated away from.
                        if let Some(this) = this.upgrade()
                            && *this.generation.borrow() == generation
                        {
                            this.show_listing(id, &path, result);
                        }
                    },
                );
            }
        }
    }

    fn show_sources(&self) {
        let Some(w) = self.ui() else {
            return;
        };
        let items: Vec<_> = self
            .sources
            .borrow()
            .iter()
            .map(|s| BrowserItem {
                label: s.source.source.display_name().into(),
                detail: s.location.as_str().into(),
                kind: KIND_SOURCE,
                progress: -1.0,
                watched: false,
            })
            .collect();
        w.set_browser_heading("Sources".into());
        w.set_browser_can_go_up(false);
        w.set_browser_at_sources(true);
        w.set_browser_items(ModelRc::new(VecModel::from(items)));
    }

    fn show_listing(
        &self,
        source: SourceId,
        folder: &SourcePath,
        result: Result<Vec<Entry>, SourceError>,
    ) {
        let Some(w) = self.ui() else {
            return;
        };
        w.set_browser_loading(false);
        let entries = match result {
            Ok(entries) => entries,
            Err(err) => {
                w.set_browser_error(format!("Cannot open this folder: {err}").into());
                return;
            }
        };
        // Markers are a nice-to-have: show the listing even if this fails.
        let states = self.store.folder_states(source, folder).unwrap_or_default();
        let resume = self
            .store
            .playback_settings(source, folder)
            .map_or(true, |s| s.resume);
        let items: Vec<_> = entries
            .iter()
            .map(|e| {
                let mut item = BrowserItem {
                    label: e.name.as_str().into(),
                    detail: SharedString::new(),
                    kind: KIND_FOLDER,
                    progress: -1.0,
                    watched: false,
                };
                if e.kind == EntryKind::Video {
                    item.kind = KIND_VIDEO;
                    match marker(states.get(&e.path)) {
                        Marker::Unwatched => {}
                        Marker::Watched => {
                            item.watched = true;
                            item.detail = "Watched".into();
                        }
                        Marker::InProgress { position, fraction } => {
                            item.progress = fraction.unwrap_or(0.0) as f32;
                            item.detail = if resume {
                                format!("Resume at {}", format_time(position)).into()
                            } else {
                                "In progress".into()
                            };
                        }
                    }
                }
                item
            })
            .collect();
        *self.entries.borrow_mut() = entries;
        w.set_browser_items(ModelRc::new(VecModel::from(items)));
    }

    /// Steps the resume setting of the current folder (inherit, off, on), or
    /// toggles the default on the source list.
    fn cycle_resume(self: &Rc<Self>) {
        let location = self.location.borrow().clone();
        let result = match &location {
            Location::Sources => self
                .store
                .default_playback_settings()
                .and_then(|s| self.store.set_default_resume(!s.resume)),
            Location::Folder { source, path } => {
                let Some(id) = self.sources.borrow().get(*source).map(|s| s.source.id) else {
                    return;
                };
                self.store.folder_overrides(id).and_then(|o| {
                    let own = o.get(path).and_then(|o| o.resume);
                    self.store.set_folder_resume(id, path, cycle(own))
                })
            }
        };
        if let Err(err) = result {
            self.show_error(&format!("Cannot save this setting: {err}"));
        }
        self.update_resume_label();
        // "Resume at" markers depend on the setting.
        if matches!(location, Location::Folder { .. }) {
            self.refresh();
        }
    }

    fn update_resume_label(&self) {
        let Some(w) = self.ui() else {
            return;
        };
        let on_off = |on: bool| if on { "On" } else { "Off" };
        let label = match &*self.location.borrow() {
            Location::Sources => {
                let s = self.store.default_playback_settings().unwrap_or_default();
                format!("Resume: {}", on_off(s.resume))
            }
            Location::Folder { source, path } => {
                let Some(id) = self.sources.borrow().get(*source).map(|s| s.source.id) else {
                    return;
                };
                let own = self
                    .store
                    .folder_overrides(id)
                    .ok()
                    .and_then(|o| o.get(path).and_then(|o| o.resume));
                match own {
                    Some(on) => format!("Resume: {}", on_off(on)),
                    None => {
                        let resolved = self.store.playback_settings(id, path).unwrap_or_default();
                        format!("Resume: {} (inherited)", on_off(resolved.resume))
                    }
                }
            }
        };
        w.set_browser_resume_label(label.into());
    }

    pub fn show_error(&self, message: &str) {
        if let Some(w) = self.ui() {
            w.set_browser_error(message.into());
        }
    }
}

/// Makes a typed folder path absolute, so it means the same folder however
/// the app is started, and drops trailing separators, so `C:\Movies\` and
/// `C:\Movies` are one source. Not `canonicalize`: on Windows that turns
/// shares into `\\?\UNC\...` paths.
fn normalize_location(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let absolute = std::path::absolute(path).ok()?;
    let s = absolute.to_string_lossy();
    let trimmed = s.trim_end_matches(['/', '\\']);
    // Keep roots like "/" and "C:\" intact.
    let keep_root = trimmed.is_empty() || trimmed.ends_with(':');
    Some(if keep_root {
        s.into_owned()
    } else {
        trimmed.to_owned()
    })
}

/// "Movies / Action / 2020" style heading for a folder in a source.
fn heading(source_name: &str, path: &SourcePath) -> String {
    std::iter::once(source_name)
        .chain(path.segments())
        .collect::<Vec<_>>()
        .join(" / ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_locations() {
        assert_eq!(normalize_location("  "), None);
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(
            normalize_location("Movies").unwrap(),
            cwd.join("Movies").to_string_lossy()
        );
        #[cfg(unix)]
        {
            assert_eq!(normalize_location("/srv/media/").unwrap(), "/srv/media");
            assert_eq!(normalize_location("/").unwrap(), "/");
        }
        #[cfg(windows)]
        {
            assert_eq!(normalize_location(r"C:\Movies\").unwrap(), r"C:\Movies");
            assert_eq!(normalize_location(r"\\nas\share\").unwrap(), r"\\nas\share");
            assert_eq!(normalize_location(r"C:\").unwrap(), r"C:\");
        }
    }

    #[test]
    fn heading_joins_source_and_segments() {
        assert_eq!(heading("Movies", &SourcePath::root()), "Movies");
        let p = SourcePath::parse("Action/2020").unwrap();
        assert_eq!(heading("Movies", &p), "Movies / Action / 2020");
    }
}
