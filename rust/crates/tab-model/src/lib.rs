//! Pure tab and tab-order state.
//!
//! Port of the reducers and selectors of `packages/app/src/tabs` and
//! `packages/app/src/ordered-tabs` (`duck.ts`, `selectors.ts`, `get.ts`).
//! Sagas, Apollo, Electron/webview calls and React are out of scope; this
//! crate only keeps the state those layers read and write.

use std::collections::{HashMap, HashSet, VecDeque};

/// A tab badge: `UPDATE_BADGE` normalizes its input to a number when it
/// starts with a base-10 integer, otherwise it stores the raw string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Badge {
    Number(i64),
    Text(String),
}

/// One browser tab (`StationTab` in `packages/app/src/tabs/types.ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub application_id: String,
    pub tab_id: String,
    pub url: String,
    pub title: Option<String>,
    pub badge: Option<Badge>,
    pub favicons: Vec<String>,
    pub is_application_home: bool,
    pub is_loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
    pub last_put_to_sleep_at: Option<i64>,
    pub last_activity_at: Option<i64>,
    pub ignored_for_back_history: bool,
}

/// Every action the two TS ducks' reducers understand, plus [`TabAction::Close`]
/// which the TS state layer reaches through a saga (see its docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabAction {
    /// `ADD`. Like the `ordered-tabs` saga, an application home tab is not
    /// pushed to its application's order.
    Add {
        application_id: String,
        tab_id: String,
        url: String,
        is_application_home: bool,
    },
    /// `CLOSE`. The TS reducer ignores it and the close saga dispatches
    /// `REMOVE` for the tab; this variant is that net state effect. The saga's
    /// home-tab and navigation policy is UI/session behavior and is not
    /// modeled.
    Close {
        tab_id: String,
    },
    /// `REMOVE`, with the `ordered-tabs` `REMOVE_TAB` effect the
    /// `REMOVE` saga dispatches alongside it: drops the tab and its id from
    /// every application order.
    Remove {
        application_id: String,
        tab_id: String,
    },
    /// `CLOSE_ALL_IN_APP`: the state effect of the saga that dispatches one
    /// `REMOVE` per tab of the application.
    CloseAllInApp {
        application_id: String,
    },
    UpdateTitle {
        tab_id: String,
        title: String,
    },
    UpdateUrl {
        tab_id: String,
        url: String,
    },
    /// The raw badge string; normalized on apply.
    UpdateBadge {
        tab_id: String,
        badge: String,
    },
    UpdateFavicons {
        tab_id: String,
        favicons: Vec<String>,
    },
    UpdateLoading {
        tab_id: String,
        is_loading: bool,
    },
    UpdateBackForward {
        tab_id: String,
        can_go_back: bool,
        can_go_forward: bool,
    },
    /// Also clears `ignored_for_back_history`, like `UPDATE_LAST_ACTIVITY`.
    UpdateLastActivity {
        tab_id: String,
        at_ms: i64,
    },
    UpdateLastPutToSleepAt {
        tab_id: String,
        at_ms: i64,
    },
    IgnoreForBackHistory {
        tab_id: String,
    },
    PushOrderedTab {
        application_id: String,
        tab_id: String,
    },
    /// `REMOVE_TAB` from the `ordered-tabs` duck. Its `applicationId` is
    /// accepted for parity with the TS action but, as there, ignored.
    RemoveOrderedTab {
        application_id: String,
        tab_id: String,
    },
    ReorderTab {
        tab_id: String,
        new_position: i64,
    },
}

impl TabAction {
    /// `addTab(applicationId, tabId, url)` — `isApplicationHome` defaults to
    /// `false`.
    pub fn add(
        application_id: impl Into<String>,
        tab_id: impl Into<String>,
        url: impl Into<String>,
    ) -> Self {
        TabAction::Add {
            application_id: application_id.into(),
            tab_id: tab_id.into(),
            url: url.into(),
            is_application_home: false,
        }
    }
}

/// Ordered tab ids for one application (`StationOrderedTabs`).
///
/// The order is a list of `(seq, id)` entries, newest first, so pushing to
/// the front is O(1). Removals only mark seqs dead in [`TabModel::dead`];
/// reads skip them and the list compacts once tombstones dominate. This
/// keeps 100k-scale add/remove sequences linear instead of quadratic while
/// preserving the reducers' order semantics exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrderedTabs {
    application_id: String,
    order: VecDeque<(u64, String)>,
    /// Every id currently listed, with the seqs of its entries (ids may be
    /// listed twice, like a TS `PUSH_TAB` duplicate).
    listed: HashMap<String, Vec<u64>>,
    /// Number of live (non-dead) entries.
    live: usize,
}

/// The composed `tabs` + `orderedTabs` state.
#[derive(Debug, Default)]
pub struct TabModel {
    tabs: HashMap<String, Tab>,
    ordered: Vec<OrderedTabs>,
    next_seq: u64,
    dead: HashSet<u64>,
}

impl TabModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one action, ignoring unknown tab ids the way the TS reducers do.
    pub fn apply(&mut self, action: TabAction) {
        match action {
            TabAction::Add {
                application_id,
                tab_id,
                url,
                is_application_home,
            } => {
                // `ordered-tabs/sagas.ts`: `if (isApplicationHome || !tabId) return;`
                if !is_application_home && !tab_id.is_empty() {
                    self.push_ordered(&application_id, &tab_id);
                }
                self.tabs.insert(
                    tab_id.clone(),
                    Tab {
                        application_id,
                        tab_id,
                        url,
                        title: None,
                        badge: None,
                        favicons: Vec::new(),
                        is_application_home,
                        is_loading: false,
                        can_go_back: false,
                        can_go_forward: false,
                        last_put_to_sleep_at: None,
                        last_activity_at: None,
                        ignored_for_back_history: false,
                    },
                );
            }
            TabAction::PushOrderedTab {
                application_id,
                tab_id,
            } => {
                // `PUSH_TAB` prepends even when the id is already listed.
                self.push_ordered(&application_id, &tab_id);
            }
            TabAction::Close { tab_id } | TabAction::Remove { tab_id, .. } => {
                self.remove_tab(&tab_id);
            }
            TabAction::CloseAllInApp { application_id } => {
                self.tabs
                    .retain(|_, tab| tab.application_id != application_id);
                self.ordered
                    .retain(|entry| entry.application_id != application_id);
                // Dropped entries leave stale tombstones behind; reclaim them.
                self.compact_if_needed();
            }
            TabAction::RemoveOrderedTab { tab_id, .. } => {
                self.remove_ordered_everywhere(&tab_id);
            }
            TabAction::ReorderTab {
                tab_id,
                new_position,
            } => {
                // `REORDER_TAB`: find the first application listing the tab,
                // delete it, then insert it at a list-splice-resolved index.
                let Self { ordered, dead, .. } = self;
                let Some(entry) = ordered
                    .iter_mut()
                    .find(|entry| entry.listed.contains_key(&tab_id))
                else {
                    return;
                };
                // First live occurrence, counting only live entries so dead
                // tombstones never shift positions.
                let Some((position, seq)) = entry
                    .order
                    .iter()
                    .enumerate()
                    .filter(|(_, (seq, _))| !dead.contains(seq))
                    .find_map(|(index, (seq, id))| (id == &tab_id).then_some((index, *seq)))
                else {
                    return;
                };
                let (_, tab_id) = entry.order.remove(position).expect("position is live");
                entry.live -= 1;
                let len = entry.live as i64;
                let target = if new_position < 0 {
                    new_position.saturating_add(len).max(0)
                } else {
                    new_position.min(len)
                };
                if target == len {
                    entry.order.push_back((seq, tab_id.clone()));
                } else {
                    // Deque index that makes the tab the `target`-th live entry.
                    let mut live_seen = 0i64;
                    let mut index = entry.order.len();
                    for (i, (s, _)) in entry.order.iter().enumerate() {
                        if dead.contains(s) {
                            continue;
                        }
                        if live_seen == target {
                            index = i;
                            break;
                        }
                        live_seen += 1;
                    }
                    entry.order.insert(index, (seq, tab_id.clone()));
                }
                entry.live += 1;
            }
            TabAction::UpdateTitle { tab_id, title } => {
                if let Some(tab) = self.tabs.get_mut(&tab_id) {
                    tab.title = Some(title);
                }
            }
            TabAction::UpdateUrl { tab_id, url } => {
                if let Some(tab) = self.tabs.get_mut(&tab_id) {
                    tab.url = url;
                }
            }
            TabAction::UpdateBadge { tab_id, badge } => {
                if let Some(tab) = self.tabs.get_mut(&tab_id) {
                    tab.badge = normalize_badge(&badge);
                }
            }
            TabAction::UpdateFavicons { tab_id, favicons } => {
                if let Some(tab) = self.tabs.get_mut(&tab_id) {
                    tab.favicons = favicons;
                }
            }
            TabAction::UpdateLoading { tab_id, is_loading } => {
                if let Some(tab) = self.tabs.get_mut(&tab_id) {
                    tab.is_loading = is_loading;
                }
            }
            TabAction::UpdateBackForward {
                tab_id,
                can_go_back,
                can_go_forward,
            } => {
                if let Some(tab) = self.tabs.get_mut(&tab_id) {
                    tab.can_go_back = can_go_back;
                    tab.can_go_forward = can_go_forward;
                }
            }
            TabAction::UpdateLastPutToSleepAt { tab_id, at_ms } => {
                if let Some(tab) = self.tabs.get_mut(&tab_id) {
                    tab.last_put_to_sleep_at = Some(at_ms);
                }
            }
            TabAction::UpdateLastActivity { tab_id, at_ms } => {
                if let Some(tab) = self.tabs.get_mut(&tab_id) {
                    tab.last_activity_at = Some(at_ms);
                    tab.ignored_for_back_history = false;
                }
            }
            TabAction::IgnoreForBackHistory { tab_id } => {
                if let Some(tab) = self.tabs.get_mut(&tab_id) {
                    tab.ignored_for_back_history = true;
                }
            }
        }
    }

    pub fn tab(&self, tab_id: &str) -> Option<&Tab> {
        self.tabs.get(tab_id)
    }

    /// Tabs of an application, newest first, in `ordered-tabs` order; ids that
    /// no longer resolve to a tab of that application are skipped.
    pub fn tabs_for_application(&self, application_id: &str) -> Vec<&Tab> {
        let Some(entry) = self
            .ordered
            .iter()
            .find(|entry| entry.application_id == application_id)
        else {
            return Vec::new();
        };
        entry
            .order
            .iter()
            .filter(|(seq, _)| !self.dead.contains(seq))
            .filter_map(|(_, tab_id)| self.tabs.get(tab_id))
            .filter(|tab| tab.application_id == application_id)
            .collect()
    }

    /// Ordered tab ids of an application, newest first, `[]` when it has none.
    pub fn ordered_tab_ids(&self, application_id: &str) -> Vec<String> {
        self.ordered
            .iter()
            .find(|entry| entry.application_id == application_id)
            .map(|entry| {
                entry
                    .order
                    .iter()
                    .filter(|(seq, _)| !self.dead.contains(seq))
                    .map(|(_, id)| id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Applications that have ordered tabs, in first-seen order.
    pub fn applications_with_tabs(&self) -> Vec<&str> {
        self.ordered
            .iter()
            .map(|entry| entry.application_id.as_str())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    fn push_ordered(&mut self, application_id: &str, tab_id: &str) {
        self.next_seq += 1;
        let seq = self.next_seq;
        match self
            .ordered
            .iter_mut()
            .find(|entry| entry.application_id == application_id)
        {
            Some(entry) => {
                entry.order.push_front((seq, tab_id.to_string()));
                entry
                    .listed
                    .entry(tab_id.to_string())
                    .or_default()
                    .push(seq);
                entry.live += 1;
            }
            None => self.ordered.push(OrderedTabs {
                application_id: application_id.to_string(),
                order: VecDeque::from([(seq, tab_id.to_string())]),
                listed: HashMap::from([(tab_id.to_string(), vec![seq])]),
                live: 1,
            }),
        }
    }

    fn remove_tab(&mut self, tab_id: &str) {
        self.tabs.remove(tab_id);
        self.remove_ordered_everywhere(tab_id);
    }

    /// `REMOVE_TAB`: drop the id from every application's order, then drop
    /// applications whose order became empty.
    fn remove_ordered_everywhere(&mut self, tab_id: &str) {
        let mut removed: Vec<u64> = Vec::new();
        for entry in &mut self.ordered {
            if let Some(seqs) = entry.listed.remove(tab_id) {
                entry.live -= seqs.len();
                removed.extend(seqs);
            }
        }
        self.dead.extend(removed);
        self.ordered.retain(|entry| entry.live > 0);
        self.compact_if_needed();
    }

    /// Rebuild the orders once tombstones dominate, so hostile add/remove
    /// cycles stay linear overall.
    fn compact_if_needed(&mut self) {
        let live_total: usize = self.ordered.iter().map(|entry| entry.live).sum();
        if self.dead.len() <= live_total {
            return;
        }
        let Self { ordered, dead, .. } = self;
        for entry in ordered.iter_mut() {
            entry.order.retain(|(seq, _)| !dead.contains(seq));
            entry.listed.clear();
            for (seq, id) in &entry.order {
                entry.listed.entry(id.clone()).or_default().push(*seq);
            }
            entry.live = entry.order.len();
        }
        dead.clear();
    }
}

/// `normalizedBadgeValue` plus `is-empty`: an empty badge clears it (the TS
/// `null`), an integer prefix becomes a number, anything else stays text.
fn normalize_badge(badge: &str) -> Option<Badge> {
    if badge.is_empty() {
        return None;
    }
    match parse_int_prefix(badge) {
        Some(number) => Some(Badge::Number(number)),
        None => Some(Badge::Text(badge.to_string())),
    }
}

/// `parseInt(value, 10)`: optional sign, then the longest digit prefix.
fn parse_int_prefix(value: &str) -> Option<i64> {
    let unsigned = value.trim_start_matches(char::is_whitespace);
    let (negative, digits) = match unsigned.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, unsigned.strip_prefix('+').unwrap_or(unsigned)),
    };
    let digits: String = digits.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let magnitude = digits.parse::<i64>().ok()?;
    Some(if negative { -magnitude } else { magnitude })
}
