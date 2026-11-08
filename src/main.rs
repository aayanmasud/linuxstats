use std::{
    cmp::Ordering,
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

        let disks =
            Disks::new_with_refreshed_list().expect("Could not read disk information");

        let mut total_disk_space = 0u64;
        let mut used_disk_space = 0u64;

        for disk in disks.iter() {
            total_disk_space += disk.total_space();
            used_disk_space += disk.total_space().saturating_sub(disk.available_space());
        }

        self.disk_percent = if total_disk_space > 0 {
            (used_disk_space as f32 / total_disk_space as f32 * 100.0)
                .clamp(0.0, 100.0)
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

        processes.sort_by(|a, b| {
            b.cpu
                .partial_cmp(&a.cpu)
                .unwrap_or(Ordering::Equal)
        });

        self.processes = processes.into_iter().take(10).collect();
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

    ui.add(
        egui::ProgressBar::new(percent / 100.0)
            .text(format!("{percent:.1}%")),
    );

    ui.end_row();
}

impl eframe::App for MonitorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.last_update.elapsed() >= Duration::from_secs(1) {
            self.refresh_stats();
            self.last_update = Instant::now();
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
