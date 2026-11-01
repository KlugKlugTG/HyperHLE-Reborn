/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Host-side App Store-style browser for the Internet Archive. Network and
//! file I/O run on worker threads; UIKit objects stay on the picker thread.

mod network;

use super::{make_icon_from_text, new_label, new_view, rect, ui_color, AppPickerDelegateHostObject};
use crate::frameworks::core_graphics::{cg_image, CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_run_loop::run_run_loop_single_iteration;
use crate::frameworks::foundation::{ns_string, NSInteger};
use crate::frameworks::uikit::ui_font::{UITextAlignmentCenter, UITextAlignmentLeft};
use crate::frameworks::uikit::ui_view::ui_control::ui_button::UIButtonTypeCustom;
use crate::frameworks::uikit::ui_view::ui_control::{
    UIControlEventTouchUpInside, UIControlStateNormal,
};
use crate::image::Image;
use crate::objc::{id, msg, msg_class, nil, release};
use crate::Environment;
use network::{display_file_name, download_key, format_size};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

const CATALOG_ROWS: usize = 20;
const DETAIL_ROWS: usize = 20;
const CATALOG_ROW_HEIGHT: CGFloat = 86.0;
const FILE_ROW_HEIGHT: CGFloat = 80.0;
const DOWNLOAD_ROW_HEIGHT: CGFloat = 110.0;
const TOOLBAR_HEIGHT: CGFloat = 48.0;
const STATUS_HEIGHT: CGFloat = 30.0;
const PAGER_HEIGHT: CGFloat = 36.0;
const TAB_HEIGHT: CGFloat = 48.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum CatalogSort {
    Popular,
    Title,
    Newest,
    Oldest,
}

impl CatalogSort {
    fn next(self) -> Self {
        match self {
            Self::Popular => Self::Newest,
            Self::Newest => Self::Oldest,
            Self::Oldest => Self::Title,
            Self::Title => Self::Popular,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Popular => "Popular",
            Self::Title => "A–Z",
            Self::Newest => "Newest",
            Self::Oldest => "Oldest",
        }
    }

    fn query_value(self) -> &'static str {
        match self {
            // Match archive.org's default collection ordering: weekly views.
            Self::Popular => "week desc",
            Self::Title => "title asc",
            Self::Newest => "publicdate desc",
            Self::Oldest => "publicdate asc",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CatalogScope {
    Collections,
    All,
}

#[derive(Clone, Debug)]
struct CatalogItem {
    identifier: String,
    title: String,
    version: Option<String>,
    date: Option<String>,
    creator: Option<String>,
    files_count: Option<u64>,
}

#[derive(Debug)]
struct CatalogPage {
    items: Vec<CatalogItem>,
    num_found: u64,
    page: usize,
}

#[derive(Clone, Debug)]
struct ArchiveFile {
    name: String,
    size: Option<u64>,
    version: Option<String>,
}

#[derive(Debug)]
struct ItemDetails {
    identifier: String,
    title: String,
    version: Option<String>,
    creator: Option<String>,
    date: Option<String>,
    description: Option<String>,
    files: Vec<ArchiveFile>,
}

#[derive(Debug)]
struct DownloadOutcome {
    path: PathBuf,
    already_present: bool,
}

enum DownloadEvent {
    Progress {
        downloaded: u64,
        total: Option<u64>,
    },
    Finished {
        result: Result<DownloadOutcome, String>,
    },
}

enum StoreMessage {
    Catalog {
        generation: u64,
        result: Result<CatalogPage, String>,
    },
    Details {
        generation: u64,
        result: Result<ItemDetails, String>,
    },
    Artwork {
        identifier: String,
        result: Result<Vec<u8>, String>,
    },
    Download {
        generation: u64,
        event: DownloadEvent,
    },
}

#[derive(Default)]
struct ArtworkCache {
    requested: HashSet<String>,
    images: HashMap<String, id>,
}

impl ArtworkCache {
    fn request(&mut self, sender: &Sender<StoreMessage>, identifier: &str) {
        if self.requested.insert(identifier.to_string()) {
            request_artwork(sender, identifier.to_string());
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TaskState {
    Queued,
    Downloading,
    Installed,
    Failed,
    Cancelled,
}

struct DownloadTask {
    id: u64,
    key: String,
    identifier: String,
    title: String,
    file: ArchiveFile,
    state: TaskState,
    downloaded: u64,
    total: Option<u64>,
    started: Option<Instant>,
    path: Option<PathBuf>,
    already_present: bool,
    error: Option<String>,
    cancel: Arc<AtomicBool>,
}

impl DownloadTask {
    fn new(id: u64, identifier: &str, title: &str, file: &ArchiveFile) -> Self {
        Self {
            id,
            key: download_key(identifier, &file.name),
            identifier: identifier.to_string(),
            title: title.to_string(),
            file: file.clone(),
            state: TaskState::Queued,
            downloaded: 0,
            total: file.size,
            started: None,
            path: None,
            already_present: false,
            error: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn retry(&mut self) {
        self.state = TaskState::Queued;
        self.downloaded = 0;
        self.total = self.file.size;
        self.started = None;
        self.path = None;
        self.already_present = false;
        self.error = None;
        self.cancel.store(false, Ordering::Release);
    }

    fn fraction(&self) -> f64 {
        if self.state == TaskState::Installed {
            return 1.0;
        }
        self.total
            .filter(|total| *total > 0)
            .map(|total| self.downloaded as f64 / total as f64)
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    }
}

struct DownloadManager {
    tasks: Vec<DownloadTask>,
    active: Option<u64>,
    next_id: u64,
    downloaded_any: bool,
}

impl DownloadManager {
    fn new() -> Self {
        Self {
            tasks: Vec::new(),
            active: None,
            next_id: 1,
            downloaded_any: false,
        }
    }

    fn task(&self, id: u64) -> Option<&DownloadTask> {
        self.tasks.iter().find(|task| task.id == id)
    }

    fn task_mut(&mut self, id: u64) -> Option<&mut DownloadTask> {
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    fn active_task(&self) -> Option<&DownloadTask> {
        self.active.and_then(|id| self.task(id))
    }

    fn state_for_key(&self, key: &str) -> Option<TaskState> {
        self.tasks
            .iter()
            .find(|task| task.key == key)
            .map(|task| task.state)
    }

    fn enqueue(&mut self, identifier: &str, title: &str, file: &ArchiveFile) -> bool {
        let key = download_key(identifier, &file.name);
        if let Some(task) = self.tasks.iter_mut().find(|task| task.key == key) {
            match task.state {
                TaskState::Queued | TaskState::Downloading | TaskState::Installed => return false,
                TaskState::Failed | TaskState::Cancelled => {
                    task.retry();
                    return true;
                }
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.tasks.push(DownloadTask::new(id, identifier, title, file));
        true
    }

    fn start_next(&mut self, sender: &Sender<StoreMessage>, apps_dir: &Path) -> bool {
        if self.active.is_some() {
            return false;
        }
        let Some(task) = self
            .tasks
            .iter_mut()
            .find(|task| task.state == TaskState::Queued)
        else {
            return false;
        };
        task.state = TaskState::Downloading;
        task.started = Some(Instant::now());
        let id = task.id;
        let identifier = task.identifier.clone();
        let file = task.file.clone();
        let cancel = task.cancel.clone();
        self.active = Some(id);
        request_download(
            sender,
            id,
            apps_dir.to_path_buf(),
            identifier,
            file,
            cancel,
        );
        true
    }

    fn toggle(&mut self, id: u64) -> bool {
        let Some(task) = self.task_mut(id) else {
            return false;
        };
        match task.state {
            TaskState::Downloading => task.cancel.store(true, Ordering::Release),
            TaskState::Queued => task.state = TaskState::Cancelled,
            TaskState::Failed | TaskState::Cancelled => task.retry(),
            TaskState::Installed => return false,
        }
        true
    }

    fn retry_all(&mut self) -> bool {
        let mut changed = false;
        for task in &mut self.tasks {
            if matches!(task.state, TaskState::Failed | TaskState::Cancelled) {
                task.retry();
                changed = true;
            }
        }
        changed
    }

    fn cancel_all(&mut self) {
        for task in &mut self.tasks {
            match task.state {
                TaskState::Queued => task.state = TaskState::Cancelled,
                TaskState::Downloading => task.cancel.store(true, Ordering::Release),
                _ => (),
            }
        }
    }

    fn pending_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| matches!(task.state, TaskState::Queued | TaskState::Downloading))
            .count()
    }

    fn installed_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| task.state == TaskState::Installed)
            .count()
    }

    fn failed_count(&self) -> usize {
        self.tasks
            .iter()
            .filter(|task| matches!(task.state, TaskState::Failed | TaskState::Cancelled))
            .count()
    }
}

#[derive(Default)]
struct StoreActions {
    close: bool,
    search: bool,
    previous: bool,
    next: bool,
    back: bool,
    retry: bool,
    cancel: bool,
    sort: bool,
    fuzzy: bool,
    tap: Option<CGPoint>,
    item: Option<NSInteger>,
    download: Option<NSInteger>,
    tab: Option<NSInteger>,
    queue_action: Option<NSInteger>,
}

fn take_actions(env: &mut Environment, delegate: id) -> StoreActions {
    let host = env
        .objc
        .borrow_mut::<AppPickerDelegateHostObject>(delegate);
    StoreActions {
        close: std::mem::take(&mut host.archive_store_close),
        search: std::mem::take(&mut host.archive_store_search),
        previous: std::mem::take(&mut host.archive_store_previous),
        next: std::mem::take(&mut host.archive_store_next),
        back: std::mem::take(&mut host.archive_store_back),
        retry: std::mem::take(&mut host.archive_store_retry),
        cancel: std::mem::take(&mut host.archive_store_cancel),
        sort: std::mem::take(&mut host.archive_store_sort),
        fuzzy: std::mem::take(&mut host.archive_store_fuzzy),
        tap: std::mem::take(&mut host.archive_store_tap),
        item: std::mem::take(&mut host.archive_store_item),
        download: std::mem::take(&mut host.archive_store_download),
        tab: std::mem::take(&mut host.archive_store_tab),
        queue_action: std::mem::take(&mut host.archive_store_queue_action),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Browse,
    Details,
    Downloads,
}

enum ScrollTap {
    Item(usize),
    Download(usize),
    Queue(u64),
}

struct CatalogState {
    scope: CatalogScope,
    query: String,
    sort: CatalogSort,
    fuzzy: bool,
    page: usize,
    total: u64,
    loading: bool,
    generation: u64,
    current: Option<CatalogPage>,
    error: Option<String>,
}

impl CatalogState {
    fn new() -> Self {
        Self {
            scope: CatalogScope::Collections,
            query: String::new(),
            sort: CatalogSort::Popular,
            fuzzy: false,
            page: 1,
            total: 0,
            loading: false,
            generation: 0,
            current: None,
            error: None,
        }
    }

    fn select_scope(&mut self, sender: &Sender<StoreMessage>, scope: CatalogScope) {
        if self.scope == scope && (self.current.is_some() || self.loading) {
            return;
        }
        self.scope = scope;
        self.query.clear();
        self.sort = CatalogSort::Popular;
        self.fuzzy = false;
        self.load(sender, 1);
    }

    fn load(&mut self, sender: &Sender<StoreMessage>, page: usize) {
        self.page = page.max(1);
        self.loading = true;
        self.error = None;
        self.current = None;
        self.generation += 1;
        request_catalog(
            sender,
            self.generation,
            self.scope,
            self.query.clone(),
            self.page,
            CATALOG_ROWS,
            self.sort,
            self.fuzzy,
        );
    }

    fn page_count(&self) -> usize {
        ((self.total as usize + CATALOG_ROWS - 1) / CATALOG_ROWS).max(1)
    }
}

struct DetailsState {
    item: Option<CatalogItem>,
    details: Option<ItemDetails>,
    loading: bool,
    generation: u64,
    error: Option<String>,
    file_query: String,
    file_page: usize,
    matching_files: Vec<usize>,
}

impl DetailsState {
    fn new() -> Self {
        Self {
            item: None,
            details: None,
            loading: false,
            generation: 0,
            error: None,
            file_query: String::new(),
            file_page: 1,
            matching_files: Vec::new(),
        }
    }

    fn open(&mut self, sender: &Sender<StoreMessage>, item: CatalogItem) {
        self.file_query.clear();
        self.load(sender, item);
    }

    fn load(&mut self, sender: &Sender<StoreMessage>, item: CatalogItem) {
        self.generation += 1;
        self.details = None;
        self.matching_files.clear();
        self.file_page = 1;
        self.loading = true;
        self.error = None;
        let identifier = item.identifier.clone();
        self.item = Some(item);
        request_details(sender, self.generation, identifier);
    }

    fn set_file_query(&mut self, query: String) {
        self.file_query = query.trim().to_string();
        self.file_page = 1;
        self.rebuild_file_matches();
    }

    fn rebuild_file_matches(&mut self) {
        let terms: Vec<String> = self
            .file_query
            .split_whitespace()
            .map(|term| term.to_lowercase())
            .collect();
        self.matching_files = self
            .details
            .as_ref()
            .map(|details| {
                details
                    .files
                    .iter()
                    .enumerate()
                    .filter(|(_, file)| {
                        let name = file.name.to_lowercase();
                        terms.iter().all(|term| name.contains(term))
                    })
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default();
    }

    fn page_count(&self) -> usize {
        ((self.matching_files.len() + DETAIL_ROWS - 1) / DETAIL_ROWS).max(1)
    }

    fn file_index(&self, row: usize) -> Option<usize> {
        let position = (self.file_page.saturating_sub(1) * DETAIL_ROWS) + row;
        self.matching_files.get(position).copied()
    }
}

struct DownloadProgressViews {
    id: u64,
    label: id,
    fill: id,
    width: CGFloat,
}

struct StoreViews {
    root: id,
    delegate: id,
    nav_title: id,
    nav_subtitle: id,
    back_button: id,
    toolbar: id,
    search_field: id,
    search_button: id,
    sort_button: id,
    fuzzy_button: id,
    status_panel: id,
    status_label: id,
    scroll_view: id,
    pager: id,
    previous_button: id,
    next_button: id,
    page_label: id,
    collections_tab: id,
    all_tab: id,
    downloads_tab: id,
    width: CGFloat,
    full_height: CGFloat,
    nav_height: CGFloat,
    rows: Vec<id>,
    download_progress: Vec<DownloadProgressViews>,
    artwork_views: Vec<(String, id)>,
}

/// Open the store as a modal picker screen. The return value tells the home
/// screen whether it should re-enumerate the apps directory.
pub(super) fn run_archive_store(
    env: &mut Environment,
    run_loop: id,
    delegate: id,
    parent: id,
    app_frame: CGRect,
    apps_dir: &Path,
) -> bool {
    let mut views = build_store_views(env, delegate, parent, app_frame);
    let (sender, receiver) = mpsc::channel();
    let mut screen = Screen::Browse;
    let mut catalog = CatalogState::new();
    let mut details = DetailsState::new();
    let mut downloads = DownloadManager::new();
    let mut artwork = ArtworkCache::default();
    let mut closing = false;
    let mut finished = false;
    let mut dirty = true;

    catalog.load(&sender, 1);
    while !finished {
        run_run_loop_single_iteration(env, run_loop);
        drain_messages(
            env,
            &sender,
            &receiver,
            &mut views,
            screen,
            &mut catalog,
            &mut details,
            &mut downloads,
            &mut artwork,
            &mut dirty,
        );

        let actions = take_actions(env, delegate);
        if closing {
            if downloads.active.is_none() {
                finished = true;
            }
        } else if actions.close {
            closing = true;
            downloads.cancel_all();
            dirty = true;
            if downloads.active.is_none() {
                finished = true;
            }
        } else if actions.cancel {
            if let Some(active) = downloads.active_task() {
                active.cancel.store(true, Ordering::Release);
                dirty = true;
            }
        } else if actions.search {
            let text: id = msg![env; (views.search_field) text];
            let query = if text == nil {
                String::new()
            } else {
                ns_string::to_rust_string(env, text).into_owned()
            };
            let _: bool = msg![env; (views.search_field) resignFirstResponder];
            match screen {
                Screen::Browse => {
                    catalog.query = query.trim().to_string();
                    catalog.load(&sender, 1);
                }
                Screen::Details => details.set_file_query(query),
                Screen::Downloads => {}
            }
            dirty = true;
        } else if actions.previous && screen != Screen::Downloads {
            match screen {
                Screen::Browse if !catalog.loading && catalog.page > 1 => {
                    catalog.load(&sender, catalog.page - 1);
                    dirty = true;
                }
                Screen::Details if !details.loading && details.file_page > 1 => {
                    details.file_page -= 1;
                    dirty = true;
                }
                _ => {}
            }
        } else if actions.next && screen != Screen::Downloads {
            match screen {
                Screen::Browse if !catalog.loading && catalog.page_count() > catalog.page => {
                    catalog.load(&sender, catalog.page + 1);
                    dirty = true;
                }
                Screen::Details
                    if !details.loading && details.page_count() > details.file_page =>
                {
                    details.file_page += 1;
                    dirty = true;
                }
                _ => {}
            }
        } else if actions.back && screen == Screen::Details {
            screen = Screen::Browse;
            set_label(env, views.search_field, &catalog.query);
            dirty = true;
        } else if let Some(tab) = actions.tab {
            let _: bool = msg![env; (views.search_field) resignFirstResponder];
            if tab == 2 {
                screen = Screen::Downloads;
            } else {
                screen = Screen::Browse;
                let scope = if tab == 1 {
                    CatalogScope::All
                } else {
                    CatalogScope::Collections
                };
                catalog.select_scope(&sender, scope);
                set_label(env, views.search_field, &catalog.query);
            }
            dirty = true;
        } else if actions.sort && screen == Screen::Browse {
            catalog.sort = catalog.sort.next();
            catalog.load(&sender, 1);
            dirty = true;
        } else if actions.fuzzy && screen == Screen::Browse {
            let text: id = msg![env; (views.search_field) text];
            if text != nil {
                catalog.query = ns_string::to_rust_string(env, text).trim().to_string();
            }
            let _: bool = msg![env; (views.search_field) resignFirstResponder];
            catalog.fuzzy = !catalog.fuzzy;
            if !catalog.query.is_empty() {
                catalog.load(&sender, 1);
            }
            dirty = true;
        } else if let Some(point) = actions.tap {
            let target = scroll_tap_target(
                &views, screen, point, &catalog, &details, &downloads,
            );
            if let Some(target) = target {
                match target {
                    ScrollTap::Item(index) => {
                        let item = catalog
                            .current
                            .as_ref()
                            .and_then(|page| page.items.get(index))
                            .cloned();
                        if let Some(item) = item {
                            screen = Screen::Details;
                            details.open(&sender, item);
                            set_label(env, views.search_field, "");
                        }
                    }
                    ScrollTap::Download(index) => {
                        if let Some(item_details) = &details.details {
                            let file = item_details.files.get(index).cloned();
                            if let Some(file) = file {
                                downloads.enqueue(
                                    &item_details.identifier,
                                    &item_details.title,
                                    &file,
                                );
                            }
                        }
                    }
                    ScrollTap::Queue(id) => {
                        downloads.toggle(id);
                    }
                }
                dirty = true;
            }
        } else if actions.retry {
            match screen {
                Screen::Browse => catalog.load(&sender, catalog.page),
                Screen::Details => {
                    if let Some(item) = details.item.clone() {
                        details.load(&sender, item);
                    }
                }
                Screen::Downloads => {
                    downloads.retry_all();
                }
            }
            dirty = true;
        } else if let Some(index) = actions.item {
            if screen == Screen::Browse && !catalog.loading {
                let item = catalog
                    .current
                    .as_ref()
                    .and_then(|page| page.items.get(index.max(0) as usize))
                    .cloned();
                if let Some(item) = item {
                    screen = Screen::Details;
                    details.open(&sender, item);
                    set_label(env, views.search_field, "");
                    dirty = true;
                }
            }
        } else if let Some(index) = actions.download {
            if screen == Screen::Details {
                if let Some(item_details) = &details.details {
                    let file = item_details.files.get(index.max(0) as usize).cloned();
                    if let Some(file) = file {
                        downloads.enqueue(&item_details.identifier, &item_details.title, &file);
                        dirty = true;
                    }
                }
            }
        } else if let Some(id) = actions.queue_action {
            if id >= 0 {
                downloads.toggle(id as u64);
                dirty = true;
            }
        }

        if !closing && downloads.start_next(&sender, apps_dir) {
            dirty = true;
        }
        if closing && downloads.active.is_none() {
            finished = true;
        }
        if dirty && !finished {
            render(
                env,
                &mut views,
                screen,
                &catalog,
                &details,
                &downloads,
                &artwork,
            );
            dirty = false;
        }
    }

    let _: bool = msg![env; (views.search_field) resignFirstResponder];
    () = msg![env; (views.root) removeFromSuperview];
    let host = env
        .objc
        .borrow_mut::<AppPickerDelegateHostObject>(delegate);
    host.archive_store_close = false;
    host.archive_store_search = false;
    host.archive_store_previous = false;
    host.archive_store_next = false;
    host.archive_store_back = false;
    host.archive_store_retry = false;
    host.archive_store_cancel = false;
    host.archive_store_sort = false;
    host.archive_store_fuzzy = false;
    host.archive_store_item = None;
    host.archive_store_download = None;
    host.archive_store_tab = None;
    host.archive_store_queue_action = None;
    host.archive_store_tap = None;
    downloads.downloaded_any
}

#[allow(clippy::too_many_arguments)]
fn drain_messages(
    env: &mut Environment,
    sender: &Sender<StoreMessage>,
    receiver: &Receiver<StoreMessage>,
    views: &mut StoreViews,
    screen: Screen,
    catalog: &mut CatalogState,
    details: &mut DetailsState,
    downloads: &mut DownloadManager,
    artwork: &mut ArtworkCache,
    dirty: &mut bool,
) {
    while let Ok(message) = receiver.try_recv() {
        match message {
            StoreMessage::Catalog { generation, result } => {
                if generation != catalog.generation {
                    continue;
                }
                catalog.loading = false;
                match result {
                    Ok(page) => {
                        for item in &page.items {
                            artwork.request(sender, &item.identifier);
                        }
                        catalog.total = page.num_found;
                        catalog.page = page.page;
                        catalog.current = Some(page);
                        catalog.error = None;
                    }
                    Err(error) => {
                        catalog.current = None;
                        catalog.error = Some(error);
                    }
                }
                *dirty = true;
            }
            StoreMessage::Details { generation, result } => {
                if generation != details.generation {
                    continue;
                }
                details.loading = false;
                match result {
                    Ok(item_details) => {
                        artwork.request(sender, &item_details.identifier);
                        details.details = Some(item_details);
                        details.file_page = 1;
                        details.rebuild_file_matches();
                        details.error = None;
                    }
                    Err(error) => {
                        details.details = None;
                        details.error = Some(error);
                    }
                }
                *dirty = true;
            }
            StoreMessage::Artwork { identifier, result } => {
                let Ok(bytes) = result else {
                    continue;
                };
                let Ok(mut image) = Image::from_bytes(&bytes) else {
                    continue;
                };
                image.round_icon_corners(/* add_sheen: */ true);
                let cg = cg_image::from_image(env, image);
                let image: id = msg_class![env; UIImage imageWithCGImage:cg];
                artwork.images.insert(identifier.clone(), image);
                for (key, image_view) in &views.artwork_views {
                    if key == &identifier {
                        () = msg![env; (*image_view) setImage:image];
                    }
                }
            }
            StoreMessage::Download { generation, event } => match event {
                DownloadEvent::Progress { downloaded, total } => {
                    if downloads.active == Some(generation) {
                        if let Some(task) = downloads.task_mut(generation) {
                            task.downloaded = downloaded;
                            task.total = total;
                        }
                        update_download_progress(env, views, screen, catalog, downloads);
                    }
                }
                DownloadEvent::Finished { result } => {
                    if downloads.active != Some(generation) {
                        continue;
                    }
                    downloads.active = None;
                    let mut installed = false;
                    if let Some(task) = downloads.task_mut(generation) {
                        match result {
                            Ok(outcome) => {
                                task.state = TaskState::Installed;
                                task.path = Some(outcome.path);
                                task.already_present = outcome.already_present;
                                task.error = None;
                                installed = true;
                            }
                            Err(error) => {
                                if error == "Download cancelled." {
                                    task.state = TaskState::Cancelled;
                                    task.error = None;
                                } else {
                                    task.state = TaskState::Failed;
                                    task.error = Some(error);
                                }
                            }
                        }
                    }
                    if installed {
                        downloads.downloaded_any = true;
                    }
                    *dirty = true;
                }
            },
        }
    }
}

fn build_store_views(
    env: &mut Environment,
    delegate: id,
    parent: id,
    app_frame: CGRect,
) -> StoreViews {
    let width = app_frame.size.width;
    let status_offset = app_frame.origin.y;
    let full_height = app_frame.size.height + status_offset;
    let nav_height = 52.0 + status_offset;
    let background = ui_color(env, 0.94, 0.95, 0.98, 1.0);
    let root = new_view(
        env,
        rect(0.0, -status_offset, width, full_height),
        background,
    );
    () = msg![env; parent addSubview:root];

    let nav_color = ui_color(env, 0.035, 0.10, 0.20, 1.0);
    let nav = new_view(env, rect(0.0, 0.0, width, nav_height), nav_color);
    () = msg![env; root addSubview:nav];
    let white: id = msg_class![env; UIColor whiteColor];
    let clear: id = msg_class![env; UIColor clearColor];
    let nav_title = new_label(
        env,
        rect(60.0, status_offset + 3.0, width - 120.0, 24.0),
        "IPA Store",
        17.0,
        true,
        white,
    );
    () = msg![env; nav_title setTextAlignment:UITextAlignmentCenter];
    () = msg![env; root addSubview:nav_title];
    let subtitle_color = ui_color(env, 0.67, 0.77, 0.88, 1.0);
    let nav_subtitle = new_label(
        env,
        rect(60.0, status_offset + 28.0, width - 120.0, 17.0),
        "Internet Archive",
        10.0,
        false,
        subtitle_color,
    );
    () = msg![env; nav_subtitle setTextAlignment:UITextAlignmentCenter];
    () = msg![env; root addSubview:nav_subtitle];

    let back_button = make_button(
        env,
        delegate,
        root,
        rect(5.0, status_offset + 10.0, 56.0, 32.0),
        "< Back",
        "archiveStoreBack",
        0,
        clear,
        white,
    );
    () = msg![env; back_button setHidden:true];
    let done_button = make_button(
        env,
        delegate,
        root,
        rect(width - 59.0, status_offset + 10.0, 54.0, 32.0),
        "Done",
        "archiveStoreClose",
        0,
        clear,
        white,
    );

    let toolbar = new_view(
        env,
        rect(0.0, nav_height, width, TOOLBAR_HEIGHT),
        background,
    );
    () = msg![env; root addSubview:toolbar];
    let field_width = (width - 194.0).max(90.0);
    let search_field: id = msg_class![env; UITextField alloc];
    let search_field: id = msg![env; search_field initWithFrame:(rect(
        8.0,
        7.0,
        field_width,
        34.0
    ))];
    let field_font: id = msg_class![env; UIFont systemFontOfSize:(13.0 as CGFloat)];
    () = msg![env; search_field setFont:field_font];
    () = msg![env; search_field setDelegate:delegate];
    () = msg![env; search_field setReturnKeyType:(6 as NSInteger)];
    () = msg![env; search_field setAutocapitalizationType:(0 as NSInteger)];
    () = msg![env; search_field setAutocorrectionType:(1 as NSInteger)];
    () = msg![env; search_field setBackgroundColor:white];
    let placeholder = ns_string::from_rust_string(env, "Search apps, IDs, developers".to_string());
    () = msg![env; search_field setPlaceholder:placeholder];
    release(env, placeholder);
    let field_layer: id = msg![env; search_field layer];
    let field_border = ui_color(env, 0.78, 0.82, 0.88, 1.0);
    let field_border_cg: id = msg![env; field_border CGColor];
    () = msg![env; field_layer setBorderColor:field_border_cg];
    () = msg![env; field_layer setBorderWidth:(1.0 as CGFloat)];
    () = msg![env; field_layer setCornerRadius:(8.0 as CGFloat)];
    () = msg![env; toolbar addSubview:search_field];

    let blue = ui_color(env, 0.0, 0.40, 0.82, 1.0);
    let search_button = make_button(
        env,
        delegate,
        toolbar,
        rect(width - 180.0, 7.0, 40.0, 34.0),
        "Go",
        "archiveStoreSearch",
        0,
        blue,
        white,
    );
    let sort_background = ui_color(env, 0.86, 0.91, 0.98, 1.0);
    let fuzzy_button = make_button(
        env,
        delegate,
        toolbar,
        rect(width - 134.0, 7.0, 54.0, 34.0),
        "Fuzzy",
        "archiveStoreFuzzy",
        0,
        sort_background,
        blue,
    );
    let sort_button = make_button(
        env,
        delegate,
        toolbar,
        rect(width - 74.0, 7.0, 66.0, 34.0),
        "Popular",
        "archiveStoreSort",
        0,
        sort_background,
        blue,
    );

    let gray = ui_color(env, 0.32, 0.37, 0.44, 1.0);
    let status_panel = new_view(
        env,
        rect(0.0, nav_height + TOOLBAR_HEIGHT, width, STATUS_HEIGHT),
        background,
    );
    () = msg![env; root addSubview:status_panel];
    let status_label = new_label(
        env,
        rect(10.0, 0.0, width - 20.0, STATUS_HEIGHT),
        "",
        11.0,
        false,
        gray,
    );
    () = msg![env; status_label setNumberOfLines:2];
    () = msg![env; status_label setTextAlignment:UITextAlignmentLeft];
    () = msg![env; status_panel addSubview:status_label];

    let scroll_y = nav_height + TOOLBAR_HEIGHT + STATUS_HEIGHT;
    let scroll_view: id = msg_class![env; UIScrollView alloc];
    let scroll_view: id = msg![env; scroll_view initWithFrame:(rect(
        0.0,
        scroll_y,
        width,
        full_height - scroll_y - PAGER_HEIGHT - TAB_HEIGHT
    ))];
    () = msg![env; scroll_view setBackgroundColor:clear];
    () = msg![env; scroll_view setClipsToBounds:true];
    () = msg![env; scroll_view setShowsHorizontalScrollIndicator:false];
    () = msg![env; root addSubview:scroll_view];
    let tap_selector = env.objc.lookup_selector("archiveStoreTap:").unwrap();
    let tap: id = msg_class![env; UITapGestureRecognizer alloc];
    let tap: id = msg![env; tap initWithTarget:delegate action:tap_selector];
    () = msg![env; tap setCancelsTouchesInView:false];
    () = msg![env; scroll_view addGestureRecognizer:tap];

    let pager = new_view(
        env,
        rect(0.0, full_height - TAB_HEIGHT - PAGER_HEIGHT, width, PAGER_HEIGHT),
        background,
    );
    () = msg![env; root addSubview:pager];
    let previous_button = make_button(
        env,
        delegate,
        pager,
        rect(8.0, 3.0, 70.0, 30.0),
        "< Prev",
        "archiveStorePrevious",
        0,
        sort_background,
        blue,
    );
    let next_button = make_button(
        env,
        delegate,
        pager,
        rect(width - 78.0, 3.0, 70.0, 30.0),
        "Next >",
        "archiveStoreNext",
        0,
        sort_background,
        blue,
    );
    let page_label = new_label(
        env,
        rect(80.0, 3.0, width - 160.0, 30.0),
        "Page 1",
        11.0,
        false,
        gray,
    );
    () = msg![env; page_label setTextAlignment:UITextAlignmentCenter];
    () = msg![env; pager addSubview:page_label];

    let tab_bar = new_view(
        env,
        rect(0.0, full_height - TAB_HEIGHT, width, TAB_HEIGHT),
        white,
    );
    () = msg![env; root addSubview:tab_bar];
    let tab_separator_color = ui_color(env, 0.80, 0.83, 0.88, 1.0);
    let tab_separator = new_view(
        env,
        rect(0.0, 0.0, width, 1.0),
        tab_separator_color,
    );
    () = msg![env; tab_bar addSubview:tab_separator];
    let collections_tab = make_button(
        env,
        delegate,
        tab_bar,
        rect(0.0, 1.0, width / 3.0, TAB_HEIGHT - 1.0),
        "Collections",
        "archiveStoreTab:",
        0,
        clear,
        blue,
    );
    let all_tab = make_button(
        env,
        delegate,
        tab_bar,
        rect(width / 3.0, 1.0, width / 3.0, TAB_HEIGHT - 1.0),
        "All IPAs",
        "archiveStoreTab:",
        1,
        clear,
        blue,
    );
    let downloads_tab = make_button(
        env,
        delegate,
        tab_bar,
        rect(width * 2.0 / 3.0, 1.0, width / 3.0, TAB_HEIGHT - 1.0),
        "Downloads",
        "archiveStoreTab:",
        2,
        clear,
        blue,
    );

    // UIScrollView clips hit-testing but the host compositor does not clip
    // every descendant. Keep opaque chrome above overflowing content.
    for view in [
        nav,
        toolbar,
        status_panel,
        nav_title,
        nav_subtitle,
        back_button,
        done_button,
    ] {
        () = msg![env; root bringSubviewToFront:view];
    }

    StoreViews {
        root,
        delegate,
        nav_title,
        nav_subtitle,
        back_button,
        toolbar,
        search_field,
        search_button,
        sort_button,
        fuzzy_button,
        status_panel,
        status_label,
        scroll_view,
        pager,
        previous_button,
        next_button,
        page_label,
        collections_tab,
        all_tab,
        downloads_tab,
        width,
        full_height,
        nav_height,
        rows: Vec::new(),
        download_progress: Vec::new(),
        artwork_views: Vec::new(),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn make_button(
    env: &mut Environment,
    delegate: id,
    parent: id,
    frame: CGRect,
    title: &str,
    action: &str,
    tag: NSInteger,
    background: id,
    title_color: id,
) -> id {
    let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; button setFrame:frame];
    () = msg![env; button setBackgroundColor:background];
    set_button_title(env, button, title);
    () = msg![env; button setTitleColor:title_color forState:UIControlStateNormal];
    () = msg![env; button setTag:tag];
    () = msg![env; button layoutSubviews];
    let label: id = msg![env; button titleLabel];
    let font: id = msg_class![env; UIFont boldSystemFontOfSize:(12.0 as CGFloat)];
    () = msg![env; label setFont:font];
    let layer: id = msg![env; button layer];
    () = msg![env; layer setCornerRadius:(7.0 as CGFloat)];
    let selector = env.objc.lookup_selector(action).unwrap();
    () = msg![env; button addTarget:delegate
                              action:selector
                    forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; parent addSubview:button];
    button
}

pub(super) fn set_label(env: &mut Environment, label: id, text: &str) {
    let text = ns_string::from_rust_string(env, text.to_string());
    () = msg![env; label setText:text];
    release(env, text);
}

pub(super) fn set_button_title(env: &mut Environment, button: id, title: &str) {
    let title = ns_string::from_rust_string(env, title.to_string());
    () = msg![env; button setTitle:title forState:UIControlStateNormal];
    release(env, title);
}

fn set_button_enabled(env: &mut Environment, button: id, enabled: bool) {
    () = msg![env; button setEnabled:enabled];
    let alpha: CGFloat = if enabled { 1.0 } else { 0.48 };
    () = msg![env; button setAlpha:alpha];
}

fn render(
    env: &mut Environment,
    views: &mut StoreViews,
    screen: Screen,
    catalog: &CatalogState,
    details: &DetailsState,
    downloads: &DownloadManager,
    artwork: &ArtworkCache,
) {
    update_chrome(env, views, screen, catalog, details, downloads);
    clear_rows(env, views);
    let content_height = match screen {
        Screen::Browse => render_catalog(env, views, catalog, artwork),
        Screen::Details => render_details(env, views, details, downloads, artwork),
        Screen::Downloads => render_downloads(env, views, downloads),
    };
    reset_scroll(env, views, content_height);
}

fn update_chrome(
    env: &mut Environment,
    views: &StoreViews,
    screen: Screen,
    catalog: &CatalogState,
    details: &DetailsState,
    downloads: &DownloadManager,
) {
    let browsing = screen == Screen::Browse;
    let searching = screen != Screen::Downloads;
    () = msg![env; (views.toolbar) setHidden:(!searching)];
    () = msg![env; (views.pager) setHidden:(!searching)];
    () = msg![env; (views.back_button) setHidden:(screen != Screen::Details)];
    () = msg![env; (views.sort_button) setHidden:(!browsing)];
    () = msg![env; (views.fuzzy_button) setHidden:(!browsing)];
    let title = match screen {
        Screen::Details => details
            .item
            .as_ref()
            .map(|item| item.title.as_str())
            .unwrap_or("App details"),
        _ => "IPA Store",
    };
    set_label(env, views.nav_title, title);
    set_button_title(env, views.sort_button, catalog.sort.label());

    let blue = ui_color(env, 0.0, 0.40, 0.82, 1.0);
    let white: id = msg_class![env; UIColor whiteColor];
    let selected = ui_color(env, 0.86, 0.91, 0.98, 1.0);
    let (fuzzy_background, fuzzy_color) = if catalog.fuzzy {
        (blue, white)
    } else {
        (selected, blue)
    };
    () = msg![env; (views.fuzzy_button) setBackgroundColor:fuzzy_background];
    () = msg![env; (views.fuzzy_button) setTitleColor:fuzzy_color forState:UIControlStateNormal];
    if browsing {
        () = msg![env; (views.search_field) setFrame:(rect(
            8.0,
            7.0,
            (views.width - 194.0).max(90.0),
            34.0
        ))];
        () = msg![env; (views.search_button) setFrame:(rect(
            views.width - 180.0,
            7.0,
            40.0,
            34.0
        ))];
    } else {
        () = msg![env; (views.search_field) setFrame:(rect(
            8.0,
            7.0,
            (views.width - 90.0).max(90.0),
            34.0
        ))];
        () = msg![env; (views.search_button) setFrame:(rect(
            views.width - 74.0,
            7.0,
            66.0,
            34.0
        ))];
    }
    let placeholder_text = if browsing {
        "Search apps, IDs, developers"
    } else {
        "Search IPA files in this collection"
    };
    let placeholder = ns_string::from_rust_string(env, placeholder_text.to_string());
    () = msg![env; (views.search_field) setPlaceholder:placeholder];
    release(env, placeholder);

    let toolbar_height = if searching { TOOLBAR_HEIGHT } else { 0.0 };
    let status_y = views.nav_height + toolbar_height;
    () = msg![env; (views.status_panel) setFrame:(rect(
        0.0,
        status_y,
        views.width,
        STATUS_HEIGHT
    ))];
    let scroll_y = status_y + STATUS_HEIGHT;
    let bottom = TAB_HEIGHT + if searching { PAGER_HEIGHT } else { 0.0 };
    () = msg![env; (views.scroll_view) setFrame:(rect(
        0.0,
        scroll_y,
        views.width,
        (views.full_height - scroll_y - bottom).max(40.0)
    ))];

    let status = match screen {
        Screen::Browse => catalog_status(catalog),
        Screen::Details => details_status(details),
        Screen::Downloads => downloads_status(downloads),
    };
    set_label(env, views.status_label, &status);
    let (page, page_count, loading) = match screen {
        Screen::Browse => (catalog.page, catalog.page_count(), catalog.loading),
        Screen::Details => (details.file_page, details.page_count(), details.loading),
        Screen::Downloads => (1, 1, false),
    };
    set_label(
        env,
        views.page_label,
        &format!("Page {} / {}", page.min(page_count), page_count),
    );
    set_button_enabled(env, views.previous_button, !loading && page > 1);
    set_button_enabled(
        env,
        views.next_button,
        !loading && page < page_count,
    );

    let pending = downloads.pending_count();
    let downloads_title = if pending == 0 {
        "Downloads".to_string()
    } else {
        format!("Downloads ({pending})")
    };
    set_button_title(env, views.downloads_tab, &downloads_title);
    let clear: id = msg_class![env; UIColor clearColor];
    let collections_background = if screen != Screen::Downloads
        && catalog.scope == CatalogScope::Collections
    {
        selected
    } else {
        clear
    };
    let all_background = if screen != Screen::Downloads && catalog.scope == CatalogScope::All {
        selected
    } else {
        clear
    };
    let downloads_background = if screen == Screen::Downloads {
        selected
    } else {
        clear
    };
    () = msg![env; (views.collections_tab) setBackgroundColor:collections_background];
    () = msg![env; (views.all_tab) setBackgroundColor:all_background];
    () = msg![env; (views.downloads_tab) setBackgroundColor:downloads_background];
    update_nav_subtitle(env, views, screen, catalog, downloads);
}

fn update_nav_subtitle(
    env: &mut Environment,
    views: &StoreViews,
    screen: Screen,
    catalog: &CatalogState,
    downloads: &DownloadManager,
) {
    let text = if let Some(task) = downloads.active_task() {
        if task.cancel.load(Ordering::Acquire) {
            "Cancelling download…".to_string()
        } else if task.total.is_some_and(|total| total > 0) {
            format!(
                "Downloading • {:.0}% • {} in queue",
                task.fraction() * 100.0,
                downloads.pending_count().saturating_sub(1)
            )
        } else {
            format!("Downloading {}…", format_size(task.downloaded))
        }
    } else {
        match screen {
            Screen::Browse => match catalog.scope {
                CatalogScope::Collections => "Internet Archive • Collections".to_string(),
                CatalogScope::All => "Internet Archive • All IPAs".to_string(),
            },
            Screen::Details => "Collection information and IPA files".to_string(),
            Screen::Downloads => "Queue, progress and installation".to_string(),
        }
    };
    set_label(env, views.nav_subtitle, &text);
}

fn catalog_status(catalog: &CatalogState) -> String {
    let noun = match catalog.scope {
        CatalogScope::Collections => "collections",
        CatalogScope::All => "apps",
    };
    if catalog.loading {
        return if catalog.query.is_empty() {
            format!("Loading {noun}…")
        } else {
            format!("Searching for “{}”…", catalog.query)
        };
    }
    if catalog.error.is_some() {
        return "Couldn't load the catalog.".to_string();
    }
    let Some(page) = &catalog.current else {
        return "Browse the Internet Archive collection.".to_string();
    };
    if page.items.is_empty() {
        return if catalog.query.is_empty() {
            format!("No {noun} found.")
        } else {
            format!("No results for “{}”. Try Fuzzy search.", catalog.query)
        };
    }
    let first = (page.page - 1) * CATALOG_ROWS + 1;
    let last = first + page.items.len() - 1;
    let query = if catalog.query.is_empty() {
        noun.to_string()
    } else {
        format!("results for “{}”", catalog.query)
    };
    let fuzzy = if catalog.fuzzy { " • Fuzzy" } else { "" };
    format!(
        "{first}–{last} of {} {query} • {}{fuzzy}",
        catalog.total,
        catalog.sort.label()
    )
}

fn details_status(details: &DetailsState) -> String {
    if details.loading {
        return "Loading collection details and IPA files…".to_string();
    }
    if details.error.is_some() {
        return "Couldn't load collection details.".to_string();
    }
    let Some(loaded) = &details.details else {
        return "Collection information.".to_string();
    };
    if !details.file_query.is_empty() {
        return format!(
            "{} of {} IPA files match “{}”",
            details.matching_files.len(),
            loaded.files.len(),
            details.file_query
        );
    }
    let total_size: u64 = loaded.files.iter().filter_map(|file| file.size).sum();
    let mut text = format!("{} IPA files", loaded.files.len());
    if total_size > 0 {
        text.push_str(&format!(" • {} combined", format_size(total_size)));
    }
    text
}

fn downloads_status(downloads: &DownloadManager) -> String {
    if downloads.tasks.is_empty() {
        return "No downloads yet. Choose GET on an app version.".to_string();
    }
    format!(
        "{} in queue • {} installed • {} to retry",
        downloads.pending_count(),
        downloads.installed_count(),
        downloads.failed_count()
    )
}

fn scroll_tap_target(
    views: &StoreViews,
    screen: Screen,
    point: CGPoint,
    catalog: &CatalogState,
    details: &DetailsState,
    downloads: &DownloadManager,
) -> Option<ScrollTap> {
    if point.x < 8.0 || point.x > views.width - 8.0 || point.y < 0.0 {
        return None;
    }
    let action_x = point.x >= views.width - 88.0 && point.x <= views.width - 20.0;
    match screen {
        Screen::Browse => {
            let page = catalog.current.as_ref()?;
            let relative = point.y - 4.0;
            if relative < 0.0 {
                return None;
            }
            let index = (relative / CATALOG_ROW_HEIGHT).floor() as usize;
            let within_row = relative - index as CGFloat * CATALOG_ROW_HEIGHT;
            (within_row <= 80.0 && index < page.items.len()).then_some(ScrollTap::Item(index))
        }
        Screen::Details => {
            if !action_x || details.details.is_none() {
                return None;
            }
            let relative = point.y - 228.0;
            if relative < 0.0 {
                return None;
            }
            let row = (relative / FILE_ROW_HEIGHT).floor() as usize;
            let within_row = relative - row as CGFloat * FILE_ROW_HEIGHT;
            if within_row < 21.0 || within_row > 53.0 || row >= DETAIL_ROWS {
                return None;
            }
            details.file_index(row).map(ScrollTap::Download)
        }
        Screen::Downloads => {
            if !action_x {
                return None;
            }
            let relative = point.y - 4.0;
            if relative < 0.0 {
                return None;
            }
            let index = (relative / DOWNLOAD_ROW_HEIGHT).floor() as usize;
            let within_row = relative - index as CGFloat * DOWNLOAD_ROW_HEIGHT;
            if within_row < 12.0 || within_row > 42.0 {
                return None;
            }
            let task = downloads.tasks.iter().rev().nth(index)?;
            (task.state != TaskState::Installed).then_some(ScrollTap::Queue(task.id))
        }
    }
}

fn clear_rows(env: &mut Environment, views: &mut StoreViews) {
    for row in views.rows.drain(..) {
        () = msg![env; row removeFromSuperview];
    }
    views.download_progress.clear();
    views.artwork_views.clear();
}

fn reset_scroll(env: &mut Environment, views: &StoreViews, content_height: CGFloat) {
    let frame: CGRect = msg![env; (views.scroll_view) frame];
    () = msg![env; (views.scroll_view) setContentSize:(CGSize {
        width: frame.size.width,
        height: content_height.max(frame.size.height),
    })];
    () = msg![env; (views.scroll_view) setContentOffset:(CGPoint { x: 0.0, y: 0.0 })];
}

fn render_catalog(
    env: &mut Environment,
    views: &mut StoreViews,
    catalog: &CatalogState,
    artwork: &ArtworkCache,
) -> CGFloat {
    if catalog.loading {
        let message = match catalog.scope {
            CatalogScope::Collections => "Loading IPA collections…",
            CatalogScope::All => "Loading the complete IPA catalog…",
        };
        add_message(env, views, message, false);
        return 210.0;
    }
    if let Some(error) = &catalog.error {
        add_message(env, views, error, true);
        return 240.0;
    }
    let Some(page) = &catalog.current else {
        add_message(env, views, "The catalog will appear here.", false);
        return 210.0;
    };
    if page.items.is_empty() {
        let message = if catalog.fuzzy {
            "No similar archives matched. Try fewer words or switch to All IPAs."
        } else {
            "No archives matched. Try Fuzzy search for similar titles and abbreviations."
        };
        add_message(env, views, message, false);
        return 210.0;
    }

    let row_width = views.width - 16.0;
    let dark: id = msg_class![env; UIColor blackColor];
    let white: id = msg_class![env; UIColor whiteColor];
    let gray = ui_color(env, 0.38, 0.43, 0.50, 1.0);
    for (index, item) in page.items.iter().enumerate() {
        let y = 4.0 + index as CGFloat * CATALOG_ROW_HEIGHT;
        let row = new_view(env, rect(8.0, y, row_width, 80.0), white);
        () = msg![env; row setUserInteractionEnabled:false];
        let row_layer: id = msg![env; row layer];
        () = msg![env; row_layer setCornerRadius:(10.0 as CGFloat)];
        () = msg![env; (views.scroll_view) addSubview:row];
        add_item_icon(
            env,
            views,
            row,
            &item.identifier,
            &item.title,
            rect(10.0, 12.0, 56.0, 56.0),
            artwork,
        );
        let title = new_label(
            env,
            rect(76.0, 8.0, row_width - 102.0, 40.0),
            &item.title,
            14.0,
            true,
            dark,
        );
        () = msg![env; title setNumberOfLines:2];
        () = msg![env; row addSubview:title];
        let subtitle = new_label(
            env,
            rect(76.0, 50.0, row_width - 102.0, 24.0),
            &catalog_subtitle(item),
            10.0,
            false,
            gray,
        );
        () = msg![env; subtitle setNumberOfLines:2];
        () = msg![env; row addSubview:subtitle];
        let chevron = new_label(
            env,
            rect(row_width - 24.0, 25.0, 14.0, 30.0),
            ">",
            20.0,
            false,
            gray,
        );
        () = msg![env; chevron setTextAlignment:UITextAlignmentCenter];
        () = msg![env; row addSubview:chevron];
        views.rows.push(row);
    }
    8.0 + page.items.len() as CGFloat * CATALOG_ROW_HEIGHT
}

fn render_details(
    env: &mut Environment,
    views: &mut StoreViews,
    details: &DetailsState,
    downloads: &DownloadManager,
    artwork: &ArtworkCache,
) -> CGFloat {
    let Some(item) = &details.item else {
        add_message(env, views, "Choose an app from the catalog.", false);
        return 210.0;
    };
    let row_width = views.width - 16.0;
    let white: id = msg_class![env; UIColor whiteColor];
    let dark: id = msg_class![env; UIColor blackColor];
    let gray = ui_color(env, 0.38, 0.43, 0.50, 1.0);
    let header = new_view(env, rect(8.0, 4.0, row_width, 188.0), white);
    () = msg![env; header setUserInteractionEnabled:false];
    let header_layer: id = msg![env; header layer];
    () = msg![env; header_layer setCornerRadius:(10.0 as CGFloat)];
    () = msg![env; (views.scroll_view) addSubview:header];
    views.rows.push(header);

    let loaded = details.details.as_ref();
    let title = loaded.map(|loaded| loaded.title.as_str()).unwrap_or(&item.title);
    let identifier = loaded
        .map(|loaded| loaded.identifier.as_str())
        .unwrap_or(&item.identifier);
    add_item_icon(
        env,
        views,
        header,
        identifier,
        title,
        rect(12.0, 12.0, 64.0, 64.0),
        artwork,
    );
    let title_label = new_label(
        env,
        rect(86.0, 10.0, row_width - 98.0, 44.0),
        title,
        15.0,
        true,
        dark,
    );
    () = msg![env; title_label setNumberOfLines:2];
    () = msg![env; header addSubview:title_label];
    let metadata = loaded.map(details_metadata).unwrap_or_else(|| catalog_subtitle(item));
    let metadata_label = new_label(
        env,
        rect(86.0, 56.0, row_width - 98.0, 46.0),
        &metadata,
        10.0,
        false,
        gray,
    );
    () = msg![env; metadata_label setNumberOfLines:3];
    () = msg![env; header addSubview:metadata_label];
    let description = loaded
        .and_then(|loaded| loaded.description.as_deref())
        .unwrap_or("No description is available for this archive entry.");
    let description = truncate_chars(description, 360);
    let description_label = new_label(
        env,
        rect(12.0, 108.0, row_width - 24.0, 70.0),
        &description,
        11.0,
        false,
        gray,
    );
    () = msg![env; description_label setNumberOfLines:4];
    () = msg![env; header addSubview:description_label];

    let section = new_label(
        env,
        rect(14.0, 200.0, views.width - 28.0, 22.0),
        "IPA FILES",
        11.0,
        true,
        gray,
    );
    () = msg![env; (views.scroll_view) addSubview:section];
    views.rows.push(section);

    if details.loading {
        add_message_at(env, views, 230.0, "Looking for public IPA files…", false);
        return 430.0;
    }
    if let Some(error) = &details.error {
        add_message_at(env, views, 226.0, error, true);
        return 470.0;
    }
    let Some(loaded) = loaded else {
        return 230.0;
    };
    if loaded.files.is_empty() {
        add_message_at(
            env,
            views,
            230.0,
            "This entry has no public IPA files. Choose another archive entry.",
            false,
        );
        return 430.0;
    }
    if details.matching_files.is_empty() {
        add_message_at(
            env,
            views,
            230.0,
            "No IPA files match this search. Try a shorter title or clear the search.",
            false,
        );
        return 430.0;
    }

    let page = details.file_page.min(details.page_count());
    let start = (page - 1) * DETAIL_ROWS;
    let end = (start + DETAIL_ROWS).min(details.matching_files.len());
    let blue = ui_color(env, 0.0, 0.40, 0.82, 1.0);
    let green = ui_color(env, 0.12, 0.58, 0.32, 1.0);
    let disabled = ui_color(env, 0.68, 0.73, 0.80, 1.0);
    for (row_index, file_index) in details.matching_files[start..end].iter().copied().enumerate() {
        let file = &loaded.files[file_index];
        let y = 228.0 + row_index as CGFloat * FILE_ROW_HEIGHT;
        let row = new_view(env, rect(8.0, y, row_width, 74.0), white);
        () = msg![env; row setUserInteractionEnabled:false];
        let row_layer: id = msg![env; row layer];
        () = msg![env; row_layer setCornerRadius:(10.0 as CGFloat)];
        () = msg![env; (views.scroll_view) addSubview:row];
        let file_title = new_label(
            env,
            rect(12.0, 7.0, row_width - 104.0, 36.0),
            display_file_name(&file.name),
            12.0,
            true,
            dark,
        );
        () = msg![env; file_title setNumberOfLines:2];
        () = msg![env; row addSubview:file_title];
        let subtitle = new_label(
            env,
            rect(12.0, 45.0, row_width - 104.0, 20.0),
            &file_subtitle(file),
            10.0,
            false,
            gray,
        );
        () = msg![env; row addSubview:subtitle];

        let key = download_key(&loaded.identifier, &file.name);
        let state = downloads.state_for_key(&key);
        let (title, enabled, color) = match state {
            Some(TaskState::Installed) => ("Installed", false, green),
            Some(TaskState::Downloading) => ("Active", false, disabled),
            Some(TaskState::Queued) => ("Queued", false, disabled),
            Some(TaskState::Failed | TaskState::Cancelled) => ("Retry", true, blue),
            None => ("GET", true, blue),
        };
        add_action_pill(
            env,
            row,
            rect(row_width - 80.0, 21.0, 68.0, 32.0),
            title,
            color,
            white,
            enabled,
        );
        views.rows.push(row);
    }
    236.0 + (end - start) as CGFloat * FILE_ROW_HEIGHT
}

fn render_downloads(
    env: &mut Environment,
    views: &mut StoreViews,
    downloads: &DownloadManager,
) -> CGFloat {
    if downloads.tasks.is_empty() {
        add_message(
            env,
            views,
            "Your queue and installed packages will appear here. You can queue several versions and return to Collections or All IPAs.",
            false,
        );
        return 220.0;
    }

    let row_width = views.width - 16.0;
    let white: id = msg_class![env; UIColor whiteColor];
    let dark: id = msg_class![env; UIColor blackColor];
    let gray = ui_color(env, 0.38, 0.43, 0.50, 1.0);
    let blue = ui_color(env, 0.0, 0.40, 0.82, 1.0);
    let red = ui_color(env, 0.82, 0.20, 0.24, 1.0);
    let green = ui_color(env, 0.12, 0.58, 0.32, 1.0);
    let track_color = ui_color(env, 0.84, 0.87, 0.91, 1.0);

    for (index, task) in downloads.tasks.iter().rev().enumerate() {
        let y = 4.0 + index as CGFloat * DOWNLOAD_ROW_HEIGHT;
        let row = new_view(env, rect(8.0, y, row_width, 104.0), white);
        () = msg![env; row setUserInteractionEnabled:false];
        let row_layer: id = msg![env; row layer];
        () = msg![env; row_layer setCornerRadius:(10.0 as CGFloat)];
        () = msg![env; (views.scroll_view) addSubview:row];
        let title_width = if task.state == TaskState::Installed {
            row_width - 24.0
        } else {
            row_width - 102.0
        };
        let title = new_label(
            env,
            rect(12.0, 7.0, title_width, 21.0),
            &task.title,
            13.0,
            true,
            dark,
        );
        () = msg![env; row addSubview:title];
        let file_label = new_label(
            env,
            rect(12.0, 29.0, title_width, 29.0),
            display_file_name(&task.file.name),
            10.0,
            false,
            gray,
        );
        () = msg![env; file_label setNumberOfLines:2];
        () = msg![env; row addSubview:file_label];
        let status_color = match task.state {
            TaskState::Installed => green,
            TaskState::Failed => red,
            _ => gray,
        };
        let status = new_label(
            env,
            rect(12.0, 59.0, row_width - 24.0, 29.0),
            &download_task_status(task),
            10.0,
            false,
            status_color,
        );
        () = msg![env; status setNumberOfLines:2];
        () = msg![env; row addSubview:status];

        if task.state != TaskState::Installed {
            let (title, background) = match task.state {
                TaskState::Failed | TaskState::Cancelled => ("Retry", blue),
                _ => ("Cancel", red),
            };
            add_action_pill(
                env,
                row,
                rect(row_width - 80.0, 12.0, 68.0, 30.0),
                title,
                background,
                white,
                true,
            );
        }

        let track = new_view(
            env,
            rect(12.0, 94.0, row_width - 24.0, 4.0),
            track_color,
        );
        () = msg![env; row addSubview:track];
        let fill_color = if task.state == TaskState::Installed {
            green
        } else {
            blue
        };
        let progress_width = row_width - 24.0;
        let fill = new_view(
            env,
            rect(12.0, 94.0, progress_width * task.fraction() as CGFloat, 4.0),
            fill_color,
        );
        () = msg![env; row addSubview:fill];
        views.download_progress.push(DownloadProgressViews {
            id: task.id,
            label: status,
            fill,
            width: progress_width,
        });
        views.rows.push(row);
    }
    8.0 + downloads.tasks.len() as CGFloat * DOWNLOAD_ROW_HEIGHT
}

fn update_download_progress(
    env: &mut Environment,
    views: &StoreViews,
    screen: Screen,
    catalog: &CatalogState,
    downloads: &DownloadManager,
) {
    for progress in &views.download_progress {
        let Some(task) = downloads.task(progress.id) else {
            continue;
        };
        set_label(env, progress.label, &download_task_status(task));
        let mut frame: CGRect = msg![env; (progress.fill) frame];
        frame.size.width = progress.width * task.fraction() as CGFloat;
        () = msg![env; (progress.fill) setFrame:frame];
    }
    update_nav_subtitle(env, views, screen, catalog, downloads);
}

fn download_task_status(task: &DownloadTask) -> String {
    match task.state {
        TaskState::Queued => "Waiting in queue".to_string(),
        TaskState::Downloading => {
            if task.cancel.load(Ordering::Acquire) {
                return "Cancelling…".to_string();
            }
            let mut text = if let Some(total) = task.total.filter(|total| *total > 0) {
                format!(
                    "{} / {} • {:.0}%",
                    format_size(task.downloaded),
                    format_size(total),
                    task.fraction() * 100.0
                )
            } else {
                format!("{} downloaded", format_size(task.downloaded))
            };
            if let Some(started) = task.started {
                let seconds = started.elapsed().as_secs_f64();
                if seconds >= 0.5 && task.downloaded > 0 {
                    let speed = (task.downloaded as f64 / seconds) as u64;
                    text.push_str(&format!(" • {}/s", format_size(speed)));
                }
            }
            text
        }
        TaskState::Installed => {
            let name = task
                .path
                .as_ref()
                .and_then(|path| path.file_name())
                .unwrap_or_default()
                .to_string_lossy();
            if task.already_present {
                format!("Already installed • {name}")
            } else {
                format!("Installed in touchHLE_apps • {name}")
            }
        }
        TaskState::Failed => task
            .error
            .as_deref()
            .map(|error| truncate_chars(error, 180))
            .unwrap_or_else(|| "Download failed. Tap Retry.".to_string()),
        TaskState::Cancelled => "Cancelled • tap Retry to start again".to_string(),
    }
}

fn add_message(env: &mut Environment, views: &mut StoreViews, text: &str, retry: bool) {
    add_message_at(env, views, 24.0, text, retry);
}

fn add_message_at(
    env: &mut Environment,
    views: &mut StoreViews,
    y: CGFloat,
    text: &str,
    retry: bool,
) {
    let gray = ui_color(env, 0.34, 0.39, 0.46, 1.0);
    let label = new_label(
        env,
        rect(20.0, y, views.width - 40.0, 86.0),
        text,
        13.0,
        false,
        gray,
    );
    () = msg![env; label setNumberOfLines:4];
    () = msg![env; label setTextAlignment:UITextAlignmentCenter];
    () = msg![env; (views.scroll_view) addSubview:label];
    views.rows.push(label);
    if retry {
        let blue = ui_color(env, 0.0, 0.40, 0.82, 1.0);
        let white: id = msg_class![env; UIColor whiteColor];
        let button = make_button(
            env,
            views.delegate,
            views.scroll_view,
            rect((views.width - 130.0) / 2.0, y + 98.0, 130.0, 36.0),
            "Retry",
            "archiveStoreRetry",
            0,
            blue,
            white,
        );
        views.rows.push(button);
    }
}

fn add_action_pill(
    env: &mut Environment,
    parent: id,
    frame: CGRect,
    title: &str,
    background: id,
    title_color: id,
    enabled: bool,
) {
    let pill = new_view(env, frame, background);
    () = msg![env; pill setUserInteractionEnabled:false];
    let layer: id = msg![env; pill layer];
    () = msg![env; layer setCornerRadius:(7.0 as CGFloat)];
    if !enabled {
        () = msg![env; pill setAlpha:(0.48 as CGFloat)];
    }
    let label = new_label(
        env,
        rect(0.0, 0.0, frame.size.width, frame.size.height),
        title,
        12.0,
        true,
        title_color,
    );
    () = msg![env; label setTextAlignment:UITextAlignmentCenter];
    () = msg![env; pill addSubview:label];
    () = msg![env; parent addSubview:pill];
}

fn add_item_icon(
    env: &mut Environment,
    views: &mut StoreViews,
    parent: id,
    identifier: &str,
    title: &str,
    frame: CGRect,
    artwork: &ArtworkCache,
) {
    let cached = artwork.images.get(identifier).copied();
    let image = if let Some(image) = cached {
        image
    } else {
        let palette = [
            (0.10, 0.40, 0.78, 1.0),
            (0.48, 0.24, 0.72, 1.0),
            (0.08, 0.56, 0.48, 1.0),
            (0.82, 0.36, 0.12, 1.0),
            (0.72, 0.20, 0.38, 1.0),
            (0.20, 0.32, 0.52, 1.0),
        ];
        let hash = identifier.bytes().fold(0u32, |hash, byte| {
            hash.wrapping_mul(33).wrapping_add(byte as u32)
        });
        let color = palette[hash as usize % palette.len()];
        make_icon_from_text(env, &item_initials(title), 22.0, 0.0, color)
    };
    let icon: id = msg_class![env; UIImageView alloc];
    let icon: id = msg![env; icon initWithImage:image];
    () = msg![env; icon setFrame:frame];
    () = msg![env; icon setUserInteractionEnabled:false];
    let layer: id = msg![env; icon layer];
    let gravity = ns_string::get_static_str(env, "resizeAspect");
    () = msg![env; layer setContentsGravity:gravity];
    () = msg![env; parent addSubview:icon];
    if cached.is_none() {
        views.artwork_views.push((identifier.to_string(), icon));
    }
}

fn item_initials(title: &str) -> String {
    let words: Vec<&str> = title.split_whitespace().take(2).collect();
    let mut initials = String::new();
    if words.len() >= 2 {
        for word in words {
            if let Some(character) = word.chars().find(|character| character.is_alphanumeric()) {
                initials.extend(character.to_uppercase());
            }
        }
    } else if let Some(word) = words.first() {
        for character in word.chars().filter(|character| character.is_alphanumeric()).take(2) {
            initials.extend(character.to_uppercase());
        }
    }
    if initials.is_empty() {
        "IPA".to_string()
    } else {
        initials
    }
}

fn catalog_subtitle(item: &CatalogItem) -> String {
    let mut parts = Vec::new();
    if let Some(files_count) = item.files_count {
        parts.push(format!("{files_count} files"));
    }
    if let Some(version) = &item.version {
        parts.push(format!("v{version}"));
    }
    if let Some(creator) = &item.creator {
        parts.push(creator.clone());
    }
    if let Some(date) = &item.date {
        parts.push(date.clone());
    }
    if parts.is_empty() {
        item.identifier.clone()
    } else {
        truncate_chars(&parts.join(" • "), 120)
    }
}

fn details_metadata(details: &ItemDetails) -> String {
    let mut parts = vec![details.identifier.clone()];
    if let Some(creator) = &details.creator {
        parts.push(creator.clone());
    }
    if let Some(version) = &details.version {
        parts.push(format!("Version {version}"));
    }
    if let Some(date) = &details.date {
        parts.push(date.clone());
    }
    truncate_chars(&parts.join(" • "), 180)
}

fn file_subtitle(file: &ArchiveFile) -> String {
    let mut parts = Vec::new();
    if let Some(version) = &file.version {
        parts.push(format!("Version {version}"));
    }
    if let Some(size) = file.size {
        parts.push(format_size(size));
    }
    if parts.is_empty() {
        "IPA package".to_string()
    } else {
        parts.join(" • ")
    }
}

fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut text: String = value.chars().take(limit.saturating_sub(1)).collect();
    text.push('…');
    text
}

fn request_catalog(
    sender: &Sender<StoreMessage>,
    generation: u64,
    scope: CatalogScope,
    query: String,
    page: usize,
    rows: usize,
    sort: CatalogSort,
    fuzzy: bool,
) {
    let worker_sender = sender.clone();
    let spawn = std::thread::Builder::new()
        .name("HyperHLE archive catalog".to_string())
        .spawn(move || {
            let result = network::fetch_catalog(scope, &query, page, rows, sort, fuzzy);
            let _ = worker_sender.send(StoreMessage::Catalog { generation, result });
        });
    if let Err(error) = spawn {
        let _ = sender.send(StoreMessage::Catalog {
            generation,
            result: Err(format!("Could not start the catalog request: {error}")),
        });
    }
}

fn request_artwork(sender: &Sender<StoreMessage>, identifier: String) {
    let worker_sender = sender.clone();
    let request_identifier = identifier.clone();
    let spawn = std::thread::Builder::new()
        .name("HyperHLE archive artwork".to_string())
        .spawn(move || {
            let result = network::fetch_artwork(&request_identifier);
            let _ = worker_sender.send(StoreMessage::Artwork { identifier, result });
        });
    if let Err(error) = spawn {
        let _ = sender.send(StoreMessage::Artwork {
            identifier: String::new(),
            result: Err(format!("Could not start the artwork request: {error}")),
        });
    }
}

fn request_details(sender: &Sender<StoreMessage>, generation: u64, identifier: String) {
    let worker_sender = sender.clone();
    let spawn = std::thread::Builder::new()
        .name("HyperHLE archive metadata".to_string())
        .spawn(move || {
            let result = network::fetch_details(&identifier);
            let _ = worker_sender.send(StoreMessage::Details { generation, result });
        });
    if let Err(error) = spawn {
        let _ = sender.send(StoreMessage::Details {
            generation,
            result: Err(format!("Could not start the metadata request: {error}")),
        });
    }
}

fn request_download(
    sender: &Sender<StoreMessage>,
    generation: u64,
    apps_dir: PathBuf,
    identifier: String,
    file: ArchiveFile,
    cancel: Arc<AtomicBool>,
) {
    let worker_sender = sender.clone();
    let progress_sender = sender.clone();
    let spawn = std::thread::Builder::new()
        .name("HyperHLE IPA download".to_string())
        .spawn(move || {
            let result = network::download_ipa(
                &apps_dir,
                &identifier,
                &file,
                &cancel,
                |downloaded, total| {
                    let _ = progress_sender.send(StoreMessage::Download {
                        generation,
                        event: DownloadEvent::Progress { downloaded, total },
                    });
                },
            );
            let _ = worker_sender.send(StoreMessage::Download {
                generation,
                event: DownloadEvent::Finished { result },
            });
        });
    if let Err(error) = spawn {
        let _ = sender.send(StoreMessage::Download {
            generation,
            event: DownloadEvent::Finished {
                result: Err(format!("Could not start the download worker: {error}")),
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file() -> ArchiveFile {
        ArchiveFile {
            name: "Game 1.0.ipa".to_string(),
            size: Some(1024),
            version: Some("1.0".to_string()),
        }
    }

    #[test]
    fn queue_deduplicates_and_retries_tasks() {
        let mut manager = DownloadManager::new();
        assert!(manager.enqueue("game", "Game", &file()));
        assert!(!manager.enqueue("game", "Game", &file()));
        assert_eq!(manager.pending_count(), 1);
        let id = manager.tasks[0].id;
        assert!(manager.toggle(id));
        assert_eq!(manager.tasks[0].state, TaskState::Cancelled);
        assert!(manager.toggle(id));
        assert_eq!(manager.tasks[0].state, TaskState::Queued);
        manager.cancel_all();
        assert_eq!(manager.tasks[0].state, TaskState::Cancelled);
        assert!(manager.retry_all());
        assert_eq!(manager.tasks[0].state, TaskState::Queued);
    }

    #[test]
    fn sorting_cycles_and_icons_have_stable_initials() {
        assert_eq!(CatalogSort::Popular.next(), CatalogSort::Newest);
        assert_eq!(CatalogSort::Newest.next(), CatalogSort::Oldest);
        assert_eq!(CatalogSort::Oldest.next(), CatalogSort::Title);
        assert_eq!(CatalogSort::Title.next(), CatalogSort::Popular);
        assert_eq!(item_initials("Temple Run"), "TR");
        assert_eq!(item_initials("Minecraft"), "MI");
        assert_eq!(item_initials(""), "IPA");
    }

    #[test]
    fn details_filter_and_pagination_keep_original_file_indices() {
        let mut state = DetailsState::new();
        state.details = Some(ItemDetails {
            identifier: "collection".to_string(),
            title: "Collection".to_string(),
            version: None,
            creator: None,
            date: None,
            description: None,
            files: (1..=45)
                .map(|index| ArchiveFile {
                    name: format!("Game {index}.ipa"),
                    size: None,
                    version: None,
                })
                .collect(),
        });
        state.rebuild_file_matches();
        assert_eq!(state.matching_files.len(), 45);
        assert_eq!(state.page_count(), 3);
        state.file_page = 2;
        assert_eq!(state.file_index(0), Some(20));
        state.set_file_query("game 45".to_string());
        assert_eq!(state.matching_files, vec![44]);
        assert_eq!(state.file_page, 1);
        assert_eq!(state.file_index(0), Some(44));
        assert_eq!(state.file_index(1), None);
    }
}
