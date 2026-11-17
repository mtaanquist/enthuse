//! Home screen: "Continue watching" and the sources (docs/ui.md, "Home").
//! Fills `HomeModel` in both windows, so Home looks the same in the
//! browser window during PiP.

use std::cell::RefCell;
use std::rc::Rc;

use enthuse_core::folder::{continue_subtitle, display_title, progress};
use enthuse_core::source::SourcePath;
use enthuse_core::storage::Store;
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::browser::{Browser, SourceRef};
use crate::nav::Nav;
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
    /// Shown source tiles, for marking offline ones when the check returns.
    tiles: Rc<VecModel<SourceTileItem>>,
    /// Bumped on every refresh so late reachability results are ignored.
    generation: RefCell<u64>,
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
        model.on_remove_source(move |i| h.browser.remove_source(i as usize));
        let h = $home.clone();
        model.on_add_source(move || h.pick_folder());
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
            tiles: Rc::new(VecModel::default()),
            generation: Default::default(),
        });
        wire_callbacks!(window, this);
        let weak = Rc::downgrade(&this);
        this.browser.set_change_handler(Box::new(move || {
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
        let generation = {
            let mut g = self.generation.borrow_mut();
            *g += 1;
            *g
        };

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
        for (id, path, state) in recent {
            let Some(source) = self.browser.source_by_id(id) else {
                continue;
            };
            let name = path.name().unwrap_or_default().to_owned();
            cards.push(ContinueItem {
                title: display_title(&name).into(),
                subtitle: continue_subtitle(&name, &state).into(),
                progress: progress(&state) as f32,
            });
            items.push((source, path));
            if cards.len() == CONTINUE_LIMIT {
                break;
            }
        }
        *self.items.borrow_mut() = items;

        let sources = self.browser.sources();
        let tiles: Vec<_> = sources
            .iter()
            .map(|(s, location)| tile(&s.source.display_name(), location))
            .collect();
        self.tiles.set_vec(tiles);

        let cards = ModelRc::new(VecModel::from(cards));
        let tiles = ModelRc::from(self.tiles.clone());
        self.each_model(|m| {
            m.set_continue_items(cards.clone());
            m.set_sources(tiles.clone());
            m.set_loaded(true);
        });

        // Mark sources that cannot be listed (an unplugged drive, a share
        // that is down) as offline. Listing can hang on a dead share, so it
        // runs off the UI thread.
        for (index, (source, _)) in sources.into_iter().enumerate() {
            let this = Rc::downgrade(self);
            bg::spawn(
                move || source.source.list(&SourcePath::root()).is_err(),
                move |offline| {
                    let Some(this) = this.upgrade() else {
                        return;
                    };
                    if !offline || *this.generation.borrow() != generation {
                        return;
                    }
                    if let Some(mut tile) = this.tiles.row_data(index) {
                        tile.offline = true;
                        tile.kind_label = "Offline".into();
                        this.tiles.set_row_data(index, tile);
                    }
                },
            );
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
        let this = Rc::downgrade(self);
        // The dialog blocks until closed, so it runs off the UI thread.
        bg::spawn(
            || rfd::FileDialog::new().set_title("Add source").pick_folder(),
            move |folder| {
                if let (Some(this), Some(folder)) = (this.upgrade(), folder) {
                    this.browser.add_local(&folder.to_string_lossy());
                }
            },
        );
    }
}

/// The tile for a source with `name` at `location`. UNC paths
/// (`\\server\share`, or `//server/share`) are network shares.
fn tile(name: &str, location: &str) -> SourceTileItem {
    let network = is_network_path(location);
    SourceTileItem {
        name: name.into(),
        detail: location.into(),
        kind_label: if network { "SMB" } else { "Local" }.into(),
        icon: if network {
            SourceIcon::Server
        } else {
            SourceIcon::Drive
        },
        offline: false,
    }
}

fn is_network_path(location: &str) -> bool {
    location.starts_with(r"\\") || location.starts_with("//") || location.starts_with("smb://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unc_paths_are_network_shares() {
        assert!(is_network_path(r"\\nas\media"));
        assert!(is_network_path("//nas/media"));
        assert!(!is_network_path(r"D:\Movies"));
        assert!(!is_network_path("/mnt/media"));
    }
}
