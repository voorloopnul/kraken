//! The browser pane's tabs, as QML sees it.
//!
//! There is no renderer here. Everything below the page — which tabs exist,
//! which is in front, what each is called and where it points, what a typed
//! address means — is this object's and [`pupo_core::browser`]'s; the page
//! itself is drawn by a `WebEngineView` the panel creates at run time *if* the
//! QtWebEngine QML module is installed, and by an explicit notice when it is
//! not. So the pane is whole either way, and nothing here has to know which.
//!
//! Tabs are per workspace, for the same reason the terminal's are: the panels
//! are built once and reused as the current workspace changes, so one flat list
//! would show whichever project's pages happened to be open when you last
//! looked.

use std::collections::HashMap;

use pupo_core::{browser, debug};
use qmetaobject::*;

/// Where a typed search goes. DuckDuckGo because it needs no account and sets
/// no cookie to answer one query.
const SEARCH: &str = "https://duckduckgo.com/?q=";

/// One tab: where it points and what it is called.
#[derive(Debug, Clone)]
struct Tab {
    id: u32,
    url: String,
    title: String,
    /// Set by the view when a page dies under it, so the panel can offer the
    /// reload rather than leaving a white void with a title over it.
    crashed: bool,
}

/// One workspace's tabs.
#[derive(Default)]
struct Pane {
    tabs: Vec<Tab>,
    current: u32,
}

#[derive(QObject, Default)]
pub struct BrowserBridge {
    base: qt_base_class!(trait QObject),

    /// The strip's tabs, left to right: `{ id, title, bell, closed }` — the same
    /// shape the terminal's strip takes, because they share `TabStrip.qml`.
    tabs: qt_property!(QVariantList; NOTIFY tabs_changed READ get_tabs),
    /// The current tab's id, or -1 when the pane has none.
    current: qt_property!(i32; NOTIFY tabs_changed READ get_current),
    /// The current tab's address, for the URL bar.
    url: qt_property!(QString; NOTIFY tabs_changed READ get_url),
    /// Whether the current tab is showing the crash notice.
    crashed: qt_property!(bool; NOTIFY tabs_changed READ get_crashed),
    /// Whether this workspace has any tab at all. A pane that has never been
    /// opened has none, and opening it is what makes the first.
    started: qt_property!(bool; NOTIFY tabs_changed READ get_started),

    tabs_changed: qt_signal!(),
    /// The last tab was closed. The workspace hides the panel in response, and
    /// the side strip's toggle goes out with it — an empty browser pane is a
    /// stripe of nothing, not a browser.
    emptied: qt_signal!(),
    /// Something asked for a page: a link in a transcript, or the URL bar. The
    /// panel loads it into the current tab.
    load_requested: qt_signal!(id: i32, url: QString),

    set_workspace: qt_method!(fn(&mut self, key: QString)),
    /// Make the first tab if this workspace has none. Called the first time the
    /// panel is shown, which is what keeps a pane nobody opens free.
    ensure_started: qt_method!(fn(&mut self)),

    add_tab: qt_method!(fn(&mut self)),
    close_tab: qt_method!(fn(&mut self, id: i32)),
    select_tab: qt_method!(fn(&mut self, id: i32)),
    move_tab: qt_method!(fn(&mut self, from: i32, to: i32)),

    /// Load an address into the current tab, making one if there is none.
    /// Takes what was typed — a URL, a bare host or a sentence — and works out
    /// which it is.
    open_url: qt_method!(fn(&mut self, typed: QString)),
    /// What the view reports back as a page loads.
    page_changed: qt_method!(fn(&mut self, id: i32, url: QString, title: QString)),
    /// The renderer died under a tab.
    page_crashed: qt_method!(fn(&mut self, id: i32)),
    /// Throw away every tab of every workspace. The window's teardown calls it.
    discard_all: qt_method!(fn(&mut self)),
    /// A fresh file under `~/.pupo/screenshots/` for the panel to save a
    /// capture into. The path is made here because the folder is ours; what
    /// goes in it is the view's, which only QML can ask for.
    capture_path: qt_method!(fn(&self) -> QString),

    panes: HashMap<String, Pane>,
    workspace: String,
    /// Ids are unique across workspaces, so a stale id from a strip that has not
    /// rebuilt yet can never name another workspace's tab.
    next_id: u32,
}

impl BrowserBridge {
    pub fn new() -> Self {
        Self::default()
    }

    fn pane(&self) -> Option<&Pane> {
        self.panes.get(&self.workspace)
    }

    fn pane_mut(&mut self) -> &mut Pane {
        self.panes.entry(self.workspace.clone()).or_default()
    }

    fn tab(&self) -> Option<&Tab> {
        let pane = self.pane()?;
        pane.tabs.iter().find(|tab| tab.id == pane.current)
    }

    fn set_workspace(&mut self, key: QString) {
        let key = key.to_string();
        if key == self.workspace {
            return;
        }
        self.workspace = key;
        self.tabs_changed();
    }

    fn get_started(&self) -> bool {
        self.pane().is_some_and(|pane| !pane.tabs.is_empty())
    }

    fn ensure_started(&mut self) {
        if self.workspace.is_empty() || self.get_started() {
            return;
        }
        self.add_tab();
    }

    fn add_tab(&mut self) {
        if self.workspace.is_empty() {
            return;
        }
        self.next_id += 1;
        let id = self.next_id;
        let pane = self.pane_mut();
        pane.tabs.push(Tab {
            id,
            // A new tab costs no renderer until it is pointed somewhere, which
            // is the whole reason it starts blank rather than on a home page.
            url: browser::BLANK.to_string(),
            title: String::new(),
            crashed: false,
        });
        pane.current = id;
        debug::action("browser.tab-new", &[]);
        self.tabs_changed();
    }

    fn close_tab(&mut self, id: i32) {
        let Ok(id) = u32::try_from(id) else {
            return;
        };
        let pane = self.pane_mut();
        let Some(at) = pane.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        pane.tabs.remove(at);
        if pane.current == id {
            // The tab to its left, or the new first one: closing the last tab
            // of several should leave you looking at one of the others.
            let next = at.saturating_sub(1);
            pane.current = pane.tabs.get(next).map(|tab| tab.id).unwrap_or(0);
        }
        let empty = pane.tabs.is_empty();
        self.tabs_changed();
        if empty {
            self.emptied();
        }
    }

    fn select_tab(&mut self, id: i32) {
        let Ok(id) = u32::try_from(id) else {
            return;
        };
        let pane = self.pane_mut();
        if pane.tabs.iter().any(|tab| tab.id == id) && pane.current != id {
            pane.current = id;
            self.tabs_changed();
        }
    }

    fn move_tab(&mut self, from: i32, to: i32) {
        let pane = self.pane_mut();
        let count = pane.tabs.len();
        let (Ok(from), Ok(to)) = (usize::try_from(from), usize::try_from(to)) else {
            return;
        };
        if from >= count || to >= count || from == to {
            return;
        }
        let tab = pane.tabs.remove(from);
        pane.tabs.insert(to, tab);
        self.tabs_changed();
    }

    fn open_url(&mut self, typed: QString) {
        let target = browser::navigate_to(&typed.to_string(), SEARCH);
        self.ensure_started();
        let Some(pane) = self.panes.get_mut(&self.workspace) else {
            return;
        };
        let current = pane.current;
        let Some(tab) = pane.tabs.iter_mut().find(|tab| tab.id == current) else {
            return;
        };
        tab.url = target.clone();
        tab.crashed = false;
        debug::action("browser.open", &[("url", target.clone())]);
        self.tabs_changed();
        self.load_requested(current as i32, target.as_str().into());
    }

    fn page_changed(&mut self, id: i32, url: QString, title: QString) {
        let Ok(id) = u32::try_from(id) else {
            return;
        };
        let Some(pane) = self.panes.get_mut(&self.workspace) else {
            return;
        };
        let Some(tab) = pane.tabs.iter_mut().find(|tab| tab.id == id) else {
            return;
        };
        let url = url.to_string();
        let title = title.to_string();
        // A page that is loading reports its title and URL many times over, and
        // most of those say what the last one did. Signalling anyway rebuilds
        // the whole tab list and re-evaluates every binding reading from it —
        // and, because the panel answers the signal by pointing the page at the
        // URL it just reported, it is also one half of a loop.
        if tab.url == url && tab.title == title && !tab.crashed {
            return;
        }
        tab.url = url;
        tab.title = title;
        tab.crashed = false;
        self.tabs_changed();
    }

    fn page_crashed(&mut self, id: i32) {
        let Ok(id) = u32::try_from(id) else {
            return;
        };
        let Some(pane) = self.panes.get_mut(&self.workspace) else {
            return;
        };
        if let Some(tab) = pane.tabs.iter_mut().find(|tab| tab.id == id) {
            tab.crashed = true;
            debug::error("browser.crashed", &[("url", tab.url.clone())]);
            self.tabs_changed();
        }
    }

    fn discard_all(&mut self) {
        self.panes.clear();
        self.tabs_changed();
    }

    fn capture_path(&self) -> QString {
        let folder = pupo_core::state::config_dir().join("screenshots");
        if std::fs::create_dir_all(&folder).is_err() {
            return "".into();
        }
        // Seconds since the epoch: unique enough for a folder one person drops
        // captures into, and sortable, which a random name would not be.
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis())
            .unwrap_or_default();
        folder
            .join(format!("page-{stamp}.png"))
            .to_string_lossy()
            .as_ref()
            .into()
    }

    fn get_tabs(&self) -> QVariantList {
        let mut list = QVariantList::default();
        let Some(pane) = self.pane() else {
            return list;
        };
        for tab in &pane.tabs {
            let mut map = QVariantMap::default();
            map.insert("id".into(), (tab.id as i32).into());
            // A tab that has not been pointed anywhere is "New Tab" rather than
            // "about:blank", which is an answer to a question nobody asked.
            let title = if !tab.title.is_empty() {
                tab.title.as_str()
            } else if browser::is_blank(&tab.url) {
                "New Tab"
            } else {
                tab.url.as_str()
            };
            map.insert("title".into(), QString::from(title).into());
            // The strip is shared with the terminal, whose tabs ring and exit;
            // a page does neither, so both marks are always off.
            map.insert("bell".into(), false.into());
            map.insert("closed".into(), false.into());
            list.push(map.into());
        }
        list
    }

    fn get_current(&self) -> i32 {
        match self.pane() {
            Some(pane) if pane.tabs.iter().any(|tab| tab.id == pane.current) => {
                pane.current as i32
            }
            _ => -1,
        }
    }

    fn get_url(&self) -> QString {
        match self.tab() {
            // The bar is empty on a blank tab: `about:blank` is not an address
            // anyone typed, and showing it means selecting it before typing.
            Some(tab) if !browser::is_blank(&tab.url) => tab.url.as_str().into(),
            _ => "".into(),
        }
    }

    fn get_crashed(&self) -> bool {
        self.tab().is_some_and(|tab| tab.crashed)
    }
}
