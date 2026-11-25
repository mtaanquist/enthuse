//! Folder browser: lists sources, navigates folders and hands videos to the
//! player. Listing runs on a worker thread so a slow or offline share does
//! not freeze the UI.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use enthuse_core::folder::{Badge, FolderView, SortOrder};
use enthuse_core::library::{drop_target, library_name};
use enthuse_core::settings::cycle;
use enthuse_core::source::{
    COMPUTER_NAME, Computer, Entry, EntryKind, LocalFs, Source, SourceError, SourcePath,
    relative_to,
};
use enthuse_core::storage::{SourceId, Store};
use enthuse_core::time::format_minutes;
use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak};

use crate::thumbs::{Thumbs, thumb_id};
use crate::{
    AppWindow, BrowserItem, BrowserModel, BrowserWindow, ItemBadge, ItemKind, SidebarModel, bg,
};

/// How long a message from [`Browser::show_error`] stays.
const TOAST_TIME: std::time::Duration = std::time::Duration::from_secs(5);

/// Stored `kind` for [`LocalFs`] sources.
const LOCAL: &str = "local";
/// Stored `kind` of the one [`Computer`] source, which holds the watch
/// state of files outside any library. Never listed as a library.
const COMPUTER: &str = "computer";

/// A source and its id in the database.
#[derive(Clone)]
pub struct SourceRef {
    pub id: SourceId,
    pub source: Arc<dyn Source>,
    /// The name it is shown with, which the user can change (#59).
    pub name: String,
}

/// Called when the user picks a video.
pub type PlayHandler = Box<dyn Fn(SourceRef, SourcePath)>;

/// A saved library, as listed.
#[derive(Clone)]
pub struct Library {
    pub source: SourceRef,
    /// Shown under the name, e.g. the folder path.
    pub location: String,
    /// Listed first (#59).
    pub favorite: bool,
    /// Left off Home; still in Libraries and Settings.
    pub hidden: bool,
}

/// A source the browser can be in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// The library at this index in the source list.
    Saved(usize),
    /// A folder opened without adding it as a library (#60), on the
    /// [`Computer`] source. Its top is [`Browser::unsaved_root`].
    Unsaved,
}

/// Where the browser is: the source list, or a folder inside a source.
#[derive(Clone)]
enum Location {
    Sources,
    Folder { source: Slot, path: SourcePath },
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
        model.on_add_library(move || b.save_unsaved());
        let b = $browser.clone();
        model.on_pick_folder(move || b.pick_folder());
        let b = $browser.clone();
        $window
            .global::<SidebarModel>()
            .on_show_libraries(move || b.show_libraries());
        let b = $browser.clone();
        model.on_toggle_favorite(move || b.toggle_favorite());
        let b = $browser.clone();
        model.on_add_source(move |path| {
            if let Some(index) = b.add_local(path.trim()) {
                b.navigate(Location::Folder {
                    source: Slot::Saved(index),
                    path: SourcePath::root(),
                });
            }
        });
    }};
}

pub struct Browser {
    view: RefCell<View>,
    store: Rc<Store>,
    sources: RefCell<Vec<Library>>,
    /// Files outside any library, by absolute path. `None` if the database
    /// could not save it; such files then cannot be opened.
    computer: Option<SourceRef>,
    /// The folder that was opened without a library, on `computer`.
    unsaved_root: RefCell<SourcePath>,
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
    on_change: RefCell<Vec<Box<dyn Fn()>>>,
    thumbs: RefCell<Option<Rc<Thumbs>>>,
    /// The folder picker is open.
    picking: Cell<bool>,
}

impl Browser {
    /// Shows the sources saved in `store`.
    pub fn new(window: &AppWindow, store: Rc<Store>) -> Rc<Self> {
        let computer = match store.add_source(COMPUTER, COMPUTER_NAME, "") {
            Ok(id) => Some(SourceRef {
                id,
                source: Arc::new(Computer),
                name: COMPUTER_NAME.to_owned(),
            }),
            Err(err) => {
                eprintln!("enthuse: cannot save files outside libraries: {err}");
                None
            }
        };
        let this = Rc::new(Self {
            view: RefCell::new(View::Main(window.as_weak())),
            store,
            sources: Default::default(),
            computer,
            unsaved_root: Default::default(),
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
            picking: Cell::new(false),
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
        let Some(source_id) = self.slot_source(source).map(|s| s.id) else {
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

    pub fn add_change_handler(&self, handler: Box<dyn Fn()>) {
        self.on_change.borrow_mut().push(handler);
    }

    /// Tells Home (or whoever listens) that sources or watch states changed.
    pub fn changed(&self) {
        for f in self.on_change.borrow().iter() {
            f();
        }
    }

    /// The saved libraries in display order.
    pub fn sources(&self) -> Vec<Library> {
        self.sources.borrow().clone()
    }

    /// Renames the library at `index`; an empty name goes back to the
    /// folder's own name.
    pub fn rename_source(self: &Rc<Self>, index: usize, name: &str) {
        let Some(library) = self.sources.borrow().get(index).cloned() else {
            return;
        };
        let name = library_name(name, &library.source.source.display_name());
        if let Err(err) = self.store.rename_source(library.source.id, &name) {
            self.show_error(&format!("Cannot rename this library: {err}"));
        }
        self.reload_sources();
    }

    /// Makes the library at `index` a favourite, listed first, or not.
    pub fn set_source_favorite(self: &Rc<Self>, index: usize, favorite: bool) {
        let Some(id) = self.sources.borrow().get(index).map(|s| s.source.id) else {
            return;
        };
        if let Err(err) = self.store.set_source_favorite(id, favorite) {
            self.show_error(&format!("Cannot save this favourite: {err}"));
        }
        self.reload_sources();
    }

    /// Moves the library at `from` to `to`, kept within its group
    /// (favourites, or the rest).
    pub fn move_source(self: &Rc<Self>, from: usize, to: usize) {
        let favorites: Vec<bool> = self.sources.borrow().iter().map(|s| s.favorite).collect();
        if let Some(target) = drop_target(&favorites, from, to) {
            self.reorder(from, target);
        }
    }

    /// Takes the library at `from` out and puts it back at `to`, and saves
    /// the order.
    fn reorder(self: &Rc<Self>, from: usize, to: usize) {
        let mut ids: Vec<SourceId> = self.sources.borrow().iter().map(|s| s.source.id).collect();
        let id = ids.remove(from);
        ids.insert(to, id);
        if let Err(err) = self.store.set_source_order(&ids) {
            self.show_error(&format!("Cannot save the order: {err}"));
        }
        self.reload_sources();
    }

    /// Everything starred, for Home: favourite libraries (as their root
    /// folder) in list order, then favourite folders in the order marked.
    pub fn favorites(&self) -> Vec<(Library, SourcePath)> {
        let mut out: Vec<_> = self
            .sources
            .borrow()
            .iter()
            .filter(|s| s.favorite)
            .map(|s| (s.clone(), SourcePath::root()))
            .collect();
        out.extend(self.favorite_folders());
        out
    }

    /// Favourite folders of libraries that are still there, in the order
    /// they were marked.
    fn favorite_folders(&self) -> Vec<(Library, SourcePath)> {
        let favorites = self.store.favorite_folders().unwrap_or_else(|err| {
            eprintln!("enthuse: cannot load favourite folders: {err}");
            Vec::new()
        });
        let sources = self.sources.borrow();
        favorites
            .into_iter()
            .filter_map(|(id, path)| {
                let library = sources.iter().find(|s| s.source.id == id)?;
                Some((library.clone(), path))
            })
            .collect()
    }

    /// Unstars a folder, or at a library's root the library.
    pub fn remove_favorite(self: &Rc<Self>, id: SourceId, path: &SourcePath) {
        if path.is_root() {
            let index = self.sources.borrow().iter().position(|s| s.source.id == id);
            if let Some(index) = index {
                self.set_source_favorite(index, false);
            }
            return;
        }
        if let Err(err) = self.store.set_folder_favorite(id, path, false) {
            self.show_error(&format!("Cannot save this favourite: {err}"));
        }
        self.update_favorite();
        self.changed();
    }

    /// Shows `path` in the library with id `id`, e.g. a favourite folder.
    pub fn open_folder(self: &Rc<Self>, id: SourceId, path: SourcePath) {
        let index = self.sources.borrow().iter().position(|s| s.source.id == id);
        if let Some(index) = index {
            self.navigate(Location::Folder {
                source: Slot::Saved(index),
                path,
            });
        }
    }

    /// The header's star: makes the folder shown a favourite, or not. At a
    /// library's root that is the library itself.
    fn toggle_favorite(self: &Rc<Self>) {
        let Location::Folder {
            source: Slot::Saved(index),
            path,
        } = self.location.borrow().clone()
        else {
            return;
        };
        let favorite = !self.is_favorite(index, &path);
        if path.is_root() {
            self.set_source_favorite(index, favorite);
            return;
        }
        let Some(id) = self.sources.borrow().get(index).map(|s| s.source.id) else {
            return;
        };
        if let Err(err) = self.store.set_folder_favorite(id, &path, favorite) {
            self.show_error(&format!("Cannot save this favourite: {err}"));
        }
        self.update_favorite();
        self.changed();
    }

    /// Shows the library at `index` on Home, or leaves it off. It stays in
    /// Libraries and Settings either way.
    pub fn set_source_hidden(self: &Rc<Self>, index: usize, hidden: bool) {
        let Some(id) = self.sources.borrow().get(index).map(|s| s.source.id) else {
            return;
        };
        if let Err(err) = self.store.set_source_hidden(id, hidden) {
            self.show_error(&format!("Cannot save this setting: {err}"));
        }
        self.reload_sources();
    }

    /// Whether the folder at `path` of library `index` is a favourite: the
    /// library itself at its root.
    fn is_favorite(&self, index: usize, path: &SourcePath) -> bool {
        let sources = self.sources.borrow();
        let Some(library) = sources.get(index) else {
            return false;
        };
        if path.is_root() {
            library.favorite
        } else {
            self.store
                .is_favorite_folder(library.source.id, path)
                .unwrap_or(false)
        }
    }

    /// Shows the header's star for the folder shown: only inside libraries.
    fn update_favorite(&self) {
        let Some(ui) = self.ui() else {
            return;
        };
        let w = ui.model();
        match &*self.location.borrow() {
            Location::Folder {
                source: Slot::Saved(index),
                path,
            } => {
                w.set_can_favorite(true);
                w.set_favorite(self.is_favorite(*index, path));
            }
            _ => {
                w.set_can_favorite(false);
                w.set_favorite(false);
            }
        }
    }

    /// The source with database id `id`, if it is still in the list.
    /// That includes the source of files outside any library.
    pub fn source_by_id(&self, id: SourceId) -> Option<SourceRef> {
        if let Some(computer) = self.computer.as_ref().filter(|c| c.id == id) {
            return Some(computer.clone());
        }
        self.sources
            .borrow()
            .iter()
            .find(|s| s.source.id == id)
            .map(|s| s.source.clone())
    }

    /// Where `path` of source `id` is, for telling network shares apart:
    /// the library's location, or the file's own path outside libraries.
    pub fn location_of(&self, id: SourceId, path: &SourcePath) -> String {
        if self.computer.as_ref().is_some_and(|c| c.id == id) {
            return Computer::os_path(path)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
        }
        self.sources
            .borrow()
            .iter()
            .find(|s| s.source.id == id)
            .map(|s| s.location.clone())
            .unwrap_or_default()
    }

    /// The source of `slot`, if it still exists.
    fn slot_source(&self, slot: Slot) -> Option<SourceRef> {
        match slot {
            Slot::Saved(i) => self.sources.borrow().get(i).map(|s| s.source.clone()),
            Slot::Unsaved => self.computer.clone(),
        }
    }

    /// The top folder of `slot`: the library's root, or the folder that
    /// was opened without a library.
    fn slot_top(&self, slot: Slot) -> SourcePath {
        match slot {
            Slot::Saved(_) => SourcePath::root(),
            Slot::Unsaved => self.unsaved_root.borrow().clone(),
        }
    }

    /// The name of `slot`'s top folder, first in the breadcrumb.
    fn slot_name(&self, slot: Slot) -> String {
        match slot {
            Slot::Saved(i) => self
                .sources
                .borrow()
                .get(i)
                .map(|s| s.source.name.clone())
                .unwrap_or_default(),
            Slot::Unsaved => {
                let top = self.unsaved_root.borrow();
                match top.name() {
                    Some(name) => name.to_owned(),
                    // A drive or "/": its OS path names it best.
                    None => Computer::os_path(&top)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|| COMPUTER_NAME.to_owned()),
                }
            }
        }
    }

    /// Where a file or folder from outside the app (dropped on the window,
    /// or on the command line) is: in the library that holds it, the
    /// deepest if libraries are nested, or else on the computer.
    fn resolve(&self, os: &Path) -> Option<(Slot, SourceRef, SourcePath)> {
        let os = std::path::absolute(os).ok()?;
        let sources = self.sources.borrow();
        let library = sources
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((i, s, relative_to(Path::new(&s.location), &os)?)))
            .max_by_key(|(_, s, _)| s.location.len());
        if let Some((i, slot, path)) = library {
            return Some((Slot::Saved(i), slot.source.clone(), path));
        }
        let computer = self.computer.clone()?;
        Some((Slot::Unsaved, computer, Computer::path_of(&os)?))
    }

    /// Plays a file from outside the app, resuming it if it was played
    /// before, whether or not a library holds it.
    pub fn play_external(&self, file: &Path) {
        let Some((_, source, path)) = self.resolve(file) else {
            self.show_error(&format!("Can't open {}", file.display()));
            return;
        };
        if let Some(play) = self.on_play.borrow().as_ref() {
            play(source, path);
        }
    }

    /// Shows a folder from outside the app: inside its library if one holds
    /// it, or else as a temporary location that is not saved (#60).
    pub fn open_external(self: &Rc<Self>, folder: &Path) {
        let Some((slot, _, path)) = self.resolve(folder) else {
            self.show_error(&format!("Can't open {}", folder.display()));
            return;
        };
        if slot == Slot::Unsaved {
            *self.unsaved_root.borrow_mut() = path.clone();
        }
        self.navigate(Location::Folder { source: slot, path });
    }

    /// Adds the folder that was opened without a library as a library,
    /// keeping its watch state, and stays in the same folder.
    fn save_unsaved(self: &Rc<Self>) {
        let Location::Folder {
            source: Slot::Unsaved,
            path,
        } = self.location.borrow().clone()
        else {
            return;
        };
        let top = self.unsaved_root.borrow().clone();
        let Some(os) = Computer::os_path(&top) else {
            return;
        };
        let Some(index) = self.add_local(&os.to_string_lossy()) else {
            return;
        };
        let library = self.sources.borrow()[index].source.id;
        if let Some(computer) = &self.computer
            && let Err(err) = self.store.move_folder_state(computer.id, &top, library)
        {
            self.show_error(&format!("Cannot keep the watch state: {err}"));
        }
        let path = below(&top, &path).unwrap_or_default();
        self.navigate(Location::Folder {
            source: Slot::Saved(index),
            path,
        });
        self.changed();
    }

    /// Asks for a folder with the system dialog, adds it as a library and
    /// opens it.
    fn pick_folder(self: &Rc<Self>) {
        if self.picking.replace(true) {
            return; // Already open.
        }
        let mut dialog = rfd::AsyncFileDialog::new().set_title("Add library");
        // Owned by the window that asked, so it stays on top of it.
        if let Some(ui) = self.ui() {
            match &ui {
                Ui::Main(w) => dialog = dialog.set_parent(&w.window().window_handle()),
                Ui::Side(w) => dialog = dialog.set_parent(&w.window().window_handle()),
            }
        }
        let picked = dialog.pick_folder();
        let this = Rc::downgrade(self);
        let spawned = slint::spawn_local(async move {
            let folder = picked.await;
            if let Some(this) = this.upgrade() {
                this.picking.set(false);
                if let Some(folder) = folder
                    && let Some(index) = this.add_local(&folder.path().to_string_lossy())
                {
                    this.open_source(index);
                }
            }
        });
        if spawned.is_err() {
            self.picking.set(false);
        }
    }

    /// Shows the list of libraries (Libraries in the sidebar, clicked again).
    pub fn show_libraries(self: &Rc<Self>) {
        self.navigate(Location::Sources);
    }

    /// Shows the root folder of the source at `index`.
    pub fn open_source(self: &Rc<Self>, index: usize) {
        if index < self.sources.borrow().len() {
            self.navigate(Location::Folder {
                source: Slot::Saved(index),
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
                self.show_error(&format!("Cannot save this library: {err}"));
                return None;
            }
        };
        self.load_sources();
        if matches!(*self.location.borrow(), Location::Sources) {
            self.show_sources();
        }
        self.changed();
        self.sources.borrow().iter().position(|s| s.source.id == id)
    }

    /// Lists the current folder again, e.g. to update watched markers after
    /// playback.
    pub fn refresh(self: &Rc<Self>) {
        let location = self.location.borrow().clone();
        self.navigate(location);
        self.changed();
    }

    /// Reads the libraries from the database, in their listed order.
    /// The browser's location is by index, so callers that may change the
    /// order use [`Self::reload_sources`].
    fn load_sources(&self) {
        let saved = match self.store.sources() {
            Ok(saved) => saved,
            Err(err) => {
                *self.startup_error.borrow_mut() =
                    Some(format!("Cannot load the libraries: {err}"));
                return;
            }
        };
        *self.sources.borrow_mut() = saved
            .into_iter()
            .filter(|s| s.kind == LOCAL)
            .map(|s| Library {
                source: SourceRef {
                    id: s.id,
                    source: Arc::new(LocalFs::new(Path::new(&s.location))),
                    name: s.name,
                },
                location: s.location,
                favorite: s.favorite,
                hidden: s.hidden,
            })
            .collect();
    }

    /// Reads the libraries again after a rename, favourite or move, and
    /// stays in the same folder (whose library may have a new index).
    fn reload_sources(self: &Rc<Self>) {
        let shown = match &*self.location.borrow() {
            Location::Folder {
                source: Slot::Saved(i),
                path,
            } => self
                .sources
                .borrow()
                .get(*i)
                .map(|s| (s.source.id, path.clone())),
            _ => None,
        };
        self.load_sources();
        if let Some((id, path)) = shown {
            let index = self.sources.borrow().iter().position(|s| s.source.id == id);
            *self.location.borrow_mut() = match index {
                Some(i) => Location::Folder {
                    source: Slot::Saved(i),
                    path,
                },
                None => Location::Sources,
            };
            // The breadcrumb shows the library's name.
            if let Some(i) = index
                && let Some(ui) = self.ui()
            {
                let name = self.slot_name(Slot::Saved(i));
                let location = self.location.borrow().clone();
                if let Location::Folder { path, .. } = location {
                    let crumbs = crumbs(&name, &path);
                    ui.model()
                        .set_title(crumbs.last().cloned().unwrap_or_default());
                    ui.model().set_crumbs(ModelRc::new(VecModel::from(crumbs)));
                }
            }
        }
        if matches!(*self.location.borrow(), Location::Sources) {
            self.show_sources();
        }
        self.update_favorite();
        self.changed();
    }

    /// Removes the source at `index` and forgets it in the database. If the
    /// browser is inside it, it goes back to the source list.
    pub fn remove_source(self: &Rc<Self>, index: usize) {
        let Some(id) = self.sources.borrow().get(index).map(|s| s.source.id) else {
            return;
        };
        if let Err(err) = self.store.remove_source(id) {
            self.show_error(&format!("Cannot remove this library: {err}"));
            return;
        }
        self.sources.borrow_mut().remove(index);
        let location = self.location.borrow().clone();
        match location {
            Location::Sources => self.show_sources(),
            Location::Folder {
                source: Slot::Saved(i),
                ..
            } if i == index => self.navigate(Location::Sources),
            Location::Folder {
                source: Slot::Saved(i),
                path,
            } if i > index => {
                *self.location.borrow_mut() = Location::Folder {
                    source: Slot::Saved(i - 1),
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
            Location::Sources => self.open_source(index),
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
                        let Some(src) = self.slot_source(source) else {
                            return;
                        };
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
            let parent = path.parent().filter(|_| path != self.slot_top(source));
            self.navigate(match parent {
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
        w.set_unsaved(matches!(
            location,
            Location::Folder {
                source: Slot::Unsaved,
                ..
            }
        ));
        self.update_resume_label();
        self.update_favorite();

        match location {
            Location::Sources => {
                w.set_loading(false);
                self.show_sources();
            }
            Location::Folder { source, path } => {
                let Some(src) = self.slot_source(source) else {
                    return;
                };
                let top = self.slot_top(source);
                let relative = below(&top, &path).unwrap_or_default();
                let crumbs = crumbs(&self.slot_name(source), &relative);
                w.set_title(crumbs.last().cloned().unwrap_or_default());
                w.set_crumbs(ModelRc::new(VecModel::from(crumbs)));
                w.set_can_go_up(true);
                w.set_at_sources(false);
                w.set_loading(true);
                w.set_items(ModelRc::default());

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
                label: s.source.name.as_str().into(),
                detail: s.location.as_str().into(),
                kind: ItemKind::Source,
                badge: ItemBadge::None,
                badge_text: SharedString::new(),
                progress: -1.0,
                thumbnail: Default::default(),
            })
            .collect();
        w.set_title("Libraries".into());
        w.set_crumbs(ModelRc::default());
        w.set_summary(match items.len() {
            0 => SharedString::new(),
            1 => "1 library".into(),
            n => format!("{n} libraries").into(),
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
        if let Some(thumbs) = &thumbs
            && let Some(src) = self.source_by_id(source).map(|s| s.source)
        {
            let location = self.location_of(source, folder);
            let files: Vec<_> = entries
                .iter()
                .filter(|e| e.kind == EntryKind::Video)
                .map(|e| e.path.clone())
                .collect();
            thumbs.want_folder(source, &src, &location, &files);
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

    /// Goes to the folder at breadcrumb `index`: 0 is the source's root
    /// (or the top of a folder opened without a library).
    fn crumb(self: &Rc<Self>, index: usize) {
        let location = self.location.borrow().clone();
        if let Location::Folder { source, path } = location {
            let top = self.slot_top(source);
            let relative = below(&top, &path).unwrap_or_default();
            let mut target = top;
            for segment in relative.segments().take(index) {
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

    /// A setting changed in Settings: show the resume label, the primary
    /// action and the thumbnails as they now are.
    pub fn settings_changed(&self) {
        self.update_resume_label();
        self.rerender();
    }

    /// Shows the current listing again (re-sorted, new settings) without
    /// listing the folder again, so the list doesn't blank or scroll back.
    fn rerender(&self) {
        let location = self.location.borrow().clone();
        if let Location::Folder { source, path } = location {
            let Some(id) = self.slot_source(source).map(|s| s.id) else {
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
                let Some(id) = self.slot_source(*source).map(|s| s.id) else {
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
                let Some(id) = self.slot_source(*source).map(|s| s.id) else {
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

/// `path` relative to `top`, if it is `top` or inside it.
fn below(top: &SourcePath, path: &SourcePath) -> Option<SourcePath> {
    let mut rest = path.segments();
    for segment in top.segments() {
        if rest.next()? != segment {
            return None;
        }
    }
    rest.try_fold(SourcePath::root(), |p, s| p.join(s).ok())
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
    fn below_strips_the_top_folder() {
        let p = |s: &str| SourcePath::parse(s).unwrap();
        assert_eq!(below(&p("C:/TV"), &p("C:/TV/Show/S1")), Some(p("Show/S1")));
        assert_eq!(below(&p("C:/TV"), &p("C:/TV")), Some(SourcePath::root()));
        assert_eq!(below(&SourcePath::root(), &p("a/b")), Some(p("a/b")));
        assert_eq!(below(&p("C:/TV"), &p("C:/TVshows/a")), None);
        assert_eq!(below(&p("C:/TV/Show"), &p("C:/TV")), None);
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
