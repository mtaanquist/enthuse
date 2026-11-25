//! Home screen: "Continue watching", favourite folders and the sources
//! (docs/ui.md, "Home").
//! Fills `HomeModel` in both windows, so Home looks the same in the
//! browser window during PiP.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use enthuse_core::folder::{continue_subtitle, display_title, progress};
use enthuse_core::source::{SourcePath, is_network_location};
use enthuse_core::storage::{SourceId, Store};
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::browser::{Browser, Library, SourceRef};
use crate::nav::Nav;
use crate::thumbs::{Thumbs, thumb_id};
use crate::{AppWindow, BrowserWindow, ContinueItem, HomeModel, SourceIcon, SourceTileItem, bg};

/// Cards in the "Continue watching" row.
const CONTINUE_LIMIT: usize = 4;

pub struct Home {
    main: Weak<AppWindow>,
    side: RefCell<Option<Weak<BrowserWindow>>>,
    store: Rc<Store>,
    browser: Rc<Browser>,
    nav: RefCell<Option<std::rc::Weak<Nav>>>,
    /// What each "Continue watching" card plays.
    items: RefCell<Vec<(SourceRef, SourcePath)>>,
    /// The "Continue watching" cards, for putting thumbnails on them.
    cards: Rc<VecModel<ContinueItem>>,
    thumbs: RefCell<Option<Rc<Thumbs>>>,
    /// Shown source tiles, for marking offline ones when the check returns.
    tiles: Rc<VecModel<SourceTileItem>>,
    /// What each favourite folder tile opens.
    favorites: RefCell<Vec<(SourceId, SourcePath)>>,
    /// Last reachability result per source: true when offline.
    offline: RefCell<HashMap<SourceId, bool>>,
    /// Sources whose reachability check is still running. A dead share can
    /// take a minute to fail; this keeps refreshes from piling up threads.
    checking: RefCell<HashSet<SourceId>>,
    /// The folder picker is open.
    picking: Cell<bool>,
}

/// Connects a window's Home callbacks to `$home`.
macro_rules! wire_callbacks {
    ($window:expr, $home:expr) => {{
        let model = $window.global::<HomeModel>();
        let h = $home.clone();
        model.on_play(move |i| h.play(i as usize));
        let h = $home.clone();
        model.on_open_source(move |i| h.browser.open_source(i as usize));
        let h = $home.clone();
        model.on_add_source(move || h.pick_folder());
        // Rebuilds the tiles, so it waits for the click (and the menu that
        // asked) to end.
        let h = $home.clone();
        model.on_toggle_favorite(move |i| {
            let h = h.clone();
            slint::Timer::single_shot(Duration::ZERO, move || {
                let index = i as usize;
                let favorite = h.browser.sources().get(index).is_some_and(|s| s.favorite);
                h.browser.set_source_favorite(index, !favorite);
            });
        });
        let h = $home.clone();
        model.on_open_favorite(move |i| {
            let favorite = h.favorites.borrow().get(i as usize).cloned();
            if let Some((id, path)) = favorite {
                h.browser.open_folder(id, path);
            }
        });
        let h = $home.clone();
        model.on_remove_favorite(move |i| {
            let h = h.clone();
            slint::Timer::single_shot(Duration::ZERO, move || {
                let favorite = h.favorites.borrow().get(i as usize).cloned();
                if let Some((id, path)) = favorite {
                    h.browser.remove_favorite(id, &path);
                }
            });
        });
        let h = $home.clone();
        model.on_refresh(move || h.refresh());
    }};
}

impl Home {
    pub fn new(window: &AppWindow, store: Rc<Store>, browser: Rc<Browser>) -> Rc<Self> {
        let this = Rc::new(Self {
            main: window.as_weak(),
            side: Default::default(),
            store,
            browser,
            nav: Default::default(),
            items: Default::default(),
            cards: Rc::new(VecModel::default()),
            thumbs: Default::default(),
            tiles: Rc::new(VecModel::default()),
            favorites: Default::default(),
            offline: Default::default(),
            checking: Default::default(),
            picking: Cell::new(false),
        });
        wire_callbacks!(window, this);
        let weak = Rc::downgrade(&this);
        this.browser.add_change_handler(Box::new(move || {
            if let Some(home) = weak.upgrade() {
                home.refresh();
            }
        }));
        this.refresh();
        this
    }

    /// Lets the browser window show Home too.
    pub fn attach_side(self: &Rc<Self>, window: &BrowserWindow) {
        wire_callbacks!(window, self);
        *self.side.borrow_mut() = Some(window.as_weak());
        self.refresh();
    }

    /// Shows thumbnails from `thumbs` on the "Continue watching" cards.
    pub fn set_thumbs(self: &Rc<Self>, thumbs: &Rc<Thumbs>) {
        let weak = Rc::downgrade(self);
        thumbs.on_ready(move |id, image| {
            let Some(this) = weak.upgrade() else {
                return;
            };
            let index = this
                .items
                .borrow()
                .iter()
                .position(|(s, p)| thumb_id(s.id, p) == id);
            if let Some(index) = index
                && let Some(mut card) = this.cards.row_data(index)
            {
                card.thumbnail = image.clone();
                this.cards.set_row_data(index, card);
            }
        });
        *self.thumbs.borrow_mut() = Some(thumbs.clone());
        self.refresh();
    }

    pub fn set_nav(&self, nav: &Rc<Nav>) {
        *self.nav.borrow_mut() = Some(Rc::downgrade(nav));
    }

    /// Runs `f` with the Home model of each open window.
    fn each_model(&self, f: impl Fn(HomeModel<'_>)) {
        if let Some(w) = self.main.upgrade() {
            f(w.global::<HomeModel>());
        }
        if let Some(w) = self.side.borrow().as_ref().and_then(|w| w.upgrade()) {
            f(w.global::<HomeModel>());
        }
    }

    /// Loads "Continue watching" and the sources again.
    pub fn refresh(self: &Rc<Self>) {
        // Continue watching: only files of sources that are still there.
        let recent = self
            .store
            .continue_watching(CONTINUE_LIMIT * 2)
            .unwrap_or_else(|err| {
                eprintln!("enthuse: cannot load continue watching: {err}");
                Vec::new()
            });
        let mut items = Vec::new();
        let mut cards = Vec::new();
        let thumbs = self.thumbs.borrow().clone();
        for (id, path, state) in recent {
            let Some(source) = self.browser.source_by_id(id) else {
                continue;
            };
            let name = path.name().unwrap_or_default().to_owned();
            let offline = self.offline.borrow().get(&id).copied().unwrap_or(false);
            let thumbnail = match &thumbs {
                // An offline share could only hang the worker.
                Some(t) if !offline => {
                    let location = self.browser.location_of(id, &path);
                    t.want(id, &source.source, &location, &path);
                    t.image(&thumb_id(id, &path))
                }
                Some(t) => t.image(&thumb_id(id, &path)),
                None => Default::default(),
            };
            cards.push(ContinueItem {
                title: display_title(&name).into(),
                subtitle: continue_subtitle(&name, &state).into(),
                progress: progress(&state) as f32,
                thumbnail,
            });
            items.push((source, path));
            if cards.len() == CONTINUE_LIMIT {
                break;
            }
        }
        *self.items.borrow_mut() = items;

        let sources = self.browser.sources();
        let tiles: Vec<_> = (0..sources.len()).map(|i| self.tile(&sources, i)).collect();
        self.tiles.set_vec(tiles);

        let favorites = self.browser.favorites();
        let favorite_tiles: Vec<_> = {
            let offline = self.offline.borrow();
            favorites
                .iter()
                .map(|(library, path)| {
                    let offline = offline.get(&library.source.id).copied().unwrap_or(false);
                    folder_tile(library, path, offline)
                })
                .collect()
        };
        *self.favorites.borrow_mut() = favorites
            .into_iter()
            .map(|(library, path)| (library.source.id, path))
            .collect();

        self.cards.set_vec(cards);
        let cards = ModelRc::from(self.cards.clone());
        let tiles = ModelRc::from(self.tiles.clone());
        let favorite_tiles = ModelRc::new(VecModel::from(favorite_tiles));
        self.each_model(|m| {
            m.set_continue_items(cards.clone());
            m.set_favorites(favorite_tiles.clone());
            m.set_sources(tiles.clone());
            m.set_loaded(true);
        });

        // Check again whether each source can be listed (an unplugged
        // drive, a share that is down). Listing can hang on a dead share,
        // so it runs off the UI thread, one check per source at a time.
        for Library { source, .. } in sources {
            if !self.checking.borrow_mut().insert(source.id) {
                continue;
            }
            let this = Rc::downgrade(self);
            let id = source.id;
            bg::spawn(
                move || source.source.list(&SourcePath::root()).is_err(),
                move |offline| {
                    if let Some(this) = this.upgrade() {
                        this.checked(id, offline);
                    }
                },
            );
        }
    }

    /// Records a reachability result and updates the source's tile.
    fn checked(&self, id: SourceId, offline: bool) {
        self.checking.borrow_mut().remove(&id);
        self.offline.borrow_mut().insert(id, offline);
        // Find the tile by id: sources may have changed since the check began.
        let sources = self.browser.sources();
        let Some(index) = sources.iter().position(|s| s.source.id == id) else {
            return;
        };
        if self
            .tiles
            .row_data(index)
            .is_some_and(|t| t.offline != offline)
        {
            self.tiles.set_row_data(index, self.tile(&sources, index));
        }
    }

    /// The tile of the library at `index` in `sources`.
    fn tile(&self, sources: &[Library], index: usize) -> SourceTileItem {
        let library = &sources[index];
        let offline = self
            .offline
            .borrow()
            .get(&library.source.id)
            .copied()
            .unwrap_or(false);
        SourceTileItem {
            favorite: library.favorite,
            ..tile(&library.source.name, &library.location, offline)
        }
    }

    fn play(&self, index: usize) {
        let Some((source, path)) = self.items.borrow().get(index).cloned() else {
            return;
        };
        if let Some(nav) = self.nav.borrow().as_ref().and_then(|n| n.upgrade()) {
            nav.play(source, path);
        }
    }

    /// Asks for a folder with the system dialog and adds it as a source.
    fn pick_folder(self: &Rc<Self>) {
        if self.picking.replace(true) {
            return; // Already open.
        }
        let mut dialog = rfd::AsyncFileDialog::new().set_title("Add source");
        // Owned by the window that asked, so it stays on top of it.
        let side = self.side.borrow().as_ref().and_then(|w| w.upgrade());
        let main = self.main.upgrade();
        let parent = match (&side, &main) {
            (Some(s), _) if s.window().is_visible() => Some(s.window().window_handle()),
            (_, Some(m)) => Some(m.window().window_handle()),
            _ => None,
        };
        if let Some(parent) = &parent {
            dialog = dialog.set_parent(parent);
        }
        let picked = dialog.pick_folder();
        let this = Rc::downgrade(self);
        let spawned = slint::spawn_local(async move {
            let folder = picked.await;
            if let Some(this) = this.upgrade() {
                this.picking.set(false);
                if let Some(folder) = folder {
                    this.browser.add_local(&folder.path().to_string_lossy());
                }
            }
        });
        if spawned.is_err() {
            self.picking.set(false);
        }
    }
}

/// The tile for a source with `name` at `location`. UNC paths
/// (`\\server\share`, or `//server/share`) are network shares.
fn tile(name: &str, location: &str, offline: bool) -> SourceTileItem {
    let network = is_network_location(location);
    SourceTileItem {
        name: name.into(),
        detail: location.into(),
        kind_label: if offline {
            "Offline"
        } else if network {
            "SMB"
        } else {
            "Local"
        }
        .into(),
        icon: if network {
            SourceIcon::Server
        } else {
            SourceIcon::Drive
        },
        offline,
        favorite: false,
        folder: false,
    }
}

/// The tile in the Favourites row: a library (its root), or a folder with
/// where it is.
fn folder_tile(library: &Library, path: &SourcePath, offline: bool) -> SourceTileItem {
    if path.is_root() {
        return SourceTileItem {
            kind_label: Default::default(),
            folder: true,
            ..tile(&library.source.name, &library.location, offline)
        };
    }
    let place = std::iter::once(library.source.name.as_str())
        .chain(path.parent().iter().flat_map(|p| p.segments()))
        .collect::<Vec<_>>()
        .join(" / ");
    SourceTileItem {
        name: path.name().unwrap_or(&library.source.name).into(),
        detail: place.into(),
        kind_label: Default::default(),
        icon: SourceIcon::Folder,
        offline,
        favorite: false,
        folder: true,
    }
}
