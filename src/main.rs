use std::{
    cmp::Ordering,
    env,
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

use eframe::egui;
use sysinfo::{Disks, System};

struct ProcessInfo {
    pid: u32,
    name: String,
    cpu: f32,
    memory: f32,
}

struct MonitorApp {
    sys: System,
    cpu_percent: f32,
    ram_percent: f32,
    disk_percent: f32,
    uptime: u64,
    processes: Vec<ProcessInfo>,
    last_update: Instant,
    tutorial_step: Option<usize>,
}

impl MonitorApp {
    fn new() -> Self {
        let mut app = Self {
            sys: System::new_all().expect("Could not read system information"),
            cpu_percent: 0.0,
            ram_percent: 0.0,
            disk_percent: 0.0,
            uptime: 0,
            processes: Vec::new(),
            last_update: Instant::now(),
            tutorial_step: if tutorial_is_completed() {
                None
            } else {
                Some(0)
            },
        };

        app.refresh_stats();
        app
    }

    fn refresh_stats(&mut self) {
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
}

fn tutorial_marker_path() -> Option<PathBuf> {
    if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
        if !config_home.is_empty() {
            return Some(PathBuf::from(config_home).join("linuxstats/tutorial-completed"));
        }
    }

    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join(".config/linuxstats/tutorial-completed"))
}

fn tutorial_is_completed() -> bool {
    tutorial_marker_path().map_or(false, |path| path.exists())
}

fn mark_tutorial_completed() {
    if let Some(path) = tutorial_marker_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
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
        if self.last_update.elapsed() >= Duration::from_secs(1) {
            self.refresh_stats();
            self.last_update = Instant::now();
        }

        let mut tutorial_action = None;
        if let Some(step) = self.tutorial_step {
            let (title, description) = match step {
                0 => (
                    "Welcome to Linux System Monitor",
                    "This quick tour shows you how to read your system's live CPU, memory, disk, and process information.",
                ),
                1 => (
                    "System usage",
                    "The CPU, Memory, and Disk bars show the percentage currently in use. System uptime shows how long your computer has been running.",
                ),
                2 => (
                    "Top processes",
                    "This table lists up to ten processes, sorted by CPU use. It also shows each process's memory use and process ID (PID).",
                ),
                _ => (
                    "You're ready to go",
                    "Statistics refresh automatically every second. You can close this window whenever you're done monitoring your system.",
                ),
            };

            egui::Window::new("Quick tutorial")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.heading(title);
                    ui.add_space(8.0);
                    ui.label(description);
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if ui.button("Skip tutorial").clicked() {
                            tutorial_action = Some("skip");
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let button_text = if step == 3 { "Finish" } else { "Next" };
                            if ui.button(button_text).clicked() {
                                tutorial_action = Some("next");
                            }
                            if step > 0 && ui.button("Back").clicked() {
                                tutorial_action = Some("back");
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.label(format!("Step {} of 4", step + 1));
                });
        }

        match tutorial_action {
            Some("skip") => {
                self.tutorial_step = None;
                mark_tutorial_completed();
            }
            Some("back") => {
                self.tutorial_step = self.tutorial_step.map(|step| step.saturating_sub(1));
            }
            Some("next") => {
                self.tutorial_step = match self.tutorial_step {
                    Some(0..=2) => self.tutorial_step.map(|step| step + 1),
                    _ => None,
                };
                if self.tutorial_step.is_none() {
                    mark_tutorial_completed();
                }
            }
            _ => {}
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Linux System Monitor");
            ui.label(format!("System uptime: {}", format_uptime(self.uptime)));
            ui.add_space(12.0);

            egui::Grid::new("system_stats")
                .num_columns(2)
                .spacing([16.0, 10.0])
                .show(ui, |ui| {
                    stat_row(ui, "CPU", self.cpu_percent);
                    stat_row(ui, "Memory", self.ram_percent);
                    stat_row(ui, "Disk", self.disk_percent);
                });

            ui.add_space(16.0);
            ui.heading("Top Processes");
            ui.add_space(6.0);

            egui::ScrollArea::vertical()
                .max_height(350.0)
                .show(ui, |ui| {
                    egui::Grid::new("process_table")
                        .striped(true)
                        .num_columns(4)
                        .show(ui, |ui| {
                            ui.strong("PID");
                            ui.strong("Name");
                            ui.strong("CPU %");
                            ui.strong("Memory %");
                            ui.end_row();

                            for process in &self.processes {
                                ui.label(process.pid.to_string());
                                ui.label(&process.name);
                                ui.label(format!("{:.1}", process.cpu));
                                ui.label(format!("{:.1}", process.memory));
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
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([800.0, 600.0])
            .with_min_inner_size([500.0, 400.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Linux System Monitor",
        options,
        Box::new(|_creation_context| Ok(Box::new(MonitorApp::new()))),
    )
}
