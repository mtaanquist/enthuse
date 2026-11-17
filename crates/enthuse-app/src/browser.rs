//! Folder browser: lists sources, navigates folders and hands videos to the
//! player. Listing runs on a worker thread so a slow or offline share does
//! not freeze the UI.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use enthuse_core::folder::{Badge, FolderView, SortOrder};
use enthuse_core::settings::cycle;
use enthuse_core::source::{Entry, EntryKind, LocalFs, Source, SourceError, SourcePath};
use enthuse_core::storage::{SourceId, Store};
use enthuse_core::time::format_minutes;
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};

use crate::thumbs::{Thumbs, thumb_id};
use crate::{AppWindow, BrowserItem, BrowserModel, BrowserWindow, ItemBadge, ItemKind, bg};

/// How long a message from [`Browser::show_error`] stays.
const TOAST_TIME: std::time::Duration = std::time::Duration::from_secs(5);

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

impl Ui {
    /// The browser state of this window; both window types have one.
    fn model(&self) -> BrowserModel<'_> {
        match self {
            Ui::Main(w) => w.global::<BrowserModel>(),
            Ui::Side(w) => w.global::<BrowserModel>(),
        }
    }
}

/// Connects a window's browser callbacks to `$browser`.
macro_rules! wire_callbacks {
    ($window:expr, $browser:expr) => {{
        let model = $window.global::<BrowserModel>();
        let b = $browser.clone();
        model.on_activate(move |i| b.activate(i as usize));
        let b = $browser.clone();
        model.on_up(move || b.up());
        let b = $browser.clone();
        model.on_remove(move |i| b.remove_source(i as usize));
        let b = $browser.clone();
        model.on_cycle_resume(move || b.cycle_resume());
        let b = $browser.clone();
        model.on_cycle_sort(move || b.cycle_sort());
        let b = $browser.clone();
        model.on_crumb(move |i| b.crumb(i as usize));
        let b = $browser.clone();
        model.on_primary_action(move || b.primary_action());
        let b = $browser.clone();
        model.on_retry(move || b.refresh());
        let b = $browser.clone();
        model.on_add_source(move |path| {
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
    sort: Cell<SortOrder>,
    toast_timer: Timer,
    /// An error from loading the sources at start, shown once the window is.
    startup_error: RefCell<Option<String>>,
    /// Index in `entries` of the file the primary action plays.
    action: Cell<Option<usize>>,
    /// Bumped on every navigation so late results from an earlier listing
    /// are ignored.
    generation: RefCell<u64>,
    on_play: RefCell<Option<PlayHandler>>,
    /// Runs after a source is added or removed, or the browser refreshes
    /// (e.g. after playback), so Home can update.
    on_change: RefCell<Option<Box<dyn Fn()>>>,
    thumbs: RefCell<Option<Rc<Thumbs>>>,
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
            sort: Default::default(),
            toast_timer: Timer::default(),
            startup_error: Default::default(),
            action: Default::default(),
            generation: Default::default(),
            on_play: Default::default(),
            on_change: Default::default(),
            thumbs: Default::default(),
        });
        this.load_sources();

        wire_callbacks!(window, this);

        this.navigate(Location::Sources);
        if let Some(err) = this.startup_error.take() {
            this.show_error(&err);
        }
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

    /// Shows thumbnails from `thumbs` on video rows and cards.
    pub fn set_thumbs(self: &Rc<Self>, thumbs: &Rc<Thumbs>) {
        let weak = Rc::downgrade(self);
        thumbs.on_ready(move |id, image| {
            if let Some(this) = weak.upgrade() {
                this.thumb_ready(id, image);
            }
        });
        *self.thumbs.borrow_mut() = Some(thumbs.clone());
    }

    /// Puts a thumbnail that just arrived on its row, if it is shown.
    fn thumb_ready(&self, id: &str, image: &slint::Image) {
        let Location::Folder { source, .. } = *self.location.borrow() else {
            return;
        };
        let Some(source_id) = self.sources.borrow().get(source).map(|s| s.source.id) else {
            return;
        };
        let Some(index) = self
            .entries
            .borrow()
            .iter()
            .position(|e| e.kind == EntryKind::Video && thumb_id(source_id, &e.path) == id)
        else {
            return;
        };
        let Some(ui) = self.ui() else {
            return;
        };
        let items = ui.model().get_items();
        if let Some(mut item) = items.row_data(index) {
            item.thumbnail = image.clone();
            items.set_row_data(index, item);
        }
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

    pub fn set_change_handler(&self, handler: Box<dyn Fn()>) {
        *self.on_change.borrow_mut() = Some(handler);
    }

    /// Tells Home (or whoever listens) that sources or watch states changed.
    pub fn changed(&self) {
        if let Some(f) = self.on_change.borrow().as_ref() {
            f();
        }
    }

    /// The saved sources in display order, each with its location.
    pub fn sources(&self) -> Vec<(SourceRef, String)> {
        self.sources
            .borrow()
            .iter()
            .map(|s| (s.source.clone(), s.location.clone()))
            .collect()
    }

    /// The source with database id `id`, if it is still in the list.
    pub fn source_by_id(&self, id: SourceId) -> Option<SourceRef> {
        self.sources
            .borrow()
            .iter()
            .find(|s| s.source.id == id)
            .map(|s| s.source.clone())
    }

    /// Shows the root folder of the source at `index`.
    pub fn open_source(self: &Rc<Self>, index: usize) {
        if index < self.sources.borrow().len() {
            self.navigate(Location::Folder {
                source: index,
                path: SourcePath::root(),
            });
        }
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
        self.changed();
        Some(self.sources.borrow().len() - 1)
    }

    /// Lists the current folder again, e.g. to update watched markers after
    /// playback.
    pub fn refresh(self: &Rc<Self>) {
        let location = self.location.borrow().clone();
        self.navigate(location);
        self.changed();
    }

    fn load_sources(&self) {
        let saved = match self.store.sources() {
            Ok(saved) => saved,
            Err(err) => {
                *self.startup_error.borrow_mut() =
                    Some(format!("Cannot load saved sources: {err}"));
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

    /// Removes the source at `index` and forgets it in the database. If the
    /// browser is inside it, it goes back to the source list.
    pub fn remove_source(self: &Rc<Self>, index: usize) {
        let Some(id) = self.sources.borrow().get(index).map(|s| s.source.id) else {
            return;
        };
        if let Err(err) = self.store.remove_source(id) {
            self.show_error(&format!("Cannot remove this source: {err}"));
            return;
        }
        self.sources.borrow_mut().remove(index);
        let location = self.location.borrow().clone();
        match location {
            Location::Sources => self.show_sources(),
            Location::Folder { source, .. } if source == index => self.navigate(Location::Sources),
            Location::Folder { source, path } if source > index => {
                *self.location.borrow_mut() = Location::Folder {
                    source: source - 1,
                    path,
                };
            }
            Location::Folder { .. } => {}
        }
        self.changed();
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
        self.action.set(None);
        // The new folder asks for its own once listed.
        if let Some(thumbs) = self.thumbs.borrow().as_ref() {
            thumbs.clear_folder();
        }
        let Some(ui) = self.ui() else {
            return;
        };
        let w = ui.model();
        w.set_error(SharedString::new());
        w.set_action_text(SharedString::new());
        w.set_summary(SharedString::new());
        w.set_sort_label(self.sort.get().label().into());
        self.update_resume_label();

        match location {
            Location::Sources => {
                w.set_loading(false);
                self.show_sources();
            }
            Location::Folder { source, path } => {
                let sources = self.sources.borrow();
                let Some(slot) = sources.get(source) else {
                    return;
                };
                let crumbs = crumbs(&slot.source.source.display_name(), &path);
                w.set_title(crumbs.last().cloned().unwrap_or_default());
                w.set_crumbs(ModelRc::new(VecModel::from(crumbs)));
                w.set_can_go_up(true);
                w.set_at_sources(false);
                w.set_loading(true);
                w.set_items(ModelRc::default());

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
        let Some(ui) = self.ui() else {
            return;
        };
        let w = ui.model();
        let items: Vec<_> = self
            .sources
            .borrow()
            .iter()
            .map(|s| BrowserItem {
                label: s.source.source.display_name().into(),
                detail: s.location.as_str().into(),
                kind: ItemKind::Source,
                badge: ItemBadge::None,
                badge_text: SharedString::new(),
                progress: -1.0,
                thumbnail: Default::default(),
            })
            .collect();
        w.set_title("Sources".into());
        w.set_crumbs(ModelRc::default());
        w.set_summary(match items.len() {
            0 => SharedString::new(),
            1 => "1 source".into(),
            n => format!("{n} sources").into(),
        });
        w.set_can_go_up(false);
        w.set_at_sources(true);
        w.set_items(ModelRc::new(VecModel::from(items)));
    }

    fn show_listing(
        &self,
        source: SourceId,
        folder: &SourcePath,
        result: Result<Vec<Entry>, SourceError>,
    ) {
        let Some(ui) = self.ui() else {
            return;
        };
        let w = ui.model();
        w.set_loading(false);
        let mut entries = match result {
            Ok(entries) => entries,
            Err(err) => {
                w.set_error(format!("Can't open this folder: {err}").into());
                return;
            }
        };
        let sort = self.sort.get();
        entries.sort_by(|a, b| sort.compare(a, b));
        // Markers are a nice-to-have: show the listing even if this fails.
        let states = self.store.folder_states(source, folder).unwrap_or_default();
        let resume = self
            .store
            .playback_settings(source, folder)
            .map_or(true, |s| s.resume);
        let view = FolderView::new(&entries, &states, resume);
        let thumbs = self.thumbs.borrow().clone();
        let thumbnail = |e: &Entry| match (&thumbs, e.kind) {
            (Some(t), EntryKind::Video) => t.image(&thumb_id(source, &e.path)),
            _ => Default::default(),
        };
        let items: Vec<_> = entries
            .iter()
            .zip(&view.badges)
            .map(|(e, badge)| {
                let duration = states.get(&e.path).and_then(|s| s.duration);
                BrowserItem {
                    label: display_name(e).into(),
                    detail: duration.map(format_minutes).unwrap_or_default().into(),
                    kind: match e.kind {
                        EntryKind::Folder => ItemKind::Folder,
                        EntryKind::Video => ItemKind::Video,
                    },
                    badge: match badge {
                        Badge::None => ItemBadge::None,
                        Badge::Watched => ItemBadge::Watched,
                        Badge::Resume { .. } => ItemBadge::Resume,
                        Badge::New => ItemBadge::New,
                    },
                    badge_text: badge.label().into(),
                    progress: match badge {
                        Badge::Resume { fraction, .. } => fraction.unwrap_or(0.0) as f32,
                        _ => -1.0,
                    },
                    thumbnail: thumbnail(e),
                }
            })
            .collect();
        w.set_summary(view.summary().into());
        w.set_action_text(view.action_label(&entries).unwrap_or_default().into());
        self.action.set(view.action.as_ref().map(|a| a.index()));
        if let Some(thumbs) = &thumbs {
            let slot = self
                .sources
                .borrow()
                .iter()
                .find(|s| s.source.id == source)
                .map(|s| (s.source.source.clone(), s.location.clone()));
            if let Some((src, location)) = slot {
                let files: Vec<_> = entries
                    .iter()
                    .filter(|e| e.kind == EntryKind::Video)
                    .map(|e| e.path.clone())
                    .collect();
                thumbs.want_folder(source, &src, &location, &files);
            }
        }
        *self.entries.borrow_mut() = entries;
        w.set_items(ModelRc::new(VecModel::from(items)));
    }

    /// Plays the file behind the folder's primary action.
    fn primary_action(self: &Rc<Self>) {
        if let Some(index) = self.action.get() {
            self.activate(index);
        }
    }

    /// Goes to the folder at breadcrumb `index`: 0 is the source's root.
    fn crumb(self: &Rc<Self>, index: usize) {
        let location = self.location.borrow().clone();
        if let Location::Folder { source, path } = location {
            let mut target = SourcePath::root();
            for segment in path.segments().take(index) {
                match target.join(segment) {
                    Ok(p) => target = p,
                    Err(_) => return,
                }
            }
            self.navigate(Location::Folder {
                source,
                path: target,
            });
        }
    }

    fn cycle_sort(self: &Rc<Self>) {
        self.sort.set(self.sort.get().next());
        if let Some(ui) = self.ui() {
            ui.model().set_sort_label(self.sort.get().label().into());
        }
        self.rerender();
    }

    /// Shows the current listing again (re-sorted, new settings) without
    /// listing the folder again, so the list doesn't blank or scroll back.
    fn rerender(&self) {
        let location = self.location.borrow().clone();
        if let Location::Folder { source, path } = location {
            let Some(id) = self.sources.borrow().get(source).map(|s| s.source.id) else {
                return;
            };
            let entries = self.entries.borrow().clone();
            self.show_listing(id, &path, Ok(entries));
        }
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
        // The primary action depends on the setting.
        self.rerender();
    }

    fn update_resume_label(&self) {
        let Some(ui) = self.ui() else {
            return;
        };
        let w = ui.model();
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
        w.set_resume_label(label.into());
    }

    /// Shows `message` at the bottom for a few seconds, over whatever the
    /// browser shows (a failed save, a file that won't play).
    pub fn show_error(&self, message: &str) {
        let Some(ui) = self.ui() else {
            return;
        };
        ui.model().set_toast(message.into());
        let view = self.view.borrow().clone();
        let message = SharedString::from(message);
        self.toast_timer
            .start(TimerMode::SingleShot, TOAST_TIME, move || {
                let clear = |m: BrowserModel<'_>| {
                    if m.get_toast() == message {
                        m.set_toast(SharedString::new());
                    }
                };
                match &view {
                    View::Main(w) => {
                        if let Some(w) = w.upgrade() {
                            clear(w.global::<BrowserModel>());
                        }
                    }
                    View::Side(w) => {
                        if let Some(w) = w.upgrade() {
                            clear(w.global::<BrowserModel>());
                        }
                    }
                }
            });
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

/// Breadcrumb segments for a folder: the source's name, then the folders.
fn crumbs(source_name: &str, path: &SourcePath) -> Vec<SharedString> {
    std::iter::once(source_name)
        .chain(path.segments())
        .map(SharedString::from)
        .collect()
}

/// A video's name without its extension; folders keep theirs.
fn display_name(entry: &Entry) -> &str {
    match entry.kind {
        EntryKind::Video => entry
            .name
            .rsplit_once('.')
            .map_or(entry.name.as_str(), |(stem, _)| stem)
            .trim(),
        EntryKind::Folder => &entry.name,
    }
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
    fn crumbs_start_with_the_source() {
        assert_eq!(crumbs("Movies", &SourcePath::root()), ["Movies"]);
        let p = SourcePath::parse("Action/2020").unwrap();
        assert_eq!(crumbs("Movies", &p), ["Movies", "Action", "2020"]);
    }

    #[test]
    fn video_names_drop_the_extension() {
        let entry = |name: &str, kind| Entry {
            name: name.into(),
            path: SourcePath::parse(name).unwrap(),
            kind,
        };
        assert_eq!(
            display_name(&entry("E01 Low Water.mkv", EntryKind::Video)),
            "E01 Low Water"
        );
        assert_eq!(display_name(&entry("noext", EntryKind::Video)), "noext");
        assert_eq!(
            display_name(&entry("Season 1.5", EntryKind::Folder)),
            "Season 1.5"
        );
    }
}
