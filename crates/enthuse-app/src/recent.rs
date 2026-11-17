//! Recent screen (#70): recently played files, watched or not. Fills
//! `RecentModel` in both windows, like Home.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use enthuse_core::folder::Badge;
use enthuse_core::recent::{RECENT_LIMIT, badge, detail, title};
use enthuse_core::source::SourcePath;
use enthuse_core::storage::Store;
use slint::{ComponentHandle, Model, ModelRc, VecModel, Weak};

use crate::browser::{Browser, SourceRef};
use crate::nav::Nav;
use crate::thumbs::{Thumbs, thumb_id};
use crate::{AppWindow, BrowserItem, BrowserWindow, ItemBadge, ItemKind, RecentModel};

/// Rows that ask for a thumbnail when the list loads.
const THUMB_ROWS: usize = 12;

pub struct Recent {
    main: Weak<AppWindow>,
    side: RefCell<Option<Weak<BrowserWindow>>>,
    store: Rc<Store>,
    browser: Rc<Browser>,
    nav: RefCell<Option<std::rc::Weak<Nav>>>,
    /// What each row plays.
    items: RefCell<Vec<(SourceRef, SourcePath)>>,
    rows: Rc<VecModel<BrowserItem>>,
    thumbs: RefCell<Option<Rc<Thumbs>>>,
}

macro_rules! wire_callbacks {
    ($window:expr, $recent:expr) => {{
        let model = $window.global::<RecentModel>();
        let r = $recent.clone();
        model.on_play(move |i| r.play(i as usize));
        let r = $recent.clone();
        model.on_refresh(move || r.refresh());
    }};
}

impl Recent {
    pub fn new(window: &AppWindow, store: Rc<Store>, browser: Rc<Browser>) -> Rc<Self> {
        let this = Rc::new(Self {
            main: window.as_weak(),
            side: Default::default(),
            store,
            browser,
            nav: Default::default(),
            items: Default::default(),
            rows: Rc::new(VecModel::default()),
            thumbs: Default::default(),
        });
        wire_callbacks!(window, this);
        let weak = Rc::downgrade(&this);
        this.browser.add_change_handler(Box::new(move || {
            if let Some(recent) = weak.upgrade() {
                recent.refresh();
            }
        }));
        this
    }

    /// Lets the browser window show Recent too.
    pub fn attach_side(self: &Rc<Self>, window: &BrowserWindow) {
        wire_callbacks!(window, self);
        *self.side.borrow_mut() = Some(window.as_weak());
        self.publish();
    }

    pub fn set_nav(&self, nav: &Rc<Nav>) {
        *self.nav.borrow_mut() = Some(Rc::downgrade(nav));
    }

    /// Shows thumbnails from `thumbs` on the rows.
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
                && let Some(mut row) = this.rows.row_data(index)
            {
                row.thumbnail = image.clone();
                this.rows.set_row_data(index, row);
            }
        });
        *self.thumbs.borrow_mut() = Some(thumbs.clone());
    }

    /// Loads the list again.
    pub fn refresh(&self) {
        let played = self
            .store
            .recently_played(RECENT_LIMIT)
            .unwrap_or_else(|err| {
                eprintln!("enthuse: cannot load recent files: {err}");
                Vec::new()
            });
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let thumbs = self.thumbs.borrow().clone();
        let locations = self.browser.sources();
        let mut items = Vec::new();
        let mut rows = Vec::new();
        for (id, path, state, at) in played {
            // Files of removed sources are left out.
            let Some(source) = self.browser.source_by_id(id) else {
                continue;
            };
            let name = path.name().unwrap_or_default();
            // Only the first screenful asks for new thumbnails, so a long
            // history doesn't fill the queue.
            if let Some(t) = &thumbs
                && rows.len() < THUMB_ROWS
            {
                let location = locations
                    .iter()
                    .find(|(s, _)| s.id == id)
                    .map_or("", |(_, l)| l.as_str());
                t.want(id, &source.source, location, &path);
            }
            let badge = badge(&state);
            rows.push(BrowserItem {
                label: title(name).into(),
                detail: detail(&source.source.display_name(), &path, at, now).into(),
                kind: ItemKind::Video,
                badge: match badge {
                    Badge::Watched => ItemBadge::Watched,
                    Badge::Resume { .. } => ItemBadge::Resume,
                    Badge::None | Badge::New => ItemBadge::None,
                },
                badge_text: badge.label().into(),
                progress: match badge {
                    Badge::Resume { fraction, .. } => fraction.unwrap_or(0.0) as f32,
                    _ => -1.0,
                },
                thumbnail: thumbs
                    .as_ref()
                    .map(|t| t.image(&thumb_id(id, &path)))
                    .unwrap_or_default(),
            });
            items.push((source, path));
        }
        *self.items.borrow_mut() = items;
        // In place when the length is the same (a position save while the
        // list shows), so the rows, and keyboard focus, survive.
        if rows.len() == self.rows.row_count() {
            for (i, row) in rows.into_iter().enumerate() {
                if self.rows.row_data(i).as_ref() != Some(&row) {
                    self.rows.set_row_data(i, row);
                }
            }
        } else {
            self.rows.set_vec(rows);
        }
        self.publish();
    }

    fn publish(&self) {
        let rows = ModelRc::from(self.rows.clone());
        if let Some(w) = self.main.upgrade() {
            let m = w.global::<RecentModel>();
            m.set_items(rows.clone());
            m.set_loaded(true);
        }
        if let Some(w) = self.side.borrow().as_ref().and_then(|w| w.upgrade()) {
            let m = w.global::<RecentModel>();
            m.set_items(rows);
            m.set_loaded(true);
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
}
