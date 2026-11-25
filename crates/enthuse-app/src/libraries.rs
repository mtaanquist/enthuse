//! The Libraries page (#59): every library, arranged there, with each
//! one's settings in a dialog. Fills `LibrariesModel` in both windows.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use enthuse_core::source::{SourcePath, is_network_location};
use enthuse_core::storage::{SourceId, Store};
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::browser::{Browser, Library};
use crate::{AppWindow, BrowserWindow, LibrariesModel, LibraryItem};

pub struct Libraries {
    main: Weak<AppWindow>,
    side: RefCell<Option<Weak<BrowserWindow>>>,
    store: Rc<Store>,
    browser: Rc<Browser>,
    /// The rows, updated in place so a list being looked at doesn't jump
    /// on each refresh (they come often, e.g. during playback).
    rows: Rc<VecModel<LibraryItem>>,
    /// The library whose dialog is open. By id: rows move.
    editing: Cell<Option<SourceId>>,
}

/// Connects a window's Libraries callbacks to `$this`.
macro_rules! wire_callbacks {
    ($window:expr, $this:expr) => {{
        let model = $window.global::<LibrariesModel>();
        model.set_libraries(ModelRc::from($this.rows.clone()));
        let l = $this.clone();
        model.on_open(move |i| l.browser.open_source(i as usize));
        // These change the list, so they run once the click (and the row
        // that asked) is done.
        let l = $this.clone();
        model.on_favorite(move |i| {
            let l = l.clone();
            later(move || {
                let favorite = l
                    .browser
                    .sources()
                    .get(i as usize)
                    .is_some_and(|s| s.favorite);
                l.browser.set_source_favorite(i as usize, !favorite);
            });
        });
        let l = $this.clone();
        model.on_move(move |from, to| {
            let l = l.clone();
            later(move || l.browser.move_source(from as usize, to as usize));
        });
        let l = $this.clone();
        model.on_edit(move |i| l.edit(i as usize));
        let l = $this.clone();
        model.on_close_edit(move || l.close_edit());
        let l = $this.clone();
        model.on_rename(move |name| {
            let l = l.clone();
            later(move || {
                if let Some(index) = l.edited_index() {
                    l.browser.rename_source(index, &name);
                }
            });
        });
        let l = $this.clone();
        model.on_show_on_home(move |i, show| {
            let l = l.clone();
            later(move || l.browser.set_source_hidden(i as usize, !show));
        });
        let l = $this.clone();
        model.on_resume(move |choice| l.set_resume(choice));
        let l = $this.clone();
        model.on_remove(move || {
            let l = l.clone();
            later(move || {
                if let Some(index) = l.edited_index() {
                    l.close_edit();
                    l.browser.remove_source(index);
                }
            });
        });
    }};
}

impl Libraries {
    pub fn new(window: &AppWindow, store: Rc<Store>, browser: Rc<Browser>) -> Rc<Self> {
        let this = Rc::new(Self {
            main: window.as_weak(),
            side: Default::default(),
            store,
            browser,
            rows: Rc::new(VecModel::default()),
            editing: Cell::new(None),
        });
        wire_callbacks!(window, this);
        let weak = Rc::downgrade(&this);
        this.browser.add_change_handler(Box::new(move || {
            if let Some(this) = weak.upgrade() {
                this.refresh();
            }
        }));
        this.refresh();
        this
    }

    /// Lets the browser window show the Libraries page too.
    pub fn attach_side(self: &Rc<Self>, window: &BrowserWindow) {
        wire_callbacks!(window, self);
        *self.side.borrow_mut() = Some(window.as_weak());
        self.refresh();
    }

    /// Runs `f` with the Libraries model of each open window.
    fn each_model(&self, f: impl Fn(LibrariesModel<'_>)) {
        if let Some(w) = self.main.upgrade() {
            f(w.global::<LibrariesModel>());
        }
        if let Some(w) = self.side.borrow().as_ref().and_then(|w| w.upgrade()) {
            f(w.global::<LibrariesModel>());
        }
    }

    /// Opens the dialog of the library at `index`.
    fn edit(&self, index: usize) {
        let Some(id) = self.browser.sources().get(index).map(|l| l.source.id) else {
            return;
        };
        self.editing.set(Some(id));
        self.refresh();
    }

    fn close_edit(&self) {
        self.editing.set(None);
        self.each_model(|m| m.set_editing(false));
    }

    /// Where the library in the dialog is in the list now.
    fn edited_index(&self) -> Option<usize> {
        let id = self.editing.get()?;
        self.browser
            .sources()
            .iter()
            .position(|l| l.source.id == id)
    }

    /// The edited library's own resume setting: 0 same as Playback, 1 on,
    /// 2 off.
    fn set_resume(&self, choice: i32) {
        let Some(id) = self.editing.get() else {
            return;
        };
        let resume = match choice {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        };
        if let Err(err) = self
            .store
            .set_folder_resume(id, &SourcePath::root(), resume)
        {
            self.browser
                .show_error(&format!("Cannot save this setting: {err}"));
        }
        self.refresh();
        // The browser shows the resume setting of the folder it is in.
        self.browser.settings_changed();
    }

    /// Loads the libraries into the page, and the dialog's library.
    pub fn refresh(&self) {
        let libraries = self.browser.sources();
        let items = items(&self.store, &libraries);
        if items.len() == self.rows.row_count() {
            for (i, row) in items.iter().enumerate() {
                if self.rows.row_data(i).as_ref() != Some(row) {
                    self.rows.set_row_data(i, row.clone());
                }
            }
        } else {
            self.rows.set_vec(items.clone());
        }

        // The dialog's library, unless it was removed meanwhile.
        let edited = self.editing.get().and_then(|id| {
            let index = libraries.iter().position(|l| l.source.id == id)?;
            items.get(index).cloned()
        });
        if edited.is_none() {
            self.editing.set(None);
        }
        let resume_default = self
            .store
            .default_playback_settings()
            .map_or(true, |s| s.resume);
        self.each_model(|m| {
            m.set_resume_default(resume_default);
            match &edited {
                Some(item) => {
                    m.set_edited(item.clone());
                    m.set_editing(true);
                }
                None => m.set_editing(false),
            }
        });
    }
}

/// The rows of the Libraries page, in list order.
fn items(store: &Store, libraries: &[Library]) -> Vec<LibraryItem> {
    let favorites = libraries.iter().filter(|l| l.favorite).count();
    libraries
        .iter()
        .map(|library| {
            let Library {
                source,
                location,
                favorite,
                hidden,
            } = library;
            let own = store
                .folder_overrides(source.id)
                .ok()
                .and_then(|o| o.get(&SourcePath::root()).and_then(|o| o.resume));
            let (first, last) = if *favorite {
                (0, favorites.saturating_sub(1))
            } else {
                (favorites, libraries.len().saturating_sub(1))
            };
            LibraryItem {
                id: source.id.0 as i32,
                name: source.name.as_str().into(),
                location: location.as_str().into(),
                network: is_network_location(location),
                favorite: *favorite,
                hidden: *hidden,
                resume: match own {
                    None => 0,
                    Some(true) => 1,
                    Some(false) => 2,
                },
                group_first: first as i32,
                group_last: last as i32,
            }
        })
        .collect()
}

/// Runs `f` once the current event is handled.
fn later(f: impl FnOnce() + 'static) {
    slint::Timer::single_shot(Duration::ZERO, f);
}
