use std::{
    cmp::Ordering,
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use eframe::egui;
use sysinfo::{Disks, System};

const HIGH_FAN_RPM_WARNING: u32 = 6_200;
const CARD_BG: egui::Color32 = egui::Color32::from_rgb(25, 33, 48);
const MUTED: egui::Color32 = egui::Color32::from_rgb(160, 174, 192);

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
}

impl MonitorApp {
    fn new() -> Self {
        let now = Instant::now();
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
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(72.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let center = rect.center();
    painter.circle_filled(center, 34.0, egui::Color32::from_rgb(31, 41, 55));
    painter.circle_stroke(center, 33.0, egui::Stroke::new(1.5, egui::Color32::from_rgb(75, 85, 99)));
    let blade_shape = [
        egui::Vec2::new(-4.0, -5.0), egui::Vec2::new(4.0, -5.0),
        egui::Vec2::new(9.0, -21.0), egui::Vec2::new(5.0, -27.0),
        egui::Vec2::new(-3.0, -26.0), egui::Vec2::new(-7.0, -18.0),
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
    painter.circle_filled(center, 6.0, egui::Color32::from_rgb(226, 232, 240));
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

fn metric_card(ui: &mut egui::Ui, title: &str, value: f32, color: egui::Color32, hint: &str) {
    egui::Frame::group(ui.style())
        .fill(CARD_BG)
        .inner_margin(12.0)
        .show(ui, |ui| {
            ui.set_min_width(120.0);
            ui.label(egui::RichText::new(title).color(MUTED).size(13.0));
            ui.add_space(4.0);
            ui.label(egui::RichText::new(format!("{value:.1}%")).size(25.0).strong());
            ui.add(egui::ProgressBar::new(value / 100.0).fill(color).desired_height(8.0));
            ui.add_space(4.0);
            ui.label(egui::RichText::new(hint).color(MUTED).small());
        });
}

fn section_heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.heading(title);
    ui.label(egui::RichText::new(subtitle).color(MUTED));
}

impl eframe::App for MonitorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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

        let mut tutorial_action = None;
        if let Some(step) = self.tutorial_step {
            let (title, description) = match step {
                0 => ("Welcome to Linuxstats", "Your simple overview of live system health. This tour explains the main readings."),
                1 => ("CPU, memory, and disk", "Each card shows the percentage currently in use. The bars make it easy to compare at a glance."),
                2 => ("Top processes", "See which running apps use the most CPU. Memory is shown as a percentage of your system's total RAM."),
                _ => ("You're ready to go", "Readings update automatically every second. Fan readings appear when your hardware exposes them."),
            };
            egui::Window::new("Quick tour")
                .collapsible(false).resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_min_width(320.0);
                    ui.heading(title);
                    ui.add_space(8.0);
                    ui.label(description);
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Skip tour").clicked() { tutorial_action = Some("skip"); }
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

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.heading(egui::RichText::new("Linuxstats").size(28.0).strong());
                        ui.label(egui::RichText::new("A clear snapshot of your system").color(MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        egui::Frame::group(ui.style()).fill(CARD_BG).inner_margin(10.0).show(ui, |ui| {
                            ui.label(egui::RichText::new("RUNNING FOR").color(MUTED).small());
                            ui.label(egui::RichText::new(format_uptime(self.uptime)).strong());
                        });
                    });
                });

                ui.add_space(22.0);
                section_heading(ui, "System overview", "Current usage · refreshes every second");
                ui.add_space(10.0);
                ui.columns(3, |columns| {
                    metric_card(&mut columns[0], "PROCESSOR", self.cpu_percent, egui::Color32::from_rgb(96, 165, 250), "CPU usage");
                    metric_card(&mut columns[1], "MEMORY", self.ram_percent, egui::Color32::from_rgb(167, 139, 250), "RAM in use");
                    metric_card(&mut columns[2], "STORAGE", self.disk_percent, egui::Color32::from_rgb(52, 211, 153), "Disk space used");
                });

                ui.add_space(24.0);
                ui.horizontal(|ui| {
                    section_heading(ui, "Top processes", "Apps currently using the most CPU");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new("TOP 10").color(MUTED).small());
                    });
                });
                ui.add_space(8.0);
                egui::Frame::group(ui.style()).fill(CARD_BG).inner_margin(10.0).show(ui, |ui| {
                    if self.processes.is_empty() {
                        ui.label("No process information is available yet.");
                    } else {
                        egui::ScrollArea::horizontal().show(ui, |ui| {
                            egui::Grid::new("process_table")
                                .striped(true)
                                .num_columns(4)
                                .spacing([18.0, 8.0])
                                .show(ui, |ui| {
                                    ui.strong("Process");
                                    ui.strong("PID");
                                    ui.strong("CPU");
                                    ui.strong("Memory");
                                    ui.end_row();
                                    for (index, process) in self.processes.iter().enumerate() {
                                        let highlighted = high_fan_speed && index == 0;
                                        let name = if highlighted { format!("⚠ {}", process.name) } else { process.name.clone() };
                                        ui.label(name);
                                        ui.label(process.pid.to_string());
                                        ui.label(format!("{:.1}%", process.cpu));
                                        ui.label(format!("{:.1}%", process.memory));
                                        ui.end_row();
                                    }
                                });
                        });
                    }
                });

                ui.add_space(24.0);
                section_heading(ui, "Cooling", "Fan readings depend on your computer's hardware and drivers");
                ui.add_space(8.0);
                egui::Frame::group(ui.style()).fill(CARD_BG).inner_margin(12.0).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        draw_fan_icon(ui, self.fan_rotation);
                        ui.add_space(8.0);
                        ui.vertical(|ui| {
                            if self.fans.is_empty() {
                                ui.label(egui::RichText::new("No fan sensors detected").strong());
                                ui.label(egui::RichText::new("Your system may not expose fan speeds to Linux.").color(MUTED));
                            } else {
                                for fan in &self.fans {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(&fan.label).strong());
                                        ui.label(format!("{} RPM", fan.rpm));
                                    });
                                }
                            }
                            if high_fan_speed {
                                ui.add_space(4.0);
                                ui.colored_label(egui::Color32::from_rgb(253, 186, 116), format!("High fan speed: {fan_rpm} RPM"));
                                ui.label(egui::RichText::new("This is an advisory reading, not a diagnosis.").color(MUTED).small());
                            }
                        });
                    });
                });
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("Updated automatically every second").color(MUTED).small());
                });
                ui.add_space(12.0);
            });
        });

        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 760.0])
            .with_min_inner_size([540.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Linuxstats",
        options,
        Box::new(|_creation_context| Ok(Box::new(MonitorApp::new()))),
    )
}
