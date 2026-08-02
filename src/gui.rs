use crate::{
    cache::{self, Applet, CachedPackage},
    extraction::{self, ExtractionMode, ExtractionSummary, ExtractionTarget},
    locator, platform,
};
use chrono::{Local, TimeZone};
use eframe::egui::{self, Align, Align2, Color32, FontFamily, Id, Layout, RichText, Sense, Vec2};
use lucide_icons::{Icon, LUCIDE_FONT_BYTES};
use pinyin::ToPinyin;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const APP_NAME: &str = "微信小程序缓存识别器";
const STORAGE_ROOT: &str = "last_cache_root";
const ACCENT: Color32 = Color32::from_rgb(37, 99, 235);
const ACCENT_DARK: Color32 = Color32::from_rgb(29, 78, 216);
const BORDER: Color32 = Color32::from_rgb(218, 225, 228);
const CONTROL_BORDER: Color32 = Color32::from_rgb(174, 187, 193);
const ROW_SELECTED: Color32 = Color32::from_rgb(239, 246, 255);
const TEXT_PRIMARY: Color32 = Color32::from_rgb(38, 47, 52);
const TEXT_SECONDARY: Color32 = Color32::from_rgb(96, 108, 114);
const SURFACE: Color32 = Color32::from_rgb(255, 255, 255);
const APP_BACKGROUND: Color32 = Color32::from_rgb(243, 246, 247);
const PROGRESS_FILL: Color32 = Color32::from_rgb(186, 230, 253);
const DEPENDENCY_BACKGROUND: Color32 = Color32::from_rgb(245, 243, 255);
const SUCCESS: Color32 = Color32::from_rgb(21, 128, 61);
const SUCCESS_BACKGROUND: Color32 = Color32::from_rgb(236, 253, 245);
const ICON_FONT: &str = "lucide-icons";
const NOTICE_DURATION: Duration = Duration::from_secs(5);

struct UiMetrics;

impl UiMetrics {
    const MIN_WINDOW_WIDTH: f32 = 1040.0;
    const CONTROL_HEIGHT: f32 = 36.0;
    const HEADER_HEIGHT: f32 = 42.0;
    const ROW_HEIGHT: f32 = 54.0;
    const COMPACT_ROW_HEIGHT: f32 = 58.0;
    const APP_ICON: f32 = 36.0;
    const CHECKBOX_PADDING: f32 = 10.0;
    const PAGE_PADDING: i8 = 16;
    const TOOLBAR_PADDING_X: i8 = 16;
    const TOOLBAR_PADDING_Y: i8 = 12;
    const FLOATING_ACTION_RIGHT_INSET: f32 = 44.0;
    // The table is inset by 32 px, so this corresponds to a 1160 px window.
    const COLUMN_BREAKPOINT: f32 = 1128.0;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SortColumn {
    Name,
    AppId,
    Version,
    PackageCount,
    Size,
    Created,
    Modified,
    Accessed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationKind {
    Locate,
    Scan,
    Extract,
}

struct Operation {
    id: u64,
    kind: OperationKind,
    completed: usize,
    total: usize,
    files: usize,
    cancelling: bool,
    cancel: Arc<AtomicBool>,
}

enum AppEvent {
    LocateFinished {
        id: u64,
        result: Vec<locator::CacheCandidate>,
    },
    ScanProgress {
        id: u64,
        completed: usize,
        total: usize,
    },
    ScanFinished {
        id: u64,
        result: Result<Vec<Applet>, String>,
    },
    ExtractProgress {
        id: u64,
        completed: usize,
        total: usize,
        files: usize,
    },
    ExtractFinished {
        id: u64,
        result: Result<ExtractionSummary, String>,
    },
}

struct Notice {
    message: String,
    error: bool,
    persistent: bool,
    created: Instant,
    output: Option<PathBuf>,
    details: Vec<String>,
}

#[derive(Clone, Copy)]
struct Columns {
    check: f32,
    icon: f32,
    name: f32,
    appid: f32,
    version: f32,
    count: f32,
    size: f32,
    date: f32,
    compact: bool,
}

impl Columns {
    fn new(width: f32) -> Self {
        let compact = width < UiMetrics::COLUMN_BREAKPOINT;
        let check = 44.0;
        let icon = 50.0;
        let size = 78.0;
        let date = if compact { 108.0 } else { 142.0 };
        let version = 68.0;
        let count = 54.0;
        let remaining = (width - check - icon - size - date * 3.0 - version - count).max(0.0);
        Self {
            check,
            icon,
            name: remaining * 0.42,
            appid: remaining * 0.58,
            version,
            count,
            size,
            date,
            compact,
        }
    }

    #[cfg(test)]
    fn total_width(self) -> f32 {
        self.check
            + self.icon
            + self.name
            + self.appid
            + self.version
            + self.count
            + self.size
            + self.date * 3.0
    }
}

pub struct InspectorApp {
    root: String,
    scanned: bool,
    rows: Vec<Applet>,
    query: String,
    selected: HashSet<String>,
    expanded: Option<String>,
    sort_column: SortColumn,
    ascending: bool,
    operation: Option<Operation>,
    next_operation: u64,
    notice: Option<Notice>,
    auto_located_candidates: Option<usize>,
    textures: HashMap<String, egui::TextureHandle>,
    plugin_packages: HashMap<String, CachedPackage>,
    tx: Sender<AppEvent>,
    rx: Receiver<AppEvent>,
}

impl InspectorApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        configure_fonts(&cc.egui_ctx);
        #[cfg(not(test))]
        let test_root = std::env::var("WXAPPLET_ROOT").ok();
        #[cfg(test)]
        let test_root: Option<String> = None;
        let root = test_root.clone().unwrap_or_else(|| {
            cc.storage
                .and_then(|storage| storage.get_string(STORAGE_ROOT))
                .unwrap_or_default()
        });
        let root = platform::display_path(Path::new(&root));
        let (tx, rx) = mpsc::channel();
        let app = Self {
            root,
            scanned: false,
            rows: Vec::new(),
            query: String::new(),
            selected: HashSet::new(),
            expanded: None,
            sort_column: SortColumn::Name,
            ascending: true,
            operation: None,
            next_operation: 0,
            notice: None,
            auto_located_candidates: None,
            textures: HashMap::new(),
            plugin_packages: HashMap::new(),
            tx,
            rx,
        };
        #[cfg(not(test))]
        {
            let mut app = app;
            let saved_root_is_valid = !app.root.trim().is_empty()
                && cache::inspect_cache_root(Path::new(app.root.trim())).is_some();
            if test_root.is_some() || saved_root_is_valid {
                app.start_scan();
            } else {
                app.root.clear();
                app.start_locate();
            }
            app
        }
        #[cfg(test)]
        {
            let _ = test_root;
            app
        }
    }

    fn notify(&mut self, message: impl Into<String>, error: bool, persistent: bool) {
        self.notice = Some(Notice {
            message: message.into(),
            error,
            persistent,
            created: Instant::now(),
            output: None,
            details: Vec::new(),
        });
    }

    fn next_id(&mut self) -> u64 {
        self.next_operation += 1;
        self.next_operation
    }

    fn start_locate(&mut self) {
        if self.operation.is_some() {
            return;
        }
        let id = self.next_id();
        self.operation = Some(Operation {
            id,
            kind: OperationKind::Locate,
            completed: 0,
            total: 0,
            files: 0,
            cancelling: false,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = locator::discover_cache_roots();
            let _ = tx.send(AppEvent::LocateFinished { id, result });
        });
    }

    fn start_scan(&mut self) {
        if self.operation.is_some() {
            return;
        }
        let root = self.root.trim().to_owned();
        if root.is_empty() {
            self.notify("请选择 Applet 缓存目录", true, true);
            return;
        }
        let id = self.next_id();
        let cancel = Arc::new(AtomicBool::new(false));
        self.operation = Some(Operation {
            id,
            kind: OperationKind::Scan,
            completed: 0,
            total: 0,
            files: 0,
            cancelling: false,
            cancel: cancel.clone(),
        });
        self.expanded = None;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = cache::scan_with_progress(Path::new(&root), |completed, total| {
                if cancel.load(AtomicOrdering::Relaxed) {
                    return Err("操作已取消".into());
                }
                let _ = progress_tx.send(AppEvent::ScanProgress {
                    id,
                    completed,
                    total,
                });
                Ok(())
            });
            let _ = tx.send(AppEvent::ScanFinished { id, result });
        });
    }

    fn choose_and_scan(&mut self) {
        if self.operation.is_some() {
            return;
        }
        let mut dialog = rfd::FileDialog::new().set_title("选择微信 Applet 缓存目录");
        if !self.root.trim().is_empty() {
            dialog = dialog.set_directory(self.root.trim());
        }
        if let Some(path) = dialog.pick_folder() {
            self.root = platform::display_path(&path);
            self.start_scan();
        }
    }

    fn start_extract(&mut self, mode: ExtractionMode) {
        if self.operation.is_some() {
            return;
        }
        let targets = self
            .rows
            .iter()
            .filter(|row| self.selected.contains(&row.appid) && !row.active_packages.is_empty())
            .map(ExtractionTarget::applet)
            .collect::<Vec<_>>();
        self.start_targets_extract(targets, mode, "选择解压输出目录");
    }

    fn start_plugin_extract(&mut self, appid: &str) {
        let Some(package) = self.plugin_packages.get(appid).cloned() else {
            self.notify("未找到该插件的缓存包", true, false);
            return;
        };
        self.start_targets_extract(
            vec![ExtractionTarget::plugin(appid.to_owned(), package)],
            ExtractionMode::Complete,
            "选择插件解压输出目录",
        );
    }

    fn start_targets_extract(
        &mut self,
        targets: Vec<ExtractionTarget>,
        mode: ExtractionMode,
        dialog_title: &str,
    ) {
        if self.operation.is_some() || targets.is_empty() {
            return;
        }
        let Some(output) = rfd::FileDialog::new().set_title(dialog_title).pick_folder() else {
            return;
        };

        let id = self.next_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let package_total = targets
            .iter()
            .map(|target| target.package_count(mode))
            .sum();
        self.operation = Some(Operation {
            id,
            kind: OperationKind::Extract,
            completed: 0,
            total: package_total,
            files: 0,
            cancelling: false,
            cancel: cancel.clone(),
        });
        let tx = self.tx.clone();
        thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = extraction::extract_many_with_progress(
                &targets,
                &output,
                mode,
                |completed, total, files| {
                    if cancel.load(AtomicOrdering::Relaxed) {
                        return Err("操作已取消".into());
                    }
                    let _ = progress_tx.send(AppEvent::ExtractProgress {
                        id,
                        completed,
                        total,
                        files,
                    });
                    Ok(())
                },
            );
            let _ = tx.send(AppEvent::ExtractFinished { id, result });
        });
    }

    fn cancel_operation(&mut self) {
        if let Some(operation) = &mut self.operation {
            operation.cancelling = true;
            operation.cancel.store(true, AtomicOrdering::Relaxed);
        }
    }

    fn poll_events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                AppEvent::LocateFinished { id, result } => {
                    if self.operation.as_ref().is_none_or(|value| value.id != id) {
                        continue;
                    }
                    self.operation = None;
                    if let Some(candidate) = result.first() {
                        self.root = platform::display_path(&candidate.path);
                        self.auto_located_candidates = Some(result.len());
                        self.start_scan();
                    } else {
                        self.notify(
                            "未自动找到缓存目录，请先在微信中打开一个小程序或手动选择目录",
                            true,
                            false,
                        );
                    }
                }
                AppEvent::ScanProgress {
                    id,
                    completed,
                    total,
                } => {
                    if let Some(operation) = self.operation.as_mut().filter(|value| value.id == id)
                    {
                        operation.completed = completed;
                        operation.total = total;
                    }
                }
                AppEvent::ScanFinished { id, result } => {
                    if self.operation.as_ref().is_none_or(|value| value.id != id) {
                        continue;
                    }
                    self.operation = None;
                    match result {
                        Ok(rows) => {
                            self.rows = rows;
                            self.scanned = true;
                            self.selected.clear();
                            self.textures.clear();
                            self.plugin_packages.clear();
                            let dependencies = self
                                .rows
                                .iter()
                                .flat_map(dependency_ids)
                                .collect::<HashSet<_>>();
                            let root = Path::new(self.root.trim());
                            for appid in dependencies {
                                if let Some(package) = cache::latest_plugin_package(root, &appid) {
                                    self.plugin_packages.insert(appid, package);
                                }
                            }
                            let message = match self.auto_located_candidates.take() {
                                Some(count) if count > 1 => format!(
                                    "已从 {count} 个缓存目录中选择最近使用目录，扫描到 {} 个小程序",
                                    self.rows.len()
                                ),
                                Some(_) => format!("已自动定位并扫描 {} 个小程序", self.rows.len()),
                                None => format!("已扫描 {} 个小程序", self.rows.len()),
                            };
                            self.notify(message, false, false);
                        }
                        Err(error) if error.contains("操作已取消") => {
                            self.notify("已取消扫描", false, false);
                        }
                        Err(error) => self.notify(format!("扫描失败：{error}"), true, true),
                    }
                }
                AppEvent::ExtractProgress {
                    id,
                    completed,
                    total,
                    files,
                } => {
                    if let Some(operation) = self.operation.as_mut().filter(|value| value.id == id)
                    {
                        operation.completed = completed;
                        operation.total = total;
                        operation.files = files;
                    }
                }
                AppEvent::ExtractFinished { id, result } => {
                    if self.operation.as_ref().is_none_or(|value| value.id != id) {
                        continue;
                    }
                    self.operation = None;
                    match result {
                        Ok(summary) => {
                            let failures = summary.failures.len();
                            let mut notice = Notice {
                                message: if failures == 0 {
                                    extraction_success_message(&summary)
                                } else {
                                    format!(
                                        "已解压 {} 个包，{} 个失败",
                                        summary.package_count, failures,
                                    )
                                },
                                error: failures > 0,
                                persistent: failures > 0,
                                created: Instant::now(),
                                output: Some(PathBuf::from(summary.output)),
                                details: summary
                                    .failures
                                    .into_iter()
                                    .map(|failure| format!("{}：{}", failure.appid, failure.error))
                                    .collect(),
                            };
                            if notice.details.len() > 8 {
                                notice.details.truncate(8);
                            }
                            self.notice = Some(notice);
                        }
                        Err(error) if error.contains("操作已取消") => {
                            self.notify("已取消解压", false, false);
                        }
                        Err(error) => self.notify(format!("解压失败：{error}"), true, true),
                    }
                }
            }
        }
        if self.operation.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn visible_indices(&self) -> Vec<usize> {
        let query = self.query.trim().to_lowercase();
        let mut indices = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches_query(row, &query))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        indices.sort_by(|left, right| {
            let order = compare_rows(&self.rows[*left], &self.rows[*right], self.sort_column);
            if self.ascending {
                order
            } else {
                order.reverse()
            }
        });
        indices
    }

    fn open_package(&mut self, appid: &str) {
        if !cache::is_appid(appid) {
            self.notify("无效 AppID", true, true);
            return;
        }
        let path = cache::package_directory(Path::new(self.root.trim()), appid).or_else(|| {
            self.rows
                .iter()
                .find(|row| row.appid == appid)
                .map(|row| PathBuf::from(&row.package_dir))
        });
        let Some(path) = path else {
            self.notify("未找到该小程序的缓存目录", true, true);
            return;
        };
        if let Err(error) = platform::open_path(&path) {
            self.notify(error, true, true);
        }
    }

    fn texture_for(&mut self, ctx: &egui::Context, row: &Applet) -> Option<egui::TextureId> {
        if let Some(texture) = self.textures.get(&row.appid) {
            return Some(texture.id());
        }
        if row.icon_path.is_empty() {
            return None;
        }
        let bytes = fs::read(&row.icon_path).ok()?;
        let image = image::load_from_memory(&bytes).ok()?.into_rgba8();
        let size = [image.width() as usize, image.height() as usize];
        let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
        let texture = ctx.load_texture(&row.appid, color, egui::TextureOptions::LINEAR);
        let id = texture.id();
        self.textures.insert(row.appid.clone(), texture);
        Some(id)
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let operation_info = self.operation.as_ref().map(|operation| {
            (
                operation.kind,
                operation.completed,
                operation.total,
                operation.files,
                operation.cancelling,
            )
        });
        let toolbar_width = ui.available_width();
        egui::Frame::new()
            .fill(SURFACE)
            .inner_margin(egui::Margin::symmetric(
                UiMetrics::TOOLBAR_PADDING_X,
                UiMetrics::TOOLBAR_PADDING_Y,
            ))
            .stroke(egui::Stroke::new(1.0, BORDER))
            .show(ui, |ui| {
                ui.set_min_width(toolbar_width - f32::from(UiMetrics::TOOLBAR_PADDING_X) * 2.0);
                ui.horizontal(|ui| {
                    let spacing = ui.spacing().item_spacing.x;
                    let available = ui.available_width();
                    if let Some((kind, completed, operation_total, files, cancelling)) =
                        operation_info
                    {
                        let progress_width = (available * 0.3).clamp(260.0, 420.0);
                        let cancel_width = if kind == OperationKind::Locate {
                            0.0
                        } else {
                            36.0
                        };
                        let operation_gaps = if kind == OperationKind::Locate {
                            4.0
                        } else {
                            5.0
                        };
                        let path_width = (available
                            - 108.0
                            - cancel_width
                            - progress_width
                            - spacing * operation_gaps)
                            .max(320.0);
                        self.path_controls_with_width(ui, path_width);
                        let total = operation_total.max(1);
                        let progress = completed as f32 / total as f32;
                        let label = match kind {
                            OperationKind::Locate => "正在定位微信缓存".to_owned(),
                            OperationKind::Scan => format!(
                                "正在扫描 {}/{}",
                                completed,
                                if operation_total == 0 {
                                    "-".into()
                                } else {
                                    operation_total.to_string()
                                }
                            ),
                            OperationKind::Extract => format!(
                                "正在解压 {}/{}，{} 个文件",
                                completed, operation_total, files
                            ),
                        };
                        ui.add(
                            egui::ProgressBar::new(progress)
                                .text(RichText::new(label).color(TEXT_PRIMARY))
                                .fill(PROGRESS_FILL)
                                .animate(kind == OperationKind::Locate)
                                .desired_height(UiMetrics::CONTROL_HEIGHT)
                                .desired_width(progress_width),
                        );
                        if kind != OperationKind::Locate
                            && ui
                                .add_enabled(!cancelling, standard_icon_button(Icon::CircleX))
                                .on_hover_text("取消")
                                .clicked()
                        {
                            self.cancel_operation();
                        }
                    } else {
                        let filter_width = if available < 1100.0 { 240.0 } else { 270.0 };
                        let path_width =
                            (available - 108.0 - filter_width - spacing * 4.0).max(320.0);
                        self.path_controls_with_width(ui, path_width);
                        self.filter_controls(ui, filter_width);
                    }
                });
            });
    }

    fn path_controls_with_width(&mut self, ui: &mut egui::Ui, path_width: f32) {
        let idle = self.operation.is_none();
        let path_response = ui
            .add_enabled_ui(idle, |ui| {
                single_line_input(ui, &mut self.root, path_width, "微信 Applet 缓存目录")
            })
            .inner;
        let enter_pressed = ui.input(|input| input.key_pressed(egui::Key::Enter));
        if enter_pressed && (path_response.has_focus() || path_response.lost_focus()) {
            self.start_scan();
        }
        if ui
            .add_enabled(idle, standard_icon_button(Icon::FolderOpen))
            .on_hover_text("选择目录")
            .clicked()
        {
            self.choose_and_scan();
        }
        if ui
            .add_enabled(idle, standard_icon_button(Icon::LocateFixed))
            .on_hover_text("自动定位")
            .clicked()
        {
            self.start_locate();
        }
        if ui
            .add_enabled(
                idle && !self.root.trim().is_empty(),
                primary_icon_button(if self.scanned {
                    Icon::RefreshCw
                } else {
                    Icon::Scan
                }),
            )
            .on_hover_text(if self.scanned {
                "重新扫描"
            } else {
                "扫描"
            })
            .clicked()
        {
            self.start_scan();
        }
    }

    fn filter_controls(&mut self, ui: &mut egui::Ui, search_width: f32) {
        let search = ui
            .add_enabled_ui(self.scanned, |ui| {
                single_line_input(ui, &mut self.query, search_width, "按名称或 AppID 筛选")
            })
            .inner;
        if search.changed() {
            self.expanded = None;
        }
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        ui.with_layout(Layout::top_down(Align::Center), |ui| {
            ui.add_space(110.0);
            let locating = self
                .operation
                .as_ref()
                .is_some_and(|operation| operation.kind == OperationKind::Locate);
            let message = if locating {
                "正在定位微信缓存"
            } else if !self.scanned {
                "尚未扫描缓存目录"
            } else if self.rows.is_empty() {
                "该目录中没有可识别的小程序"
            } else {
                "没有符合当前筛选条件的小程序"
            };
            ui.label(
                RichText::new(message)
                    .size(16.0)
                    .color(Color32::from_gray(100)),
            );
            ui.add_space(10.0);
            if !self.scanned && self.operation.is_none() {
                let action_width = UiMetrics::CONTROL_HEIGHT * 2.0 + ui.spacing().item_spacing.x;
                ui.allocate_ui_with_layout(
                    Vec2::new(action_width, UiMetrics::CONTROL_HEIGHT),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        if ui
                            .add(standard_icon_button(Icon::LocateFixed))
                            .on_hover_text("自动定位")
                            .clicked()
                        {
                            self.start_locate();
                        }
                        if ui
                            .add(standard_icon_button(Icon::FolderOpen))
                            .on_hover_text("选择目录")
                            .clicked()
                        {
                            self.choose_and_scan();
                        }
                    },
                );
            } else if !self.query.is_empty()
                && !self.rows.is_empty()
                && ui
                    .add(standard_icon_button(Icon::X))
                    .on_hover_text("清除筛选")
                    .clicked()
            {
                self.query.clear();
            }
        });
    }

    fn table(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.spacing_mut().item_spacing.y = 0.0;
        let indices = self.visible_indices();
        if indices.is_empty() {
            self.empty_state(ui);
            return;
        }
        let columns = Columns::new(ui.available_width());
        self.table_header(ui, columns, &indices);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for index in indices {
                    let row = self.rows[index].clone();
                    self.table_row(ui, ctx, columns, &row);
                    if self.expanded.as_deref() == Some(&row.appid) {
                        self.dependency_row(ui, &row);
                    }
                }
            });
    }

    fn table_header(&mut self, ui: &mut egui::Ui, columns: Columns, indices: &[usize]) {
        let width = ui.available_width();
        egui::Frame::new()
            .fill(Color32::from_rgb(247, 249, 249))
            .stroke(egui::Stroke::new(1.0, BORDER))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(width, UiMetrics::HEADER_HEIGHT),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let all_selected = indices
                            .iter()
                            .all(|index| self.selected.contains(&self.rows[*index].appid));
                        let mut select_all = all_selected;
                        cell(ui, columns.check, |ui| {
                            ui.add_space(UiMetrics::CHECKBOX_PADDING);
                            if ui.checkbox(&mut select_all, "").changed() {
                                for index in indices {
                                    let appid = &self.rows[*index].appid;
                                    if select_all {
                                        self.selected.insert(appid.clone());
                                    } else {
                                        self.selected.remove(appid);
                                    }
                                }
                            }
                        });
                        cell(ui, columns.icon, |ui| {
                            ui.label(RichText::new("图标").size(13.0).color(TEXT_SECONDARY));
                        });
                        self.sort_cell(ui, columns.name, "名称", SortColumn::Name);
                        self.sort_cell(ui, columns.appid, "AppID", SortColumn::AppId);
                        self.sort_cell(ui, columns.version, "版本", SortColumn::Version);
                        self.sort_cell(ui, columns.count, "包数", SortColumn::PackageCount);
                        self.sort_cell(ui, columns.size, "大小", SortColumn::Size);
                        self.sort_cell(ui, columns.date, "创建日期", SortColumn::Created);
                        self.sort_cell(ui, columns.date, "修改日期", SortColumn::Modified);
                        self.sort_cell(ui, columns.date, "访问日期", SortColumn::Accessed);
                    },
                );
            });
    }

    fn sort_cell(&mut self, ui: &mut egui::Ui, width: f32, label: &str, column: SortColumn) {
        cell(ui, width, |ui| {
            let marker = if self.sort_column == column {
                if self.ascending {
                    " ↑"
                } else {
                    " ↓"
                }
            } else {
                ""
            };
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("{label}{marker}"))
                            .size(14.0)
                            .color(TEXT_PRIMARY),
                    )
                    .frame(false),
                )
                .clicked()
            {
                if self.sort_column == column {
                    self.ascending = !self.ascending;
                } else {
                    self.sort_column = column;
                    self.ascending = true;
                }
            }
        });
    }

    fn table_row(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        columns: Columns,
        row: &Applet,
    ) {
        let selected = self.selected.contains(&row.appid);
        let texture = self.texture_for(ctx, row);
        let fill = if selected {
            ROW_SELECTED
        } else {
            Color32::WHITE
        };
        let width = ui.available_width();
        let row_height = if columns.compact {
            UiMetrics::COMPACT_ROW_HEIGHT
        } else {
            UiMetrics::ROW_HEIGHT
        };
        egui::Frame::new()
            .fill(fill)
            .stroke(egui::Stroke::new(0.5, BORDER))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(width, row_height),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let mut checked = selected;
                        cell(ui, columns.check, |ui| {
                            ui.add_space(UiMetrics::CHECKBOX_PADDING);
                            if ui.checkbox(&mut checked, "").changed() {
                                if checked {
                                    self.selected.insert(row.appid.clone());
                                } else {
                                    self.selected.remove(&row.appid);
                                }
                            }
                        });
                        cell(ui, columns.icon, |ui| {
                            let clicked = package_icon(ui, texture, &row.name)
                                .on_hover_text("打开小程序包目录")
                                .clicked();
                            if clicked {
                                self.open_package(&row.appid);
                            }
                        });
                        cell(ui, columns.name, |ui| {
                            ui.horizontal_centered(|ui| {
                                let dependencies = dependency_ids(row);
                                if dependencies.is_empty() {
                                    ui.add_space(22.0);
                                } else {
                                    let expanded = self.expanded.as_deref() == Some(&row.appid);
                                    if disclosure_button(ui, expanded)
                                        .on_hover_text("展开依赖")
                                        .clicked()
                                    {
                                        self.expanded = if expanded {
                                            None
                                        } else {
                                            Some(row.appid.clone())
                                        };
                                    }
                                }
                                let response = ui
                                    .add(
                                        egui::Label::new(
                                            RichText::new(&row.name)
                                                .size(14.0)
                                                .color(TEXT_PRIMARY)
                                                .strong(),
                                        )
                                        .truncate()
                                        .sense(Sense::click()),
                                    )
                                    .on_hover_text("点击复制名称");
                                if response.clicked() {
                                    ctx.copy_text(row.name.clone());
                                    self.notify("已复制小程序名称", false, false);
                                }
                            });
                        });
                        cell(ui, columns.appid, |ui| {
                            let response = ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(&row.appid).size(14.0).color(TEXT_SECONDARY),
                                    )
                                    .truncate()
                                    .sense(Sense::click()),
                                )
                                .on_hover_text("点击复制 AppID");
                            if response.clicked() {
                                ctx.copy_text(row.appid.clone());
                                self.notify(format!("已复制 {}", row.appid), false, false);
                            }
                        });
                        cell(ui, columns.version, |ui| {
                            ui.label(RichText::new(&row.version).size(14.0).color(TEXT_PRIMARY));
                        });
                        cell(ui, columns.count, |ui| {
                            ui.label(
                                RichText::new(row.package_count.to_string())
                                    .size(14.0)
                                    .color(TEXT_PRIMARY),
                            );
                        });
                        cell(ui, columns.size, |ui| {
                            ui.label(
                                RichText::new(&row.package_size)
                                    .size(14.0)
                                    .color(TEXT_PRIMARY),
                            );
                        });
                        cell(ui, columns.date, |ui| {
                            date_cell(ui, row.created_at, columns.compact)
                        });
                        cell(ui, columns.date, |ui| {
                            date_cell(ui, row.modified_at, columns.compact)
                        });
                        cell(ui, columns.date, |ui| {
                            date_cell(ui, row.accessed_at, columns.compact)
                        });
                    },
                );
            });
    }

    fn dependency_row(&mut self, ui: &mut egui::Ui, row: &Applet) {
        let dependencies = dependency_ids(row);
        let width = ui.available_width();
        let horizontal_margin = 86.0 * 2.0;
        egui::Frame::new()
            .fill(DEPENDENCY_BACKGROUND)
            .inner_margin(egui::Margin::symmetric(86, 12))
            .stroke(egui::Stroke::new(0.5, BORDER))
            .show(ui, |ui| {
                let content_width = (width - horizontal_margin).max(240.0);
                ui.set_width(content_width);
                ui.set_max_width(content_width);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(10.0, 8.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(62.0, UiMetrics::CONTROL_HEIGHT),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(format!("插件 {}", dependencies.len()))
                                    .size(14.0)
                                    .color(TEXT_SECONDARY)
                                    .strong(),
                            );
                        },
                    );
                    for dependency in dependencies {
                        ui.allocate_ui_with_layout(
                            Vec2::new(260.0, UiMetrics::CONTROL_HEIGHT),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                if ui
                                    .add(icon_text_button(
                                        Icon::FolderOpen,
                                        dependency.clone(),
                                        false,
                                    ))
                                    .on_hover_text("打开插件包目录")
                                    .clicked()
                                {
                                    self.open_package(&dependency);
                                }
                                let available = self.plugin_packages.contains_key(&dependency);
                                if ui
                                    .add_enabled(available, standard_icon_button(Icon::PackageOpen))
                                    .on_hover_text(if available {
                                        "解压插件包"
                                    } else {
                                        "未发现插件缓存包"
                                    })
                                    .clicked()
                                {
                                    self.start_plugin_extract(&dependency);
                                }
                            },
                        );
                    }
                });
            });
    }

    fn floating_controls(&mut self, ctx: &egui::Context) {
        if !self.selected.is_empty() {
            egui::Area::new(Id::new("bulk-actions"))
                .anchor(
                    Align2::RIGHT_BOTTOM,
                    Vec2::new(-UiMetrics::FLOATING_ACTION_RIGHT_INSET, -20.0),
                )
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .fill(Color32::WHITE)
                        .show(ui, |ui| {
                            ui.allocate_ui_with_layout(
                                Vec2::new(118.0, UiMetrics::CONTROL_HEIGHT),
                                Layout::left_to_right(Align::Center),
                                |ui| {
                                    ui.spacing_mut().item_spacing.x = 3.0;
                                    if ui
                                        .add(plain_icon_button(Icon::X))
                                        .on_hover_text("清除选择")
                                        .clicked()
                                    {
                                        self.selected.clear();
                                    }
                                    if badged_primary_icon_button(
                                        ui,
                                        Icon::ArchiveRestore,
                                        self.selected.len(),
                                        self.operation.is_none(),
                                    )
                                    .on_hover_text(format!(
                                        "完整解压已选 {} 个小程序",
                                        self.selected.len()
                                    ))
                                    .clicked()
                                    {
                                        self.start_extract(ExtractionMode::Complete);
                                    }
                                    let mut requested_mode = None;
                                    ui.scope(|ui| {
                                        ui.spacing_mut().interact_size =
                                            Vec2::new(28.0, UiMetrics::CONTROL_HEIGHT);
                                        let visuals = &mut ui.style_mut().visuals.widgets;
                                        visuals.inactive.bg_fill = ACCENT;
                                        visuals.inactive.fg_stroke.color = Color32::WHITE;
                                        visuals.hovered.bg_fill = ACCENT_DARK;
                                        visuals.hovered.fg_stroke.color = Color32::WHITE;
                                        visuals.active.bg_fill = ACCENT_DARK;
                                        visuals.active.fg_stroke.color = Color32::WHITE;
                                        let icon = RichText::new(char::from(Icon::ChevronDown))
                                            .font(egui::FontId::new(
                                                17.0,
                                                FontFamily::Name(ICON_FONT.into()),
                                            ))
                                            .color(Color32::WHITE);
                                        ui.menu_button(icon, |ui| {
                                            if ui.button("完整小程序（主包 + 分包）").clicked()
                                            {
                                                requested_mode = Some(ExtractionMode::Complete);
                                                ui.close();
                                            }
                                            if ui.button("仅主包").clicked() {
                                                requested_mode = Some(ExtractionMode::MainOnly);
                                                ui.close();
                                            }
                                        })
                                        .response
                                        .on_hover_text("选择解压方式");
                                    });
                                    if let Some(mode) = requested_mode {
                                        self.start_extract(mode);
                                    }
                                },
                            );
                        });
                });
        }

        if let Some(notice) = &self.notice {
            if !notice.persistent {
                let lifetime = NOTICE_DURATION;
                let elapsed = notice.created.elapsed();
                if elapsed >= lifetime {
                    self.notice = None;
                } else {
                    ctx.request_repaint_after(lifetime - elapsed);
                }
            }
        }
        if self.notice.is_some() {
            egui::Area::new(Id::new("notice"))
                .anchor(Align2::LEFT_BOTTOM, Vec2::new(20.0, -20.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let (error, message, output, details) = {
                        let notice = self.notice.as_ref().expect("notice exists");
                        (
                            notice.error,
                            notice.message.clone(),
                            notice.output.clone(),
                            notice.details.clone(),
                        )
                    };
                    let fill = if error {
                        Color32::from_rgb(255, 243, 241)
                    } else {
                        SUCCESS_BACKGROUND
                    };
                    egui::Frame::popup(ui.style()).fill(fill).show(ui, |ui| {
                        let measure = |value: &str| {
                            ui.painter()
                                .layout_no_wrap(
                                    value.to_owned(),
                                    egui::FontId::proportional(14.0),
                                    TEXT_PRIMARY,
                                )
                                .size()
                                .x
                        };
                        let content_width = details
                            .iter()
                            .map(|detail| measure(detail))
                            .fold(measure(&message), f32::max);
                        let notice_width = toast_width(
                            content_width,
                            output.is_some(),
                            ui.spacing().item_spacing.x,
                        );
                        ui.set_max_width(notice_width);
                        ui.allocate_ui_with_layout(
                            Vec2::new(notice_width, UiMetrics::CONTROL_HEIGHT),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                let (status_icon, status_color, status_text) = if error {
                                    (Icon::CircleX, Color32::from_rgb(190, 52, 45), "错误")
                                } else {
                                    (Icon::CheckCircle2, SUCCESS, "成功")
                                };
                                status_icon_view(ui, status_icon, status_color)
                                    .on_hover_text(status_text);
                                let output_width = if output.is_some() {
                                    UiMetrics::CONTROL_HEIGHT + ui.spacing().item_spacing.x
                                } else {
                                    0.0
                                };
                                let message_width = (ui.available_width()
                                    - 28.0
                                    - ui.spacing().item_spacing.x
                                    - output_width)
                                    .max(80.0);
                                ui.allocate_ui_with_layout(
                                    Vec2::new(message_width, UiMetrics::CONTROL_HEIGHT),
                                    Layout::left_to_right(Align::Center),
                                    |ui| {
                                        ui.set_max_width(message_width);
                                        ui.add(egui::Label::new(message.clone()).truncate())
                                            .on_hover_text(message);
                                    },
                                );
                                if let Some(output) = &output {
                                    if ui
                                        .add(standard_icon_button(Icon::FolderOpen))
                                        .on_hover_text("打开输出目录")
                                        .clicked()
                                    {
                                        if let Err(error) = platform::open_path(output) {
                                            self.notify(error, true, true);
                                        }
                                    }
                                }
                                if ui
                                    .add(plain_icon_button(Icon::X))
                                    .on_hover_text("关闭")
                                    .clicked()
                                {
                                    self.notice = None;
                                }
                            },
                        );
                        for detail in details {
                            ui.small(detail);
                        }
                    });
                });
        }
    }
}

impl eframe::App for InspectorApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string(STORAGE_ROOT, self.root.clone());
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_events(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ui.spacing_mut().item_spacing.y = 0.0;
        self.toolbar(ui);
        let remaining = ui.available_size();
        ui.allocate_ui_with_layout(remaining, Layout::top_down(Align::Min), |ui| {
            egui::Frame::new()
                .fill(APP_BACKGROUND)
                .inner_margin(egui::Margin::same(UiMetrics::PAGE_PADDING))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.set_min_height(ui.available_height());
                    self.table(ui, &ctx);
                });
        });
        self.floating_controls(&ctx);
    }
}

pub fn run() -> eframe::Result {
    let icon = load_window_icon();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_NAME)
            .with_app_id("wxapplet-inspector")
            .with_inner_size([1240.0, 780.0])
            .with_min_inner_size([UiMetrics::MIN_WINDOW_WIDTH, 560.0])
            .with_icon(Arc::new(icon)),
        renderer: eframe::Renderer::Glow,
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| Ok(Box::new(InspectorApp::new(cc)))),
    )
}

fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let height = ui.available_height().max(1.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add(&mut child);
}

fn toast_width(content_width: f32, has_output: bool, spacing: f32) -> f32 {
    let status_width = 20.0 + spacing;
    let output_width = if has_output {
        UiMetrics::CONTROL_HEIGHT + spacing
    } else {
        0.0
    };
    let close_width = 28.0 + spacing;
    (content_width + status_width + output_width + close_width + 12.0).clamp(180.0, 440.0)
}

fn icon_text(icon: Icon, color: Color32) -> RichText {
    RichText::new(char::from(icon).to_string())
        .font(egui::FontId::new(17.0, FontFamily::Name(ICON_FONT.into())))
        .color(color)
}

fn icon_button(icon: Icon) -> egui::Button<'static> {
    egui::Button::new(icon_text(icon, TEXT_PRIMARY))
}

fn disclosure_button(ui: &mut egui::Ui, expanded: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(24.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            if expanded {
                "收起依赖"
            } else {
                "展开依赖"
            },
        )
    });
    if ui.is_rect_visible(rect) {
        let center = rect.center();
        let points = if expanded {
            vec![
                center + Vec2::new(-4.0, -2.0),
                center + Vec2::new(0.0, 2.0),
                center + Vec2::new(4.0, -2.0),
            ]
        } else {
            vec![
                center + Vec2::new(-2.0, -4.0),
                center + Vec2::new(2.0, 0.0),
                center + Vec2::new(-2.0, 4.0),
            ]
        };
        let color = if response.hovered() {
            ACCENT_DARK
        } else {
            TEXT_SECONDARY
        };
        ui.painter()
            .add(egui::Shape::line(points, egui::Stroke::new(1.8, color)));
    }
    response
}

fn standard_icon_button(icon: Icon) -> egui::Button<'static> {
    icon_button(icon).min_size(Vec2::splat(UiMetrics::CONTROL_HEIGHT))
}

fn plain_icon_button(icon: Icon) -> egui::Button<'static> {
    icon_button(icon)
        .frame(false)
        .min_size(Vec2::new(28.0, UiMetrics::CONTROL_HEIGHT))
}

fn status_icon_view(ui: &mut egui::Ui, icon: Icon, color: Color32) -> egui::Response {
    let label = char::from(icon).to_string();
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(20.0, UiMetrics::CONTROL_HEIGHT), Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), label.clone())
    });
    if ui.is_rect_visible(rect) {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            label,
            egui::FontId::new(17.0, FontFamily::Name(ICON_FONT.into())),
            color,
        );
    }
    response
}

fn primary_icon_button(icon: Icon) -> egui::Button<'static> {
    egui::Button::new(icon_text(icon, Color32::WHITE))
        .fill(ACCENT)
        .stroke(egui::Stroke::new(1.0, ACCENT_DARK))
        .min_size(Vec2::splat(UiMetrics::CONTROL_HEIGHT))
}

fn badged_primary_icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    count: usize,
    enabled: bool,
) -> egui::Response {
    let response = ui.add_enabled(
        enabled,
        primary_icon_button(icon).min_size(Vec2::new(46.0, UiMetrics::CONTROL_HEIGHT)),
    );
    if count > 0 && ui.is_rect_visible(response.rect) {
        let label = if count > 99 {
            "99+".to_owned()
        } else {
            count.to_string()
        };
        let width = if label.len() > 2 { 24.0 } else { 17.0 };
        let badge = egui::Rect::from_min_size(
            response.rect.right_top() + Vec2::new(-width - 2.0, 2.0),
            Vec2::new(width, 16.0),
        );
        ui.painter().rect_filled(
            badge,
            egui::CornerRadius::same(8),
            Color32::from_rgb(205, 55, 50),
        );
        ui.painter().text(
            badge.center(),
            Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(10.0),
            Color32::WHITE,
        );
    }
    response
}

fn icon_text_button(icon: Icon, label: impl Into<String>, primary: bool) -> egui::Button<'static> {
    let color = if primary {
        Color32::WHITE
    } else {
        TEXT_PRIMARY
    };
    let mut text = egui::text::LayoutJob::default();
    text.append(
        &char::from(icon).to_string(),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::new(17.0, FontFamily::Name(ICON_FONT.into())),
            color,
            valign: Align::Center,
            ..Default::default()
        },
    );
    text.append(
        &format!("  {}", label.into()),
        0.0,
        egui::TextFormat {
            font_id: egui::FontId::proportional(14.0),
            color,
            valign: Align::Center,
            ..Default::default()
        },
    );
    let button = egui::Button::new(text).min_size(Vec2::new(0.0, UiMetrics::CONTROL_HEIGHT));
    if primary {
        button
            .fill(ACCENT)
            .stroke(egui::Stroke::new(1.0, ACCENT_DARK))
    } else {
        button
    }
}

fn single_line_input(
    ui: &mut egui::Ui,
    text: &mut String,
    width: f32,
    hint: &str,
) -> egui::Response {
    let frame = egui::Frame::new()
        .fill(SURFACE)
        .stroke(egui::Stroke::new(1.0, CONTROL_BORDER))
        .corner_radius(5.0)
        .inner_margin(egui::Margin::symmetric(10, 7));
    let response = ui.add_sized(
        [width, UiMetrics::CONTROL_HEIGHT],
        egui::TextEdit::singleline(text)
            .hint_text(hint)
            .vertical_align(Align::Center)
            .frame(frame),
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            5.0,
            egui::Stroke::new(1.5, ACCENT_DARK),
            egui::StrokeKind::Inside,
        );
    }
    response
}

fn package_icon(ui: &mut egui::Ui, texture: Option<egui::TextureId>, name: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(UiMetrics::APP_ICON), Sense::click());
    let radius = UiMetrics::APP_ICON / 2.0;
    ui.painter().rect(
        rect,
        radius,
        Color32::from_rgb(248, 250, 250),
        egui::Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );
    if let Some(texture) = texture {
        egui::Image::new((texture, rect.shrink(1.0).size()))
            .fit_to_exact_size(rect.shrink(1.0).size())
            .corner_radius(radius - 1.0)
            .alt_text(format!("打开 {name} 的包目录"))
            .paint_at(ui, rect.shrink(1.0));
    } else {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "包",
            egui::FontId::proportional(14.0),
            TEXT_SECONDARY,
        );
    }
    response
}

fn dependency_ids(row: &Applet) -> Vec<String> {
    row.depends_on
        .split('、')
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn extraction_success_message(summary: &ExtractionSummary) -> String {
    if summary.plugin_count > 0 && summary.applet_count == 0 {
        format!(
            "已解压 {} 个插件，共 {} 个包、{} 个文件",
            summary.plugin_count, summary.package_count, summary.file_count
        )
    } else {
        format!(
            "已解压 {} 个小程序，共 {} 个包、{} 个文件",
            summary.applet_count, summary.package_count, summary.file_count
        )
    }
}

fn matches_query(row: &Applet, normalized_query: &str) -> bool {
    normalized_query.is_empty()
        || row.name.to_lowercase().contains(normalized_query)
        || row.appid.to_lowercase().contains(normalized_query)
}

fn date_cell(ui: &mut egui::Ui, timestamp: u64, compact: bool) {
    let (date, time) = format_date(timestamp);
    if compact {
        let line_height = ui.text_style_height(&egui::TextStyle::Body);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), line_height * 2.0 + 1.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new(date).size(14.0).color(TEXT_PRIMARY));
                ui.label(RichText::new(time).size(14.0).color(TEXT_SECONDARY));
            },
        );
    } else {
        ui.label(
            RichText::new(format!("{date} {time}"))
                .size(14.0)
                .color(TEXT_PRIMARY),
        );
    }
}

fn format_date(timestamp: u64) -> (String, String) {
    if timestamp == 0 {
        return ("-".into(), String::new());
    }
    let Some(value) = Local.timestamp_opt(timestamp as i64, 0).single() else {
        return ("-".into(), String::new());
    };
    (
        value.format("%Y-%m-%d").to_string(),
        value.format("%H:%M:%S").to_string(),
    )
}

fn compare_rows(left: &Applet, right: &Applet, column: SortColumn) -> Ordering {
    match column {
        SortColumn::Name => compare_names(&left.name, &right.name),
        SortColumn::AppId => natural_compare(&left.appid, &right.appid),
        SortColumn::Version => natural_compare(&left.version, &right.version),
        SortColumn::PackageCount => left.package_count.cmp(&right.package_count),
        SortColumn::Size => left.package_bytes.cmp(&right.package_bytes),
        SortColumn::Created => left.created_at.cmp(&right.created_at),
        SortColumn::Modified => left.modified_at.cmp(&right.modified_at),
        SortColumn::Accessed => left.accessed_at.cmp(&right.accessed_at),
    }
}

fn compare_names(left: &str, right: &str) -> Ordering {
    let (left_initials, left_full) = pinyin_sort_key(left);
    let (right_initials, right_full) = pinyin_sort_key(right);
    natural_compare(&left_initials, &right_initials)
        .then_with(|| natural_compare(&left_full, &right_full))
        .then_with(|| natural_compare(left, right))
}

fn pinyin_sort_key(value: &str) -> (String, String) {
    let mut initials = String::new();
    let mut full = String::new();
    for character in value.chars() {
        if let Some(pinyin) = character.to_pinyin() {
            initials.push_str(pinyin.first_letter());
            full.push_str(pinyin.plain());
            full.push(' ');
        } else {
            for lower in character.to_lowercase() {
                initials.push(lower);
                full.push(lower);
            }
        }
    }
    (initials, full)
}

fn natural_compare(left: &str, right: &str) -> Ordering {
    let mut left = left.chars().peekable();
    let mut right = right.chars().peekable();
    loop {
        match (left.peek(), right.peek()) {
            (Some(a), Some(b)) if a.is_ascii_digit() && b.is_ascii_digit() => {
                let a = take_digits(&mut left);
                let b = take_digits(&mut right);
                let order = a
                    .trim_start_matches('0')
                    .len()
                    .cmp(&b.trim_start_matches('0').len())
                    .then_with(|| a.trim_start_matches('0').cmp(b.trim_start_matches('0')))
                    .then_with(|| a.len().cmp(&b.len()));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(_), Some(_)) => {
                let a = left.next().expect("peeked").to_lowercase().to_string();
                let b = right.next().expect("peeked").to_lowercase().to_string();
                let order = a.cmp(&b);
                if order != Ordering::Equal {
                    return order;
                }
            }
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
        }
    }
}

fn take_digits(iter: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut value = String::new();
    while iter.peek().is_some_and(char::is_ascii_digit) {
        value.push(iter.next().expect("peeked"));
    }
    value
}

fn configure_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = APP_BACKGROUND;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = SURFACE;
    visuals.faint_bg_color = Color32::from_rgb(247, 249, 249);
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = egui::Stroke::new(1.5, ACCENT_DARK);
    visuals.widgets.active.bg_fill = ACCENT_DARK;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT_DARK);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(241, 244, 245);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, CONTROL_BORDER);
    visuals.widgets.inactive.fg_stroke.color = TEXT_PRIMARY;
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(5);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(232, 240, 254);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    visuals.widgets.hovered.fg_stroke.color = ACCENT_DARK;
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(5);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(5);
    visuals.window_corner_radius = egui::CornerRadius::same(6);
    ctx.set_visuals(visuals);
    ctx.style_mut_of(egui::Theme::Light, |style| {
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 7.0);
        style.spacing.interact_size.y = UiMetrics::CONTROL_HEIGHT;
        style.visuals.text_options.font_hinting = false;
        style.visuals.text_options.subpixel_binning = true;
        style
            .text_styles
            .get_mut(&egui::TextStyle::Body)
            .expect("body style")
            .size = 14.0;
        style
            .text_styles
            .get_mut(&egui::TextStyle::Button)
            .expect("button style")
            .size = 14.0;
    });
}

fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        ICON_FONT.into(),
        egui::FontData::from_static(LUCIDE_FONT_BYTES).into(),
    );
    fonts
        .families
        .insert(FontFamily::Name(ICON_FONT.into()), vec![ICON_FONT.into()]);
    if let Some(path) = find_cjk_font() {
        if let Ok(bytes) = fs::read(path) {
            let font = egui::FontData::from_owned(bytes).tweak(egui::FontTweak {
                hinting: Some(false),
                subpixel_binning: Some(true),
                ..Default::default()
            });
            fonts.font_data.insert("system-cjk".into(), font.into());
            fonts
                .families
                .entry(FontFamily::Proportional)
                .or_default()
                .insert(0, "system-cjk".into());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .push("system-cjk".into());
        }
    }
    ctx.set_fonts(fonts);
}

fn find_cjk_font() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    #[cfg(target_os = "linux")]
    candidates.extend([
        "/usr/share/fonts/noto-cjk/NotoSansCJK-VF.otf.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ]);
    #[cfg(target_os = "macos")]
    candidates.extend([
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
    ]);
    #[cfg(target_os = "windows")]
    candidates.extend([r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simhei.ttf"]);
    candidates
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

fn load_window_icon() -> egui::IconData {
    let bytes = include_bytes!("../assets/icon.png");
    let image = image::load_from_memory(bytes)
        .expect("embedded icon is valid")
        .into_rgba8();
    egui::IconData {
        rgba: image.as_raw().clone(),
        width: image.width(),
        height: image.height(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{kittest::Queryable, Harness};

    fn row(name: &str, version: &str) -> Applet {
        Applet {
            appid: "wx0123456789abcdef".into(),
            name: name.into(),
            name_source: "test".into(),
            version: version.into(),
            package_count: 1,
            package_bytes: 1,
            package_size: "1 B".into(),
            icon_path: String::new(),
            package_dir: String::new(),
            main_package: String::new(),
            active_packages: Vec::new(),
            used_by: String::new(),
            depends_on: String::new(),
            created_at: 0,
            modified_at: 0,
            accessed_at: 0,
            name_confidence: 90,
            name_candidates: Vec::new(),
        }
    }

    #[test]
    fn versions_sort_naturally() {
        assert_eq!(natural_compare("2", "10"), Ordering::Less);
        assert_eq!(natural_compare("1.9", "1.10"), Ordering::Less);
        assert_eq!(
            compare_rows(&row("a", "2"), &row("b", "10"), SortColumn::Version),
            Ordering::Less
        );
    }

    #[test]
    fn names_sort_by_numbers_english_and_pinyin_initials() {
        let mut names = vec!["微信", "百度", "Alpha10", "阿里", "alpha2", "9号"];
        names.sort_by(|left, right| compare_names(left, right));
        assert_eq!(
            names,
            vec!["9号", "阿里", "alpha2", "Alpha10", "百度", "微信"]
        );
        assert_eq!(pinyin_sort_key("小程序").0, "xcx");
    }

    #[test]
    fn toast_width_tracks_content_and_stays_bounded() {
        assert_eq!(toast_width(20.0, false, 8.0), 180.0);
        assert!(toast_width(180.0, false, 8.0) < toast_width(300.0, false, 8.0));
        assert_eq!(toast_width(1_000.0, true, 8.0), 440.0);
        assert!(toast_width(180.0, true, 8.0) > toast_width(180.0, false, 8.0));
    }

    #[test]
    fn both_dates_use_the_same_compact_mode() {
        let compact = Columns::new(1008.0);
        let wide = Columns::new(1208.0);
        assert!(compact.compact);
        assert!(!wide.compact);
        assert!((compact.total_width() - 1008.0).abs() < f32::EPSILON);
        assert!((wide.total_width() - 1208.0).abs() < f32::EPSILON);
        assert!(compact.icon >= UiMetrics::APP_ICON + 2.0);
        assert!(compact.version > 0.0);
        assert!(compact.count > 0.0);
        assert!(compact.size > 0.0);
    }

    #[test]
    fn multi_character_filter_matches_name_and_appid() {
        let row = row("草料二维码", "1");
        assert!(matches_query(&row, "草料二维码"));
        assert!(matches_query(&row, "01234567"));
        assert!(!matches_query(&row, "问卷星"));
    }

    #[test]
    fn native_gui_renders_idle_and_loaded_states_at_minimum_size() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(UiMetrics::MIN_WINDOW_WIDTH, 560.0))
            .build_eframe(|cc| InspectorApp::new(cc));
        harness.get_by_label("尚未扫描缓存目录");
        let folder_icon = char::from(Icon::FolderOpen).to_string();
        assert_eq!(harness.get_all_by_label(&folder_icon).count(), 2);
        let locate_icon = char::from(Icon::LocateFixed).to_string();
        assert_eq!(harness.get_all_by_label(&locate_icon).count(), 2);
        for button in harness.get_all_by_label(&folder_icon) {
            assert!((button.rect().height() - UiMetrics::CONTROL_HEIGHT).abs() <= 1.0);
        }
        let message_center = harness.get_by_label("尚未扫描缓存目录").rect().center().x;
        let empty_locate = harness
            .get_all_by_label(&locate_icon)
            .last()
            .expect("empty-state locate button")
            .rect();
        let empty_folder = harness
            .get_all_by_label(&folder_icon)
            .last()
            .expect("empty-state folder button")
            .rect();
        let actions_center = (empty_locate.left() + empty_folder.right()) / 2.0;
        assert!((message_center - actions_center).abs() <= 1.0);

        let state = harness.state_mut();
        state.scanned = true;
        state.rows = vec![row("测试小程序", "10")];
        harness.step();

        harness.get_by_label("测试小程序");
        harness.get_by_label("创建日期");
        harness.get_by_label("修改日期");
        harness.get_by_label("访问日期");
        harness.get_by_label("版本");
        harness.get_by_label("包数");
        harness.get_by_label("大小");
        let choose_height = harness.get_by_label(&folder_icon).rect().height();
        let refresh_icon = char::from(Icon::RefreshCw).to_string();
        let scan_height = harness.get_by_label(&refresh_icon).rect().height();
        assert!((choose_height - UiMetrics::CONTROL_HEIGHT).abs() <= 1.0);
        assert!((scan_height - UiMetrics::CONTROL_HEIGHT).abs() <= 1.0);
        assert!(harness.query_by_label_contains("个小程序").is_none());
    }

    #[test]
    fn toolbar_and_progress_stay_on_one_line_at_minimum_width() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(UiMetrics::MIN_WINDOW_WIDTH, 560.0))
            .build_eframe(|cc| InspectorApp::new(cc));
        let state = harness.state_mut();
        state.root = "/tmp/Applet".into();
        state.operation = Some(Operation {
            id: 1,
            kind: OperationKind::Scan,
            completed: 3,
            total: 10,
            files: 0,
            cancelling: false,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        harness.step();

        let path_center = harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .next()
            .expect("path input")
            .rect()
            .center()
            .y;
        let progress_center = harness.get_by_label("正在扫描 3/10").rect().center().y;
        let cancel_icon = char::from(Icon::CircleX).to_string();
        let cancel_center = harness.get_by_label(&cancel_icon).rect().center().y;
        assert!((path_center - progress_center).abs() <= 1.0);
        assert!((path_center - cancel_center).abs() <= 1.0);
    }

    #[test]
    fn locating_state_is_clear_and_hides_duplicate_empty_actions() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(UiMetrics::MIN_WINDOW_WIDTH, 560.0))
            .build_eframe(|cc| InspectorApp::new(cc));
        harness.state_mut().operation = Some(Operation {
            id: 1,
            kind: OperationKind::Locate,
            completed: 0,
            total: 0,
            files: 0,
            cancelling: false,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        harness.step();

        assert_eq!(
            harness
                .get_all_by_label_contains("正在定位微信缓存")
                .count(),
            2
        );
        let locate_icon = char::from(Icon::LocateFixed).to_string();
        assert_eq!(harness.get_all_by_label(&locate_icon).count(), 1);
        let cancel_icon = char::from(Icon::CircleX).to_string();
        assert!(harness.query_by_label(&cancel_icon).is_none());

        let state = harness.state_mut();
        state.scanned = true;
        state.rows = vec![row("自动定位边界测试", "1")];
        harness.step();
        assert!(
            harness.get_by_label("正在定位微信缓存").rect().right() <= UiMetrics::MIN_WINDOW_WIDTH
        );
        assert!(harness.get_by_label("访问日期").rect().right() <= UiMetrics::MIN_WINDOW_WIDTH);
    }

    #[test]
    fn package_icon_is_square_with_or_without_a_texture() {
        let mut harness = Harness::new_ui_state(
            |ui, sizes: &mut Vec<Vec2>| {
                sizes.push(package_icon(ui, None, "无图标").rect.size());
                let texture = ui.ctx().load_texture(
                    "test-icon",
                    egui::ColorImage::new([2, 2], vec![Color32::WHITE; 4]),
                    egui::TextureOptions::LINEAR,
                );
                sizes.push(package_icon(ui, Some(texture.id()), "有图标").rect.size());
            },
            Vec::new(),
        );
        harness.run();
        for size in harness.state().iter().rev().take(2) {
            assert_eq!(*size, Vec2::splat(UiMetrics::APP_ICON));
        }
    }

    #[test]
    fn dependency_controls_are_centered_and_wrap_inside_the_table() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(UiMetrics::MIN_WINDOW_WIDTH, 560.0))
            .build_eframe(|cc| InspectorApp::new(cc));
        let mut applet = row("依赖对齐测试", "1");
        applet.depends_on =
            "wx1111111111111111、wx2222222222222222、wx3333333333333333、wx4444444444444444".into();
        let appid = applet.appid.clone();
        let state = harness.state_mut();
        state.scanned = true;
        state.expanded = Some(appid);
        state.rows = vec![applet];
        harness.run();

        let name_center = harness.get_by_label("依赖对齐测试").rect().center().y;
        let disclosure_center = harness.get_by_label("收起依赖").rect().center().y;
        assert!((name_center - disclosure_center).abs() <= 1.0);

        let label_center = harness.get_by_label("插件 4").rect().center().y;
        let first = harness.get_by_label_contains("wx1111111111111111").rect();
        assert!((label_center - first.center().y).abs() <= 1.0);
        for button in harness.get_all_by_label_contains("wx") {
            assert!(button.rect().right() <= UiMetrics::MIN_WINDOW_WIDTH);
        }
    }

    #[test]
    fn enter_in_path_field_starts_scan() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(UiMetrics::MIN_WINDOW_WIDTH, 560.0))
            .build_eframe(|cc| InspectorApp::new(cc));
        harness.state_mut().root = "/path/that/does/not/exist".into();
        harness.run();
        harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .next()
            .expect("path input exists")
            .focus();
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert_eq!(harness.state().next_operation, 1);
    }

    #[test]
    fn compact_dates_and_notice_content_are_vertically_centered() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(UiMetrics::MIN_WINDOW_WIDTH, 560.0))
            .build_eframe(|cc| InspectorApp::new(cc));
        let mut applet = row("垂直居中测试", "10");
        applet.created_at = 1_700_000_000;
        let (date, time) = format_date(applet.created_at);
        let state = harness.state_mut();
        state.scanned = true;
        state.selected.insert(applet.appid.clone());
        state.rows = vec![applet];
        state.notice = Some(Notice {
            message: "操作完成".into(),
            error: false,
            persistent: true,
            created: Instant::now(),
            output: Some(PathBuf::from("/tmp")),
            details: Vec::new(),
        });
        harness.run();

        let name_center = harness.get_by_label("垂直居中测试").rect().center().y;
        let date_rect = harness.get_by_label(&date).rect();
        let time_rect = harness.get_by_label(&time).rect();
        let date_block_center = (date_rect.top() + time_rect.bottom()) / 2.0;
        assert!(
            (date_block_center - name_center).abs() <= 2.0,
            "date block center={date_block_center}, row content center={name_center}"
        );

        let notice_center = harness.get_by_label("操作完成").rect().center().y;
        let close_icon = char::from(Icon::X).to_string();
        let close = harness
            .get_all_by_label(&close_icon)
            .last()
            .expect("notice close button")
            .rect();
        let bulk_close = harness
            .get_all_by_label(&close_icon)
            .next()
            .expect("bulk action close button")
            .rect();
        assert!((notice_center - close.center().y).abs() <= 1.0);
        let folder_icon = char::from(Icon::FolderOpen).to_string();
        let output = harness
            .get_all_by_label(&folder_icon)
            .last()
            .expect("open output button")
            .rect();
        assert!((notice_center - output.center().y).abs() <= 1.0);
        assert!((close.height() - UiMetrics::CONTROL_HEIGHT).abs() <= 1.0);
        assert!((close.width() - 28.0).abs() <= 1.0);
        assert!((bulk_close.width() - 28.0).abs() <= 1.0);
        assert_eq!(NOTICE_DURATION, Duration::from_secs(5));
        assert!(harness.query_by_label("完成").is_none());
        assert!(harness.query_by_label("成功").is_none());
        let status = harness
            .get_by_label(&char::from(Icon::CheckCircle2).to_string())
            .rect();
        assert!((notice_center - status.center().y).abs() <= 1.0);
        let extract = harness
            .get_by_label(&char::from(Icon::ArchiveRestore).to_string())
            .rect();
        assert!((extract.height() - UiMetrics::CONTROL_HEIGHT).abs() <= 1.0);
        assert!((extract.width() - 46.0).abs() <= 1.0);
        let mode_menu = harness
            .get_by_label(&char::from(Icon::ChevronDown).to_string())
            .rect();
        assert!((mode_menu.height() - UiMetrics::CONTROL_HEIGHT).abs() <= 1.0);
        assert!(mode_menu.right() <= UiMetrics::MIN_WINDOW_WIDTH);
    }

    #[test]
    fn error_notice_uses_an_icon_instead_of_a_text_prefix() {
        let mut harness = Harness::builder()
            .with_size(Vec2::new(UiMetrics::MIN_WINDOW_WIDTH, 560.0))
            .build_eframe(|cc| InspectorApp::new(cc));
        harness.state_mut().notice = Some(Notice {
            message: "扫描失败".into(),
            error: true,
            persistent: true,
            created: Instant::now(),
            output: None,
            details: Vec::new(),
        });
        harness.run();

        let message_center = harness.get_by_label("扫描失败").rect().center().y;
        let status = harness
            .get_by_label(&char::from(Icon::CircleX).to_string())
            .rect();
        assert!((message_center - status.center().y).abs() <= 1.0);
        assert!(harness.query_by_label("错误").is_none());
    }
}
