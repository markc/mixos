// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! The egui rendering of the engine's state. [`view`] reads the engine and
//! returns what the person did as [`UiEvent`]s; the shell applies them after
//! the frame, so drawing never mutates state and a snapshot needs no Bus.
//!
//! The window: the toolkit title bar; a tool bar with the two pages, the
//! node chooser and, on the processes page, the filter, the tree switch and
//! End task; the page; and a status bar. The processes page is one
//! virtualised table. The performance page is a list of sections (CPU,
//! memory, each disk and network interface, each with a small graph) beside
//! the chosen section's graphs and figures.
use crate::graph::{self, Kind};
use crate::{label, label_with};
use egui::{Align, Layout, RichText, ScrollArea, Sense, Ui, Vec2};
use egui_extras::{Column as Width, TableBuilder};
use toolkit::action::Origin;
use toolkit::button::PushButton;
use toolkit::dialog::{self, Choice};
use toolkit::titlebar::{self, Action};
use toolkit::{Icon, Registry, Strings, bars, combo, field, icons, panel, tooltip};
use vitals::format::{bytes, mhz, percent, rate, uptime};
use vitals::history::scale;
use vitals::model::{self, Snapshot};
use vitals::{Column, Dialog, Engine, Tab};

/// One interaction from this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum UiEvent {
    Command(&'static str, Origin),
    CloseMacros,
    Tab(Tab),
    Section(String),
    Sort(Column),
    Filter(String),
    Select(Option<u32>),
    Node(Option<String>),
    CloseDialog,
    FilterFocused,
    /// The title bar's light/dark toggle.
    SetMode(design::Mode),
}

const ICON: f32 = 14.0;

/// Draw the whole window into `ui`.
pub fn view(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
) -> Vec<UiEvent> {
    let mut events = Vec::new();
    let title = format!("{} — {}", label("title"), engine.host());
    // The icon bar: refresh, then the standard tail.
    let fired = titlebar::show_bar(
        ui,
        &title,
        Some(Icon::Monitor),
        commands,
        engine,
        strings,
        &[Action::new("file.refresh", Icon::RefreshCw)],
    );
    // The toggle goes to the mode opposite the one this frame drew, so a
    // click reported twice cannot flip it back.
    let mut mode_set = false;
    for (id, origin) in fired {
        if id == "view.mode" {
            if !mode_set {
                commands.note(id, origin);
                let target = if engine.effective_theme().1 == design::Mode::Dark {
                    design::Mode::Light
                } else {
                    design::Mode::Dark
                };
                events.push(UiEvent::SetMode(target));
                mode_set = true;
            }
        } else {
            events.push(UiEvent::Command(id, origin));
        }
    }
    status_bar(ui, engine);
    tool_bar(ui, engine, commands, strings, &mut events);
    egui::CentralPanel::default()
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| match engine.ui.tab {
            Tab::Processes => processes(ui, engine, &mut events),
            Tab::Performance => performance(ui, engine, &mut events),
        });
    if let Some(dialog) = engine.ui.dialog {
        let title = match dialog {
            Dialog::About => "about",
            Dialog::Shortcuts => "shortcuts",
        };
        let id = egui::Id::unique("dialog").with(title);
        let shown = dialog::Dialog::new(id, label(title))
            .buttons(vec![Choice::new(label("done"), dialog::Role::Default)])
            .show(ui.ctx(), |ui| match dialog {
                Dialog::About => {
                    ui.add(egui::Label::new(label("about-body")).wrap());
                }
                Dialog::Shortcuts => commands.shortcut_grid(ui, strings),
            });
        if shown.chosen.is_some() || shown.dismissed {
            dialog::Dialog::reset(ui.ctx(), id);
            events.push(UiEvent::CloseDialog);
        }
    }
    if toolkit::macros::show_app(ui.ctx(), "monitor", engine.ui.macros_open) {
        events.push(UiEvent::CloseMacros);
    }
    toolkit::titlebar::edges(ui);
    events
}

fn status_bar(ui: &mut Ui, engine: &Engine) {
    bars::status(ui, |ui| {
        let (icon, colour) = if engine.connected && engine.error.is_none() {
            (Icon::Plug, ui.visuals().weak_text_color())
        } else {
            (Icon::Unplug, ui.visuals().error_fg_color)
        };
        ui.add(icons::image_of(ui.ctx(), icon, ICON, colour));
        ui.small(&engine.status);
        if let Some(error) = &engine.error {
            ui.small(RichText::new(error).color(ui.visuals().error_fg_color));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(up) = engine.snapshot.as_ref().and_then(|s| s.uptime_s) {
                ui.small(format!("{} {}", label("uptime"), uptime(up)));
            }
            if let Some(t) = &engine.processes {
                ui.small(label_with(
                    "processes-count",
                    &[
                        ("count", &t.count.to_string()),
                        ("total", &t.total.to_string()),
                    ],
                ));
            }
        });
    });
}

fn tool_bar(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    bars::options(ui, |ui| {
        for tab in Tab::ALL {
            if toolkit::selectable(ui, engine.ui.tab == tab, label(tab.name())).clicked() {
                events.push(UiEvent::Tab(tab));
            }
        }
        bars::divider(ui, ui.available_height().min(18.0));
        ui.add(icons::image_of(
            ui.ctx(),
            Icon::Server,
            ICON,
            ui.visuals().weak_text_color(),
        ));
        let mut options = vec![label("this-node")];
        options.extend(engine.peers.iter().cloned());
        if let Some(node) = &engine.ui.node
            && !engine.peers.contains(node)
        {
            options.push(node.clone());
        }
        let mut chosen = engine
            .ui
            .node
            .as_ref()
            .and_then(|n| options.iter().skip(1).position(|o| o == n).map(|i| i + 1))
            .unwrap_or(0);
        let before = chosen;
        let response = combo::show(ui, "node", &mut chosen, &options, Some(150.0));
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, label("node")));
        if chosen != before {
            events.push(UiEvent::Node((chosen > 0).then(|| options[chosen].clone())));
        }
        if engine.ui.paused {
            ui.label(RichText::new(label("paused")).color(ui.visuals().warn_fg_color));
        }
        if engine.ui.tab != Tab::Processes {
            return;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if let Some(command) = commands.get("process.end") {
                let enabled = (command.enabled)(engine);
                let mut tip = tooltip::for_command(ui.ctx(), command, strings);
                if engine.ui.node.is_some() {
                    tip = label("not-local");
                }
                let response = ui
                    .add_enabled(enabled, PushButton::secondary(strings.get(command.label)))
                    .on_hover_text(tip.clone())
                    .on_disabled_hover_text(tip);
                if response.clicked() {
                    events.push(UiEvent::Command(command.id, Origin::Ui));
                }
            }
            if toolkit::selectable(ui, engine.ui.tree, label("tree")).clicked() {
                events.push(UiEvent::Command("view.tree", Origin::Ui));
            }
            let mut filter = engine.ui.filter.clone();
            let search = field::search(ui, &mut filter, &label("search"), 220.0);
            if search.changed() {
                events.push(UiEvent::Filter(filter));
            }
            if engine.ui.focus_filter {
                search.request_focus();
                events.push(UiEvent::FilterFocused);
            }
        });
    });
}

fn right(ui: &mut Ui, text: impl Into<RichText>) {
    let text: RichText = text.into();
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(text);
    });
}

fn processes(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    let Some(table) = &engine.processes else {
        ui.centered_and_justified(|ui| ui.weak(label("no-readings")));
        return;
    };
    let row_height = ui.text_style_height(&egui::TextStyle::Body) + 6.0;
    let columns: [(Column, &str); 6] = [
        (Column::Name, "column-name"),
        (Column::Pid, "column-pid"),
        (Column::User, "column-user"),
        (Column::Cpu, "column-cpu"),
        (Column::Memory, "column-memory"),
        (Column::Io, "column-io"),
    ];
    let weak = ui.visuals().weak_text_color();
    TableBuilder::new(ui)
        .id_salt("processes")
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Width::initial(240.0).at_least(120.0).clip(true))
        .column(Width::initial(70.0).at_least(50.0).clip(true))
        .column(Width::initial(100.0).at_least(60.0).clip(true))
        .column(Width::initial(70.0).at_least(50.0).clip(true))
        .column(Width::initial(90.0).at_least(60.0).clip(true))
        .column(Width::initial(90.0).at_least(60.0).clip(true))
        .column(Width::remainder().at_least(120.0).clip(true))
        .header(row_height, |mut header| {
            for (column, key) in columns {
                header.col(|ui| {
                    let arrow = if engine.ui.sort == column {
                        if engine.ui.reverse { " ↑" } else { " ↓" }
                    } else {
                        ""
                    };
                    let text = RichText::new(format!("{}{arrow}", label(key))).strong();
                    if ui.add(egui::Button::new(text).frame(false)).clicked() {
                        events.push(UiEvent::Sort(column));
                    }
                });
            }
            header.col(|ui| {
                ui.strong(label("column-command"));
            });
        })
        .body(|body| {
            body.rows(row_height, table.rows.len(), |mut row| {
                let p = &table.rows[row.index()];
                row.set_selected(engine.ui.selected == Some(p.pid));
                row.col(|ui| {
                    if let Some(depth) = p.depth {
                        ui.add_space(depth as f32 * 14.0);
                    }
                    ui.label(&p.name);
                });
                row.col(|ui| right(ui, p.pid.to_string()));
                row.col(|ui| {
                    ui.label(&p.user);
                });
                row.col(|ui| right(ui, format!("{:.1}", p.cpu)));
                row.col(|ui| right(ui, bytes(p.memory as f64)));
                row.col(|ui| match p.io() {
                    Some(io) => right(ui, rate(io)),
                    None => right(ui, RichText::new("—").color(weak)),
                });
                row.col(|ui| {
                    ui.label(RichText::new(&p.cmdline).color(weak));
                });
                if row.response().clicked() {
                    events.push(UiEvent::Select(Some(p.pid)));
                }
            });
        });
}

/// The section list's title and one-line figure for `key`.
fn section_text(s: &Snapshot, key: &str) -> Option<(String, String, Kind)> {
    if key == "cpu" {
        let speed = average_mhz(s)
            .map(|m| format!("  {}", mhz(m)))
            .unwrap_or_default();
        return Some((
            label("cpu"),
            format!("{}{speed}", percent(s.cpu.busy)),
            Kind::Cpu,
        ));
    }
    if key == "memory" {
        let m = &s.memory;
        let share = if m.total > 0 {
            m.used as f64 * 100.0 / m.total as f64
        } else {
            0.0
        };
        return Some((
            label("memory"),
            format!(
                "{} / {} ({:.0}%)",
                bytes(m.used as f64),
                bytes(m.total as f64),
                share
            ),
            Kind::Memory,
        ));
    }
    if let Some(name) = key.strip_prefix("disk:") {
        let d = s.disks.iter().find(|d| d.name == name)?;
        return Some((
            format!("{} {name}", label("disk")),
            d.busy.map(percent).unwrap_or_else(|| "—".into()),
            Kind::Disk,
        ));
    }
    let name = key.strip_prefix("net:")?;
    let i = s.net.iter().find(|i| i.name == name)?;
    Some((
        format!("{} {name}", label("network")),
        format!(
            "↓ {}  ↑ {}",
            rate(i.rx_bps.unwrap_or(0.0)),
            rate(i.tx_bps.unwrap_or(0.0))
        ),
        Kind::Net,
    ))
}

fn average_mhz(s: &Snapshot) -> Option<u64> {
    let speeds: Vec<u64> = s.cpu.per_core.iter().filter_map(|c| c.mhz).collect();
    (!speeds.is_empty()).then(|| speeds.iter().sum::<u64>() / speeds.len() as u64)
}

/// The series and scale a section's small graph shows.
fn section_series(engine: &Engine, key: &str) -> (Vec<f64>, f64) {
    let rings = &engine.rings;
    if key == "cpu" {
        return (rings.get("cpu.busy"), 100.0);
    }
    if key == "memory" {
        let total = engine.snapshot.as_ref().map_or(1, |s| s.memory.total) as f64;
        return (rings.get("mem.used"), total.max(1.0));
    }
    if let Some(name) = key.strip_prefix("disk:") {
        return (rings.get(&format!("disk.{name}.busy")), 100.0);
    }
    let name = key.strip_prefix("net:").unwrap_or_default();
    let rx = rings.get(&format!("net.{name}.rx"));
    let max = scale(&rx, 1024.0);
    (rx, max)
}

fn performance(ui: &mut Ui, engine: &Engine, events: &mut Vec<UiEvent>) {
    let Some(s) = &engine.snapshot else {
        ui.centered_and_justified(|ui| ui.weak(label("no-readings")));
        return;
    };
    egui::Panel::left("sections")
        .resizable(false)
        .default_size(250.0)
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| {
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                for key in model::sections(Some(s)) {
                    let Some((title, figure, kind)) = section_text(s, &key) else {
                        continue;
                    };
                    let selected = engine.ui.section == key;
                    let response = ui
                        .horizontal(|ui| {
                            let (values, max) = section_series(engine, &key);
                            graph::line(ui, Vec2::new(64.0, 36.0), &values, max, kind);
                            ui.vertical(|ui| {
                                // Both drawn every frame; either click chooses.
                                let title = toolkit::selectable(
                                    ui,
                                    selected,
                                    RichText::new(&title).strong(),
                                )
                                .clicked();
                                let figure = ui
                                    .add(
                                        egui::Label::new(RichText::new(&figure).small())
                                            .sense(Sense::click()),
                                    )
                                    .clicked();
                                title || figure
                            })
                            .inner
                        })
                        .inner;
                    if response {
                        events.push(UiEvent::Section(key.clone()));
                    }
                    ui.add_space(6.0);
                }
            });
        });
    egui::CentralPanel::default()
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| {
            ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let key = engine.ui.section.as_str();
                if key == "cpu" {
                    cpu(ui, engine, s);
                } else if key == "memory" {
                    memory(ui, engine, s);
                } else if let Some(name) = key.strip_prefix("disk:") {
                    disk(ui, engine, s, name);
                } else if let Some(name) = key.strip_prefix("net:") {
                    net(ui, engine, s, name);
                }
            });
        });
}

fn heading(ui: &mut Ui, title: &str, detail: Option<&str>) {
    ui.horizontal(|ui| {
        ui.heading(title);
        if let Some(detail) = detail {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(detail).weak());
            });
        }
    });
}

/// A grid of figure name and value pairs, `columns` pairs wide.
fn figures(ui: &mut Ui, id: &str, pairs: &[(String, String)], columns: usize) {
    egui::Grid::new(id)
        .num_columns(columns * 2)
        .spacing([18.0, 6.0])
        .show(ui, |ui| {
            for (i, (name, value)) in pairs.iter().enumerate() {
                ui.label(RichText::new(name).weak());
                ui.label(RichText::new(value).strong());
                if (i + 1) % columns == 0 {
                    ui.end_row();
                }
            }
        });
}

fn graph_size(ui: &Ui, share: f32) -> Vec2 {
    Vec2::new(
        ui.available_width(),
        (ui.available_height() * share).clamp(120.0, 320.0),
    )
}

fn cpu(ui: &mut Ui, engine: &Engine, s: &Snapshot) {
    heading(ui, &label("cpu"), s.cpu.model.as_deref());
    ui.small(format!("{} % · 60 s", label("utilisation")));
    graph::line(
        ui,
        graph_size(ui, 0.3),
        &engine.rings.get("cpu.busy"),
        100.0,
        Kind::Cpu,
    );
    ui.add_space(8.0);
    // Each logical processor, in as square a grid as the width allows.
    let n = s.cpu.per_core.len();
    if n > 1 {
        ui.small(label("cores"));
        let width = ui.available_width();
        let columns = ((n as f32).sqrt().ceil() as usize)
            .clamp(1, 16)
            .max((width / 160.0) as usize)
            .min(n);
        let cell = Vec2::new((width / columns as f32) - 6.0, 44.0);
        egui::Grid::new("cores").spacing([6.0, 6.0]).show(ui, |ui| {
            for (i, core) in s.cpu.per_core.iter().enumerate() {
                let values = engine.rings.get(&format!("cpu.core.{}.busy", core.cpu));
                graph::line(ui, cell, &values, 100.0, Kind::Cpu).on_hover_text(format!(
                    "CPU {} · {}",
                    core.cpu,
                    percent(core.busy)
                ));
                if (i + 1) % columns == 0 {
                    ui.end_row();
                }
            }
        });
        ui.add_space(8.0);
    }
    let mut pairs = vec![
        (label("utilisation"), percent(s.cpu.busy)),
        (label("user"), percent(s.cpu.user)),
        (label("system"), percent(s.cpu.system)),
        (label("iowait"), percent(s.cpu.iowait)),
        (label("cores"), s.cpu.cores.to_string()),
    ];
    if let Some(m) = average_mhz(s) {
        pairs.push((label("speed"), mhz(m)));
    }
    if let Some(l) = &s.load {
        pairs.push((
            label("load"),
            format!("{:.2}  {:.2}  {:.2}", l.one, l.five, l.fifteen),
        ));
    }
    pairs.push((label("tasks"), s.tasks.total.to_string()));
    if let Some(up) = s.uptime_s {
        pairs.push((label("uptime"), uptime(up)));
    }
    figures(ui, "cpu-figures", &pairs, 3);
}

fn memory(ui: &mut Ui, engine: &Engine, s: &Snapshot) {
    let m = &s.memory;
    heading(ui, &label("memory"), Some(&bytes(m.total as f64)));
    ui.small(format!("{} · 60 s", label("in-use")));
    graph::line(
        ui,
        graph_size(ui, 0.4),
        &engine.rings.get("mem.used"),
        (m.total as f64).max(1.0),
        Kind::Memory,
    );
    ui.add_space(8.0);
    let share = if m.total > 0 {
        m.used as f64 / m.total as f64
    } else {
        0.0
    };
    graph::meter(ui, ui.available_width(), share, Kind::Memory);
    ui.add_space(8.0);
    let mut pairs = vec![
        (label("in-use"), bytes(m.used as f64)),
        (label("available"), bytes(m.available as f64)),
        (label("cached"), bytes(m.cached as f64)),
        (label("total"), bytes(m.total as f64)),
    ];
    if s.swap.total > 0 {
        pairs.push((
            label("swap"),
            format!(
                "{} / {}",
                bytes(s.swap.used as f64),
                bytes(s.swap.total as f64)
            ),
        ));
    }
    figures(ui, "memory-figures", &pairs, 2);
}

fn disk(ui: &mut Ui, engine: &Engine, s: &Snapshot, name: &str) {
    let Some(d) = s.disks.iter().find(|d| d.name == name) else {
        return;
    };
    heading(ui, &format!("{} {name}", label("disk")), d.model.as_deref());
    ui.small(format!("{} % · 60 s", label("active-time")));
    let busy = engine.rings.get(&format!("disk.{name}.busy"));
    graph::line(ui, graph_size(ui, 0.3), &busy, 100.0, Kind::Disk);
    ui.add_space(8.0);
    let read = engine.rings.get(&format!("disk.{name}.read"));
    let write = engine.rings.get(&format!("disk.{name}.write"));
    let top = scale(&read, 1024.0 * 1024.0).max(scale(&write, 1024.0 * 1024.0));
    ui.small(format!(
        "{} / {} · {}",
        label("read"),
        label("write"),
        rate(top)
    ));
    graph::lines(
        ui,
        graph_size(ui, 0.25),
        &[
            (read.as_slice(), Kind::Disk),
            (write.as_slice(), Kind::Memory),
        ],
        top,
    );
    ui.add_space(8.0);
    let pairs = vec![
        (
            label("active-time"),
            d.busy.map(percent).unwrap_or_else(|| "—".into()),
        ),
        (label("read"), rate(d.read_bps.unwrap_or(0.0))),
        (label("write"), rate(d.write_bps.unwrap_or(0.0))),
    ];
    figures(ui, "disk-figures", &pairs, 3);
    // The filesystems on this disk's partitions.
    let mounted: Vec<_> = s
        .filesystems
        .iter()
        .filter(|f| f.device.contains(name))
        .collect();
    if !mounted.is_empty() {
        ui.add_space(10.0);
        panel::section_label(ui, &label("filesystems"));
        for f in mounted {
            ui.horizontal(|ui| {
                ui.label(&f.mount);
                ui.weak(&f.kind);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(format!(
                        "{} / {}",
                        bytes(f.used as f64),
                        bytes(f.total as f64)
                    ));
                    let share = if f.total > 0 {
                        f.used as f64 / f.total as f64
                    } else {
                        0.0
                    };
                    graph::meter(ui, 160.0, share, Kind::Disk);
                });
            });
        }
    }
}

fn net(ui: &mut Ui, engine: &Engine, s: &Snapshot, name: &str) {
    let Some(i) = s.net.iter().find(|i| i.name == name) else {
        return;
    };
    heading(
        ui,
        &format!("{} {name}", label("network")),
        i.state.as_deref(),
    );
    let rx = engine.rings.get(&format!("net.{name}.rx"));
    let tx = engine.rings.get(&format!("net.{name}.tx"));
    let top = scale(&rx, 1024.0).max(scale(&tx, 1024.0));
    ui.small(format!(
        "{} / {} · {}",
        label("receive"),
        label("send"),
        rate(top)
    ));
    graph::lines(
        ui,
        graph_size(ui, 0.45),
        &[(rx.as_slice(), Kind::Net), (tx.as_slice(), Kind::Memory)],
        top,
    );
    ui.add_space(8.0);
    let mut pairs = vec![
        (label("receive"), rate(i.rx_bps.unwrap_or(0.0))),
        (label("send"), rate(i.tx_bps.unwrap_or(0.0))),
        (
            label("state"),
            i.state.clone().unwrap_or_else(|| "—".into()),
        ),
    ];
    if let Some(speed) = i.speed_mbps {
        pairs.push((label("link-speed"), format!("{speed} Mbit/s")));
    }
    pairs.push((format!("{} ↓", label("total")), bytes(i.rx_bytes as f64)));
    pairs.push((format!("{} ↑", label("total")), bytes(i.tx_bytes as f64)));
    figures(ui, "net-figures", &pairs, 3);
}
