use std::{
    cmp::Ordering,
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use eframe::egui;
use sysinfo::{get_current_pid, Disks, Pid, Signal, System};

const HIGH_FAN_RPM_WARNING: u32 = 6_200;
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/modenlaggard/linuxstats/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/modenlaggard/linuxstats/releases/latest";
const SYSTEM_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const FAN_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const DISK_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const EFFICIENT_SYSTEM_REFRESH_INTERVAL: Duration = Duration::from_secs(8);
const EFFICIENT_FAN_REFRESH_INTERVAL: Duration = Duration::from_secs(20);
const EFFICIENT_DISK_REFRESH_INTERVAL: Duration = Duration::from_secs(120);
const CARD_BG: egui::Color32 = egui::Color32::from_rgb(28, 37, 53);
const MUTED: egui::Color32 = egui::Color32::from_rgb(160, 174, 192);

#[derive(Clone, Copy)]
enum ProcessAction {
    Terminate,
    Kill,
    Stop,
    Continue,
}

impl ProcessAction {
    fn label(self) -> &'static str {
        match self {
            Self::Terminate => "Terminate (graceful)",
            Self::Kill => "Kill (force)",
            Self::Stop => "Stop (pause)",
            Self::Continue => "Continue (resume)",
        }
    }

    fn signal(self) -> Signal {
        match self {
            Self::Terminate => Signal::Term,
            Self::Kill => Signal::Kill,
            Self::Stop => Signal::Stop,
            Self::Continue => Signal::Cont,
        }
    }

    fn warning(self) -> &'static str {
        match self {
            Self::Terminate => "The process will be asked to exit and may lose unsaved work.",
            Self::Kill => "This forcefully ends the process immediately. Unsaved work will be lost.",
            Self::Stop => "The process will be paused until you choose Continue or another program resumes it.",
            Self::Continue => "The paused process will resume running.",
        }
    }
}

struct ProcessInfo {
    pid: u32,
    name: String,
    cpu: f32,
    memory: f32,
}

struct FanInfo {
    label: String,
    rpm: u32,
}

struct MonitorApp {
    sys: System,
    cpu_percent: f32,
    ram_percent: f32,
    disk_percent: f32,
    uptime: u64,
    processes: Vec<ProcessInfo>,
    fans: Vec<FanInfo>,
    last_update: Instant,
    last_fan_update: Instant,
    last_disk_update: Instant,
    efficiency_mode: bool,
    pending_action: Option<(u32, String, ProcessAction)>,
    action_status: Option<String>,
    tutorial_step: Option<usize>,
    update_rx: Receiver<bool>,
    update_available: bool,
    update_dismissed: bool,
}

impl MonitorApp {
    fn new() -> Self {
        let now = Instant::now();
        let (update_tx, update_rx) = mpsc::channel();
        thread::spawn(move || {
            let is_available = check_for_update();
            let _ = update_tx.send(is_available);
        });

        let mut app = Self {
            sys: System::new_all().expect("Could not read system information"),
            cpu_percent: 0.0,
            ram_percent: 0.0,
            disk_percent: 0.0,
            uptime: 0,
            processes: Vec::new(),
            fans: Vec::new(),
            last_update: now,
            last_fan_update: now,
            last_disk_update: now,
            efficiency_mode: false,
            pending_action: None,
            action_status: None,
            tutorial_step: if tutorial_is_completed() { None } else { Some(0) },
            update_rx,
            update_available: false,
            update_dismissed: false,
        };
        app.refresh_system_stats();
        app.refresh_disk_stats();
        app.refresh_fans();
        app
    }

    fn refresh_system_stats(&mut self) {
        self.sys.refresh_all();
        self.cpu_percent = self.sys.global_cpu_usage().clamp(0.0, 100.0);
        self.uptime = System::uptime().unwrap_or(0);

        let total_memory = self.sys.total_memory();
        let used_memory = self.sys.used_memory();
        self.ram_percent = if total_memory > 0 {
            (used_memory as f32 / total_memory as f32 * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };

        let mut processes: Vec<ProcessInfo> = self
            .sys
            .processes()
            .iter()
            .map(|(pid, process)| {
                let memory_percent = if total_memory > 0 {
                    process.memory() as f32 / total_memory as f32 * 100.0
                } else {
                    0.0
                };
                ProcessInfo {
                    pid: pid.as_u32(),
                    name: process.name().to_string_lossy().into_owned(),
                    cpu: process.cpu_usage(),
                    memory: memory_percent,
                }
            })
            .collect();
        processes.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(Ordering::Equal));
        self.processes = processes.into_iter().take(10).collect();
        self.last_update = Instant::now();
    }

    fn refresh_disk_stats(&mut self) {
        let disks = Disks::new_with_refreshed_list().expect("Could not read disk information");
        let mut total_disk_space = 0u64;
        let mut used_disk_space = 0u64;
        for disk in disks.iter() {
            total_disk_space += disk.total_space();
            used_disk_space += disk.total_space().saturating_sub(disk.available_space());
        }
        self.disk_percent = if total_disk_space > 0 {
            (used_disk_space as f32 / total_disk_space as f32 * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };
        self.last_disk_update = Instant::now();
    }

    fn refresh_fans(&mut self) {
        self.fans = read_fan_speeds();
        self.last_fan_update = Instant::now();
    }

    fn max_fan_rpm(&self) -> u32 {
        self.fans.iter().map(|fan| fan.rpm).max().unwrap_or(0)
    }

    fn perform_process_action(&mut self, pid: u32, name: &str, action: ProcessAction) {
        // Never allow this UI to signal PID 1 or itself. The OS still enforces user permissions.
        if pid <= 1 || get_current_pid().ok() == Some(Pid::from_u32(pid)) {
            self.action_status = Some("For safety, actions on PID 1 and Linuxstats itself are disabled.".to_owned());
            return;
        }
        let Some(process) = self.sys.process(Pid::from_u32(pid)) else {
            self.action_status = Some(format!("Process {pid} no longer exists; refresh and try again."));
            return;
        };
        match process.kill_with(action.signal()) {
            Some(true) => {
                self.action_status = Some(format!("{} signal sent to {name} (PID {pid}).", action.label()));
                self.last_update = Instant::now() - SYSTEM_REFRESH_INTERVAL;
            }
            Some(false) => {
                self.action_status = Some(format!("Could not send a signal to {name} (PID {pid}); permission may be denied."));
            }
            None => {
                self.action_status = Some("This platform does not support that process signal.".to_owned());
            }
        }
    }
}

fn check_for_update() -> bool {
    let Ok(response) = Command::new("curl")
        .args([
            "--fail", "--silent", "--show-error", "--location", "--max-time", "8",
            "--user-agent", "linuxstats", LATEST_RELEASE_API,
        ])
        .output()
    else {
        return false;
    };
    if !response.status.success() {
        return false;
    }
    let body = String::from_utf8_lossy(&response.stdout);
    let Some(tag) = body.split_once("\"tag_name\"")
        .and_then(|(_, rest)| rest.split_once(':'))
        .and_then(|(_, rest)| rest.trim_start().strip_prefix('"'))
        .and_then(|rest| rest.split_once('"').map(|(tag, _)| tag))
    else {
        return false;
    };
    version_is_newer(tag, env!("CARGO_PKG_VERSION"))
}

fn version_is_newer(latest: &str, current: &str) -> bool {
    let parse = |version: &str| -> Option<Vec<u64>> {
        version.trim_start_matches('v').split('.').map(|part| part.parse::<u64>().ok()).collect()
    };
    match (parse(latest), parse(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

fn append_fans_from_directory(
    directory: &Path,
    chip_name: &str,
    fans: &mut Vec<FanInfo>,
    seen_paths: &mut HashSet<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(directory) else { return };
    let mut sensor_paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map_or(false, |name| name.starts_with("fan") && name.ends_with("_input"))
        })
        .collect();
    sensor_paths.sort();

    for input_path in sensor_paths {
        let Ok(value) = fs::read_to_string(&input_path) else { continue };
        let Ok(rpm) = value.trim().parse::<u32>() else { continue };
        let identity = fs::canonicalize(&input_path).unwrap_or_else(|_| input_path.clone());
        if !seen_paths.insert(identity) { continue; }
        let Some(file_name) = input_path.file_name().and_then(|name| name.to_str()) else { continue };
        let sensor_number = file_name.strip_prefix("fan").and_then(|name| name.strip_suffix("_input")).unwrap_or("?");
        let label_path = input_path.with_file_name(format!("fan{sensor_number}_label"));
        let sensor_label = fs::read_to_string(label_path)
            .ok().map(|label| label.trim().to_owned()).filter(|label| !label.is_empty())
            .unwrap_or_else(|| format!("Fan {sensor_number}"));
        fans.push(FanInfo { label: format!("{chip_name} · {sensor_label}"), rpm });
    }
}

fn read_fan_speeds() -> Vec<FanInfo> {
    let mut fans = Vec::new();
    let mut seen_paths = HashSet::new();
    if let Ok(entries) = fs::read_dir("/sys/class/hwmon") {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = fs::read_to_string(path.join("name"))
                .ok().map(|name| name.trim().to_owned()).filter(|name| !name.is_empty())
                .unwrap_or_else(|| "Hardware monitor".to_owned());
            append_fans_from_directory(&path, &name, &mut fans, &mut seen_paths);
        }
    }
    if let Ok(entries) = fs::read_dir("/sys/devices/platform") {
        for entry in entries.flatten() {
            if entry.file_name().to_str().map_or(false, |name| name.starts_with("applesmc.")) {
                append_fans_from_directory(&entry.path(), "Apple SMC", &mut fans, &mut seen_paths);
            }
        }
    }
    fans.sort_by(|a, b| a.label.cmp(&b.label));
    fans
}

fn draw_fan_icon(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(56.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    painter.circle_filled(center, 26.0, egui::Color32::from_rgb(36, 54, 76));
    painter.circle_stroke(center, 25.0, egui::Stroke::new(1.0, egui::Color32::from_rgb(71, 98, 128)));
    for blade in 0..3 {
        let angle = blade as f32 * std::f32::consts::TAU / 3.0;
        let direction = egui::Vec2::angled(angle);
        let tip = center + direction * 19.0;
        let side = egui::Vec2::new(-direction.y, direction.x) * 5.0;
        painter.add(egui::Shape::convex_polygon(
            vec![center - side, center + side, tip],
            egui::Color32::from_rgb(96, 165, 250),
            egui::Stroke::NONE,
        ));
    }
    painter.circle_filled(center, 5.0, egui::Color32::from_rgb(226, 232, 240));
}

fn tutorial_marker_path() -> Option<PathBuf> {
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        if !config_home.is_empty() {
            return Some(PathBuf::from(config_home).join("linuxstats/tutorial-completed"));
        }
    }
    env::var_os("HOME").map(PathBuf::from).map(|home| home.join(".config/linuxstats/tutorial-completed"))
}

fn tutorial_is_completed() -> bool {
    tutorial_marker_path().map_or(false, |path| path.exists())
}

fn mark_tutorial_completed() {
    if let Some(path) = tutorial_marker_path() {
        if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
        let _ = fs::write(path, b"completed\n");
    }
}

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    format!("{days}d {hours}h {minutes}m")
}

fn metric_card(ui: &mut egui::Ui, title: &str, value: f32, color: egui::Color32, description: &str) {
    egui::Frame::group(ui.style()).fill(CARD_BG).inner_margin(12.0).show(ui, |ui| {
        ui.set_min_width(110.0);
        ui.label(egui::RichText::new(title).color(MUTED).size(12.0).strong());
        ui.add_space(3.0);
        ui.label(egui::RichText::new(format!("{value:.1}%")).size(24.0).strong());
        ui.add(egui::ProgressBar::new(value / 100.0).fill(color).desired_height(7.0).text(""));
        ui.add_space(3.0);
        ui.label(egui::RichText::new(description).color(MUTED).small());
    });
}

impl eframe::App for MonitorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Ok(update_available) = self.update_rx.try_recv() {
            self.update_available = update_available;
        }

        let system_interval = if self.efficiency_mode { EFFICIENT_SYSTEM_REFRESH_INTERVAL } else { SYSTEM_REFRESH_INTERVAL };
        let fan_interval = if self.efficiency_mode { EFFICIENT_FAN_REFRESH_INTERVAL } else { FAN_REFRESH_INTERVAL };
        let disk_interval = if self.efficiency_mode { EFFICIENT_DISK_REFRESH_INTERVAL } else { DISK_REFRESH_INTERVAL };
        if self.last_update.elapsed() >= system_interval { self.refresh_system_stats(); }
        if self.last_fan_update.elapsed() >= fan_interval { self.refresh_fans(); }
        if self.last_disk_update.elapsed() >= disk_interval { self.refresh_disk_stats(); }

        let fan_rpm = self.max_fan_rpm();
        let high_fan_speed = fan_rpm >= HIGH_FAN_RPM_WARNING;
        let busiest_process = self.processes.first();

        let mut tutorial_action = None;
        if let Some(step) = self.tutorial_step {
            let (title, description) = match step {
                0 => ("Welcome to Linux System Monitor", "This quick tour shows you how to read your system's live CPU, memory, disk, and process information."),
                1 => ("System usage", "The CPU, Memory, and Disk cards show the percentage currently in use. System uptime shows how long your computer has been running."),
                2 => ("Top processes", "Right-click a process for actions. Signals may be denied if you do not own that process."),
                _ => ("You're ready to go", "Statistics refresh automatically. Fan readings appear when your system exposes them."),
            };
            egui::Window::new("Quick tutorial").collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_min_width(320.0);
                    ui.heading(title);
                    ui.add_space(8.0);
                    ui.label(description);
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Skip tutorial").clicked() { tutorial_action = Some("skip"); }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(if step == 3 { "Finish" } else { "Next" }).clicked() { tutorial_action = Some("next"); }
                            if step > 0 && ui.button("Back").clicked() { tutorial_action = Some("back"); }
                        });
                    });
                    ui.label(format!("Step {} of 4", step + 1));
                });
        }
        match tutorial_action {
            Some("skip") => { self.tutorial_step = None; mark_tutorial_completed(); }
            Some("back") => { self.tutorial_step = self.tutorial_step.map(|step| step.saturating_sub(1)); }
            Some("next") => {
                self.tutorial_step = match self.tutorial_step { Some(0..=2) => self.tutorial_step.map(|step| step + 1), _ => None };
                if self.tutorial_step.is_none() { mark_tutorial_completed(); }
            }
            _ => {}
        }

        egui::TopBottomPanel::top("update_notice").show_animated(ctx, self.update_available && !self.update_dismissed, |ui| {
            egui::Frame::group(ui.style()).fill(egui::Color32::from_rgb(55, 42, 18))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(245, 158, 11))).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new("Your app has to update to the latest version!").strong().color(egui::Color32::from_rgb(253, 230, 138)));
                        ui.hyperlink_to("Download latest version", RELEASES_PAGE);
                        if ui.small_button("Later").clicked() { self.update_dismissed = true; }
                    });
                });
        });

        let mut requested_action: Option<(u32, String, ProcessAction)> = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading(egui::RichText::new("Linuxstats").size(28.0).strong());
                        ui.label(egui::RichText::new("Your system, at a glance").color(MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        egui::Frame::group(ui.style()).fill(CARD_BG).inner_margin(10.0).show(ui, |ui| {
                            ui.label(egui::RichText::new("UPTIME").color(MUTED).small().strong());
                            ui.label(egui::RichText::new(format_uptime(self.uptime)).strong());
                        });
                    });
                });

                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    let label = if self.efficiency_mode { "Efficiency mode: on" } else { "Efficiency mode: off" };
                    if ui.toggle_value(&mut self.efficiency_mode, label).changed() {
                        self.last_update = Instant::now();
                        self.last_fan_update = Instant::now();
                        self.last_disk_update = Instant::now();
                    }
                    ui.label(egui::RichText::new(if self.efficiency_mode {
                        "Lower activity: system every 8s, fans every 20s, storage every 2 min."
                    } else {
                        "Standard monitoring: system every 2s, fans every 5s, storage every 30s."
                    }).color(MUTED).small());
                });

                ui.add_space(16.0);
                ui.heading("System overview");
                ui.label(egui::RichText::new(if self.efficiency_mode { "Efficiency mode · less frequent updates" } else { "Live usage · updates every 2 seconds" }).color(MUTED));
                ui.add_space(10.0);
                ui.columns(3, |columns| {
                    metric_card(&mut columns[0], "PROCESSOR", self.cpu_percent, egui::Color32::from_rgb(96, 165, 250), "CPU in use");
                    metric_card(&mut columns[1], "MEMORY", self.ram_percent, egui::Color32::from_rgb(167, 139, 250), "RAM in use");
                    metric_card(&mut columns[2], "STORAGE", self.disk_percent, egui::Color32::from_rgb(52, 211, 153), "Disk space used");
                });

                ui.add_space(22.0);
                ui.heading("Top processes");
                ui.label(egui::RichText::new("Right-click a process to manage it. Actions are confirmed before a signal is sent.").color(MUTED));
                if high_fan_speed {
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::from_rgb(253, 186, 116), "High fan speed detected; the top process is only a possible contributor.");
                }
                if let Some(status) = &self.action_status {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(status);
                        if ui.small_button("Dismiss").clicked() { self.action_status = None; }
                    });
                }
                ui.add_space(8.0);
                egui::Frame::group(ui.style()).fill(CARD_BG).inner_margin(10.0).show(ui, |ui| {
                    egui::ScrollArea::horizontal().show(ui, |ui| {
                        egui::Grid::new("process_table").striped(true).num_columns(4).spacing([20.0, 8.0]).show(ui, |ui| {
                            ui.strong("PROCESS"); ui.strong("PID"); ui.strong("CPU"); ui.strong("MEMORY"); ui.end_row();
                            if self.processes.is_empty() {
                                ui.label("No process information available yet."); ui.label(""); ui.label(""); ui.label(""); ui.end_row();
                            }
                            for (index, process) in self.processes.iter().enumerate() {
                                let is_top = high_fan_speed && index == 0;
                                let name = if is_top { format!("⚠ {}", process.name) } else { process.name.clone() };
                                let response = if is_top {
                                    ui.colored_label(egui::Color32::from_rgb(253, 186, 116), name)
                                } else {
                                    ui.label(name)
                                };
                                let pid = process.pid;
                                let process_name = process.name.clone();
                                response.context_menu(|ui| {
                                    ui.label(egui::RichText::new(format!("{} · PID {pid}", process_name)).strong());
                                    ui.separator();
                                    for action in [ProcessAction::Terminate, ProcessAction::Kill, ProcessAction::Stop, ProcessAction::Continue] {
                                        if ui.button(action.label()).clicked() {
                                            requested_action = Some((pid, process_name.clone(), action));
                                            ui.close_menu();
                                        }
                                    }
                                });
                                ui.label(process.pid.to_string());
                                ui.label(format!("{:.1}%", process.cpu));
                                ui.label(format!("{:.1}%", process.memory));
                                ui.end_row();
                            }
                        });
                    });
                });

                ui.add_space(22.0);
                ui.horizontal(|ui| {
                    draw_fan_icon(ui);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.heading("Cooling");
                        ui.label(egui::RichText::new("Fan data is available only when supported by your hardware").color(MUTED));
                    });
                });
                ui.add_space(8.0);
                egui::Frame::group(ui.style()).fill(CARD_BG).inner_margin(12.0).show(ui, |ui| {
                    if self.fans.is_empty() {
                        ui.label(egui::RichText::new("No fan sensors detected").strong());
                        ui.label(egui::RichText::new("Your computer or its Linux drivers may not expose fan speeds.").color(MUTED));
                    } else {
                        for fan in &self.fans {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&fan.label).strong());
                                ui.label(format!("{} RPM", fan.rpm));
                            });
                        }
                    }
                    if high_fan_speed {
                        ui.add_space(6.0);
                        ui.colored_label(egui::Color32::from_rgb(253, 186, 116), format!("High fan speed: {fan_rpm} RPM · advisory reading, not a diagnosis."));
                        if let Some(process) = busiest_process {
                            ui.label(egui::RichText::new(format!("Highest CPU use: {} ({:.1}%)", process.name, process.cpu)).color(MUTED).small());
                        }
                    }
                });

                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("Background polling slows in efficiency mode; the dashboard stays available.").color(MUTED).small());
                });
                ui.add_space(12.0);
            });
        });
        if let Some(action) = requested_action {
            self.pending_action = Some(action);
            self.action_status = None;
        }

        let mut confirm = false;
        let mut cancel = false;
        if let Some((pid, ref name, action)) = self.pending_action {
            egui::Window::new("Confirm process action")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_min_width(330.0);
                    ui.label(format!("{} · PID {pid}", name));
                    ui.add_space(8.0);
                    ui.label(action.warning());
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() { cancel = true; }
                        let button = if matches!(action, ProcessAction::Kill) { "Force kill" } else { "Confirm" };
                        if ui.button(button).clicked() { confirm = true; }
                    });
                });
        }
        if confirm {
            if let Some((pid, name, action)) = self.pending_action.take() {
                self.perform_process_action(pid, &name, action);
            }
        } else if cancel {
            self.pending_action = None;
        }

        ctx.request_repaint_after(if self.efficiency_mode { Duration::from_secs(4) } else { Duration::from_secs(1) });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 720.0]).with_min_inner_size([540.0, 500.0]),
        ..Default::default()
    };
    eframe::run_native("Linuxstats", options, Box::new(|creation_context| {
        let mut style = (*creation_context.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = egui::Color32::from_rgb(16, 22, 34);
        style.visuals.window_fill = egui::Color32::from_rgb(24, 32, 46);
        style.visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(28, 37, 53);
        style.spacing.item_spacing = egui::vec2(10.0, 8.0);
        creation_context.egui_ctx.set_style(style);
        Ok(Box::new(MonitorApp::new()))
    }))
}
