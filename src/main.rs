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
use sysinfo::{Disks, System};

const HIGH_FAN_RPM_WARNING: u32 = 6_200;
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/aayanmasud/linuxstats/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/aayanmasud/linuxstats/releases/latest";

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
    last_animation_update: Instant,
    fan_rotation: f32,
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
            last_animation_update: now,
            fan_rotation: 0.0,
            tutorial_step: if tutorial_is_completed() { None } else { Some(0) },
            update_rx,
            update_available: false,
            update_dismissed: false,
        };
        app.refresh_stats();
        app
    }

    fn refresh_stats(&mut self) {
        self.sys.refresh_all();
        self.cpu_percent = self.sys.global_cpu_usage().clamp(0.0, 100.0);
        self.uptime = System::uptime().unwrap_or(0);
        self.fans = read_fan_speeds();

        let total_memory = self.sys.total_memory();
        let used_memory = self.sys.used_memory();
        self.ram_percent = if total_memory > 0 {
            (used_memory as f32 / total_memory as f32 * 100.0).clamp(0.0, 100.0)
        } else {
            0.0
        };

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

        let total_memory = self.sys.total_memory();
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
    }

    fn max_fan_rpm(&self) -> u32 {
        self.fans.iter().map(|fan| fan.rpm).max().unwrap_or(0)
    }
}

fn check_for_update() -> bool {
    // Use curl for HTTPS so the application does not need another Rust dependency.
    // If curl is unavailable or the network request fails, simply skip the check.
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
        version
            .trim_start_matches('v')
            .split('.')
            .map(|part| part.parse::<u64>().ok())
            .collect()
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
        let sensor_number = file_name
            .strip_prefix("fan")
            .and_then(|name| name.strip_suffix("_input"))
            .unwrap_or("?");
        let label_path = input_path.with_file_name(format!("fan{sensor_number}_label"));
        let sensor_label = fs::read_to_string(label_path)
            .ok()
            .map(|label| label.trim().to_owned())
            .filter(|label| !label.is_empty())
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
                .ok()
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
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

fn draw_fan_icon(ui: &mut egui::Ui, rotation: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(132.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    painter.circle_filled(center, 62.0, egui::Color32::from_rgb(31, 41, 55));
    painter.circle_stroke(center, 61.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(75, 85, 99)));
    let blade_shape = [
        egui::Vec2::new(-7.0, -9.0), egui::Vec2::new(7.0, -9.0),
        egui::Vec2::new(15.0, -39.0), egui::Vec2::new(9.0, -49.0),
        egui::Vec2::new(-5.0, -47.0), egui::Vec2::new(-13.0, -34.0),
    ];
    for blade in 0..3 {
        let angle = rotation + blade as f32 * std::f32::consts::TAU / 3.0;
        let (sin, cos) = angle.sin_cos();
        let points = blade_shape.iter().map(|point| {
            center + egui::Vec2::new(point.x * cos - point.y * sin, point.x * sin + point.y * cos)
        }).collect();
        painter.add(egui::Shape::convex_polygon(
            points,
            egui::Color32::from_rgb(96, 165, 250),
            egui::Stroke::new(1.0, egui::Color32::from_rgb(147, 197, 253)),
        ));
    }
    painter.circle_filled(center, 10.0, egui::Color32::from_rgb(226, 232, 240));
    painter.circle_filled(center, 4.0, egui::Color32::from_rgb(71, 85, 105));
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

fn stat_row(ui: &mut egui::Ui, label: &str, percent: f32) {
    ui.label(label);
    ui.add(egui::ProgressBar::new(percent / 100.0).text(format!("{percent:.1}%")));
    ui.end_row();
}

impl eframe::App for MonitorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Ok(update_available) = self.update_rx.try_recv() {
            self.update_available = update_available;
        }
        if self.last_update.elapsed() >= Duration::from_secs(1) {
            self.refresh_stats();
            self.last_update = Instant::now();
        }

        let now = Instant::now();
        let elapsed = now.duration_since(self.last_animation_update).as_secs_f32();
        self.last_animation_update = now;
        let fan_rpm = self.max_fan_rpm();
        self.fan_rotation = (self.fan_rotation
            + elapsed * std::f32::consts::TAU * fan_rpm as f32 / 3000.0)
            .rem_euclid(std::f32::consts::TAU);
        let high_fan_speed = fan_rpm >= HIGH_FAN_RPM_WARNING;
        let busiest_process = self.processes.first();

        let mut tutorial_action = None;
        if let Some(step) = self.tutorial_step {
            let (title, description) = match step {
                0 => ("Welcome to Linux System Monitor", "This quick tour shows you how to read your system's live CPU, memory, disk, and process information."),
                1 => ("System usage", "The CPU, Memory, and Disk bars show the percentage currently in use. System uptime shows how long your computer has been running."),
                2 => ("Top processes", "This table lists up to ten processes, sorted by CPU use. It also shows each process's memory use and process ID (PID)."),
                _ => ("You're ready to go", "Statistics refresh automatically every second. You can close this window whenever you're done monitoring your system."),
            };
            egui::Window::new("Quick tutorial")
                .collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
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
                    ui.add_space(4.0);
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
            egui::Frame::group(ui.style())
                .fill(egui::Color32::from_rgb(55, 42, 18))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(245, 158, 11)))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Your app has to update to the latest version!").strong().color(egui::Color32::from_rgb(253, 230, 138)));
                        ui.hyperlink_to("Download latest version", RELEASES_PAGE);
                        if ui.small_button("Later").clicked() { self.update_dismissed = true; }
                    });
                });
        });

        egui::SidePanel::right("fan_panel").resizable(false).default_width(235.0).show(ctx, |ui| {
            let top_space = ((ui.available_height() - 300.0) / 2.0).max(0.0);
            ui.add_space(top_space);
            ui.vertical_centered(|ui| {
                ui.heading("Fan speed");
                ui.add_space(12.0);
                draw_fan_icon(ui, self.fan_rotation);
                ui.add_space(12.0);
                if self.fans.is_empty() {
                    ui.label("No fan sensors found");
                    ui.add_space(4.0);
                    ui.label("This computer or its Linux drivers may not expose fan readings.");
                } else {
                    for fan in &self.fans {
                        ui.strong(&fan.label);
                        ui.label(format!("{} RPM", fan.rpm));
                        ui.add_space(8.0);
                    }
                }
                if high_fan_speed {
                    ui.add_space(8.0);
                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_rgb(75, 38, 20))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(249, 115, 22)))
                        .show(ui, |ui| {
                            ui.colored_label(egui::Color32::from_rgb(253, 186, 116), "⚠ High fan speed");
                            ui.label(format!("Fan is at {fan_rpm} RPM."));
                            if let Some(process) = busiest_process {
                                ui.label(format!("Highest CPU use: {} ({:.1}%).", process.name, process.cpu));
                                ui.label("Highlighted in the task list as a possible contributor—not a confirmed cause.");
                            } else {
                                ui.label("High system load or heat may be involved; no process data is available.");
                            }
                        });
                }
                ui.add_space(8.0);
                ui.small("Readings refresh every second.");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Linux System Monitor");
            ui.label(format!("System uptime: {}", format_uptime(self.uptime)));
            ui.add_space(12.0);
            egui::Grid::new("system_stats").num_columns(2).spacing([16.0, 10.0]).show(ui, |ui| {
                stat_row(ui, "CPU", self.cpu_percent);
                stat_row(ui, "Memory", self.ram_percent);
                stat_row(ui, "Disk", self.disk_percent);
            });
            ui.add_space(16.0);
            ui.heading("Top Processes");
            ui.add_space(6.0);
            if high_fan_speed {
                ui.colored_label(egui::Color32::from_rgb(249, 115, 22), "⚠ High CPU process highlighted; it may contribute to heat, but this is not proof of cause.");
                ui.add_space(4.0);
            }
            egui::ScrollArea::vertical().max_height(350.0).show(ui, |ui| {
                egui::Grid::new("process_table").striped(true).num_columns(5).show(ui, |ui| {
                    ui.strong(""); ui.strong("PID"); ui.strong("Name"); ui.strong("CPU %"); ui.strong("Memory %"); ui.end_row();
                    for (index, process) in self.processes.iter().enumerate() {
                        let highlight = high_fan_speed && index == 0;
                        let color = if highlight { egui::Color32::from_rgb(253, 186, 116) } else { ui.visuals().text_color() };
                        if highlight { ui.colored_label(color, "⚠"); } else { ui.label(""); }
                        ui.colored_label(color, process.pid.to_string());
                        ui.colored_label(color, &process.name);
                        ui.colored_label(color, format!("{:.1}", process.cpu));
                        ui.colored_label(color, format!("{:.1}", process.memory));
                        ui.end_row();
                    }
                });
            });
            ui.add_space(12.0);
            ui.label("Stats refresh every second.");
        });

        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([900.0, 600.0]).with_min_inner_size([600.0, 400.0]),
        ..Default::default()
    };
    eframe::run_native("Linux System Monitor", options, Box::new(|_creation_context| Ok(Box::new(MonitorApp::new()))))
}
