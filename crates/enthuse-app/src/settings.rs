//! Settings screen (#69). Fills `SettingsModel` in both windows and saves
//! each change as soon as it is made. Libraries (#59) are arranged here:
//! order, favourites, names, removal and their own resume setting.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use enthuse_core::settings::override_row;
use enthuse_core::source::{SourcePath, is_network_location};
use enthuse_core::storage::{SourceId, Store};
use enthuse_core::watched::{
    FRACTION_STEPS, REMAINING_STEPS, WatchedMode, WatchedRule, percent_label, remaining_label, step,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::browser::{Browser, Library};
use crate::{AppWindow, BrowserWindow, LibraryItem, OverrideItem, SettingsModel};

pub struct Settings {
    main: Weak<AppWindow>,
    side: RefCell<Option<Weak<BrowserWindow>>>,
    store: Rc<Store>,
    browser: Rc<Browser>,
    /// The folders listed under "Folder settings", in order.
    overrides: RefCell<Vec<(SourceId, SourcePath)>>,
    /// The rows under "Libraries". Updated in place, so a name being
    /// typed survives a refresh (they come often, e.g. during playback).
    libraries: Rc<VecModel<LibraryItem>>,
    /// Called after every saved change.
    listeners: RefCell<Vec<Box<dyn Fn()>>>,
}

/// Connects a window's Settings callbacks to `$settings`.
macro_rules! wire_callbacks {
    ($window:expr, $settings:expr) => {{
        let model = $window.global::<SettingsModel>();
        let s = $settings.clone();
        model.on_set_resume(move |on| s.save(|store| store.set_default_resume(on)));
        let s = $settings.clone();
        model.on_set_auto_next(move |on| s.save(|store| store.set_auto_next(on)));
        let s = $settings.clone();
        model.on_set_network_thumbnails(move |on| s.save(|store| store.set_network_thumbnails(on)));
        let s = $settings.clone();
        model.on_set_watched_mode(move |i| {
            s.change_rule(|r| r.mode = WatchedMode::from_index(i.max(0) as usize))
        });
        let s = $settings.clone();
        model.on_step_percent(move |d| {
            s.change_rule(|r| r.fraction = step(&FRACTION_STEPS, r.fraction, d))
        });
        let s = $settings.clone();
        model.on_step_remaining(move |d| {
            s.change_rule(|r| r.remaining = step(&REMAINING_STEPS, r.remaining, d))
        });
        let s = $settings.clone();
        model.on_clear_override(move |i| s.clear_override(i as usize));
        // These change the list, so they run once the click (and the row
        // that asked) is done.
        let s = $settings.clone();
        model.on_library_favorite(move |i| {
            let s = s.clone();
            later(move || {
                let favorite = s
                    .browser
                    .sources()
                    .get(i as usize)
                    .is_some_and(|l| l.favorite);
                s.browser.set_source_favorite(i as usize, !favorite);
            });
        });
        let s = $settings.clone();
        model.on_library_rename(move |i, name| {
            let s = s.clone();
            later(move || s.browser.rename_source(i as usize, &name));
        });
        let s = $settings.clone();
        model.on_library_move(move |from, to| {
            let s = s.clone();
            later(move || s.browser.move_source(from as usize, to as usize));
        });
        let s = $settings.clone();
        model.on_library_remove(move |i| {
            let s = s.clone();
            later(move || s.browser.remove_source(i as usize));
        });
        let s = $settings.clone();
        model.on_library_resume(move |i, choice| s.set_library_resume(i as usize, choice));
        let s = $settings.clone();
        model.on_refresh(move || s.refresh());
        model.set_version(env!("CARGO_PKG_VERSION").into());
        model.set_libraries(ModelRc::from($settings.libraries.clone()));
    }};
}

impl Settings {
    pub fn new(window: &AppWindow, store: Rc<Store>, browser: Rc<Browser>) -> Rc<Self> {
        let this = Rc::new(Self {
            main: window.as_weak(),
            side: Default::default(),
            store,
            browser,
            overrides: Default::default(),
            libraries: Rc::new(VecModel::default()),
            listeners: Default::default(),
        });
        wire_callbacks!(window, this);
        // Libraries change from Home and the browser too.
        let weak = Rc::downgrade(&this);
        this.browser.add_change_handler(Box::new(move || {
            if let Some(this) = weak.upgrade() {
                this.refresh();
            }
        }));
        this.refresh();
        this
    }

    /// Lets the browser window show Settings too.
    pub fn attach_side(self: &Rc<Self>, window: &BrowserWindow) {
        wire_callbacks!(window, self);
        *self.side.borrow_mut() = Some(window.as_weak());
        self.refresh();
    }

    /// Calls `f` after each change, e.g. so playback picks it up.
    pub fn on_change(&self, f: impl Fn() + 'static) {
        self.listeners.borrow_mut().push(Box::new(f));
    }

    /// Runs `f` with the Settings model of each open window.
    fn each_model(&self, f: impl Fn(SettingsModel<'_>)) {
        if let Some(w) = self.main.upgrade() {
            f(w.global::<SettingsModel>());
        }
        if let Some(w) = self.side.borrow().as_ref().and_then(|w| w.upgrade()) {
            f(w.global::<SettingsModel>());
        }
    }

    /// Saves one change, then shows the stored state again (so a failed
    /// save shows the old value) and lets the browser pick it up.
    fn save(&self, f: impl FnOnce(&Store) -> enthuse_core::storage::Result<()>) {
        if let Err(err) = f(&self.store) {
            self.browser
                .show_error(&format!("Cannot save this setting: {err}"));
        }
        self.refresh();
        self.browser.settings_changed();
        for f in self.listeners.borrow().iter() {
            f();
        }
    }

    fn change_rule(&self, f: impl FnOnce(&mut WatchedRule)) {
        // Never save defaults over a rule that could not be read.
        let mut rule = match self.store.watched_rule() {
            Ok(rule) => rule,
            Err(err) => {
                self.browser
                    .show_error(&format!("Cannot read this setting: {err}"));
                return;
            }
        };
        f(&mut rule);
        self.save(|store| store.set_watched_rule(&rule));
    }

    fn clear_override(&self, index: usize) {
        let Some((id, path)) = self.overrides.borrow().get(index).cloned() else {
            return;
        };
        self.save(|store| store.set_folder_resume(id, &path, None));
    }

    /// A library's own resume setting: 0 same as Playback, 1 on, 2 off.
    fn set_library_resume(&self, index: usize, choice: i32) {
        let Some(library) = self.browser.sources().get(index).cloned() else {
            return;
        };
        let resume = match choice {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        };
        self.save(|store| store.set_folder_resume(library.source.id, &SourcePath::root(), resume));
    }

    /// The rows under "Libraries", in list order.
    fn library_items(&self) -> Vec<LibraryItem> {
        let libraries = self.browser.sources();
        let favorites = libraries.iter().filter(|l| l.favorite).count();
        libraries
            .iter()
            .map(|library| {
                let Library {
                    source,
                    location,
                    favorite,
                } = library;
                let own = self
                    .store
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

    /// Loads every setting from the database into the screen.
    pub fn refresh(&self) {
        let defaults = self.store.default_playback_settings().unwrap_or_default();
        let auto_next = self.store.auto_next().unwrap_or(true);
        let network = self.store.network_thumbnails().unwrap_or(true);
        let rule = self.store.watched_rule().unwrap_or_default();

        let names: Vec<(SourceId, String)> = self
            .store
            .sources()
            .unwrap_or_default()
            .into_iter()
            .map(|s| (s.id, s.name))
            .collect();
        let mut listed = Vec::new();
        let mut items = Vec::new();
        for (id, path, o) in self.store.all_folder_overrides().unwrap_or_default() {
            let Some((_, name)) = names.iter().find(|(n, _)| *n == id) else {
                continue;
            };
            let (folder, detail) = override_row(name, &path, &o);
            items.push(OverrideItem {
                folder: folder.into(),
                detail: detail.into(),
            });
            listed.push((id, path));
        }
        *self.overrides.borrow_mut() = listed;

        let libraries = self.library_items();
        if libraries.len() == self.libraries.row_count() {
            for (i, row) in libraries.into_iter().enumerate() {
                if self.libraries.row_data(i).as_ref() != Some(&row) {
                    self.libraries.set_row_data(i, row);
                }
            }
        } else {
            self.libraries.set_vec(libraries);
        }

        let first = |steps: &[f64]| steps.first().copied().unwrap_or_default();
        let last = |steps: &[f64]| steps.last().copied().unwrap_or_default();
        self.each_model(|m| {
            m.set_resume(defaults.resume);
            m.set_auto_next(auto_next);
            m.set_network_thumbnails(network);
            m.set_watched_mode(rule.mode.index() as i32);
            m.set_watched_percent(percent_label(rule.fraction).into());
            m.set_watched_remaining(remaining_label(rule.remaining).into());
            m.set_watched_summary(rule.summary().into());
            m.set_percent_down(rule.fraction > first(&FRACTION_STEPS));
            m.set_percent_up(rule.fraction < last(&FRACTION_STEPS));
            m.set_remaining_down(rule.remaining > first(&REMAINING_STEPS));
            m.set_remaining_up(rule.remaining < last(&REMAINING_STEPS));
            m.set_overrides(ModelRc::new(VecModel::from(items.clone())));
        });
    }
}

/// Runs `f` once the current event is handled.
fn later(f: impl FnOnce() + 'static) {
    slint::Timer::single_shot(Duration::ZERO, f);
}
