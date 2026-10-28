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
use crate::chart::{self, Chart, Scale, Series};
use crate::{label, label_with};
use egui::{Align, Layout, RichText, ScrollArea, Sense, Ui, Vec2};
use egui_extras::{Column as Width, TableBuilder};
use toolkit::action::Origin;
use toolkit::button::PushButton;
use toolkit::dialog::{self, Choice};
use toolkit::graph::{self, Hue, Readout};
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
    chart::set_node(ui.ctx(), engine.ui.node.as_deref());
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
        if ui.available_width() < 640.0 {
            ScrollArea::horizontal()
                .min_scrolled_width(0.0)
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Keep the existing controls and ordering reachable
                        // by scrolling rather than overlapping the tab strip.
                        ui.set_width(1040.0);
                        toolbar_contents(ui, engine, commands, strings, events);
                    });
                });
        } else {
            toolbar_contents(ui, engine, commands, strings, events);
        }
    });
}

fn toolbar_contents(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
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
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, label("node")));
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

/// Monitor owns resource semantics; toolkit only knows theme-relative hues.
#[derive(Clone, Copy)]
enum Kind {
    Cpu,
    Memory,
    Disk,
    Net,
    Tcp,
}

impl Kind {
    fn hue(self) -> Hue {
        match self {
            Self::Cpu => Hue::Accent,
            Self::Memory => Hue::AccentPlus,
            Self::Disk => Hue::Warning,
            Self::Net => Hue::AccentMinus,
            Self::Tcp => Hue::Series(1),
        }
    }

    fn format(self) -> fn(f64) -> String {
        match self {
            Self::Cpu | Self::Disk => percent,
            Self::Memory => bytes,
            Self::Net => rate,
            Self::Tcp => tcp_rate,
        }
    }
}

/// The section list's title and one-line figure for `key`.
fn section_text(s: &Snapshot, key: &str) -> Option<(String, String, Kind)> {
    if key == "tcp" {
        let value = |v: Option<f64>| v.map(tcp_rate).unwrap_or_else(|| "—".into());
        return Some((
            label("tcp"),
            format!("↓ {}  ↑ {}", value(s.tcp.in_segs), value(s.tcp.out_segs)),
            Kind::Tcp,
        ));
    }
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
    if key == "tcp" {
        let rx = rings.get("tcp.in_segs");
        let tx = rings.get("tcp.out_segs");
        let top = scale(&rx, 1.0).max(scale(&tx, 1.0));
        return (rx, top);
    }
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
    if ui.available_width() < 640.0 {
        egui::Panel::top("compact-sections").show(ui, |ui| {
            let keys = model::sections(Some(s));
            let names: Vec<_> = keys
                .iter()
                .map(|key| section_text(s, key).map(|v| v.0).unwrap_or_default())
                .collect();
            let mut selected = keys
                .iter()
                .position(|key| *key == engine.ui.section)
                .unwrap_or(0);
            if toolkit::combo::show(
                ui,
                "compact-section",
                &mut selected,
                &names,
                Some(ui.available_width()),
            )
            .accessible_name(label("performance"))
            .changed()
            {
                events.push(UiEvent::Section(keys[selected].clone()));
            }
        });
    } else {
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
                                let format = |index, value, age| {
                                    let name = if key == "tcp" {
                                        label(if index == 0 { "receive" } else { "send" })
                                    } else {
                                        String::new()
                                    };
                                    chart::readout(&name, value, age as f64, kind.format())
                                };
                                let second = engine.rings.get("tcp.out_segs");
                                let series: Vec<_> = if key == "tcp" {
                                    vec![
                                        (values.as_slice(), kind.hue()),
                                        (second.as_slice(), Hue::Series(2)),
                                    ]
                                } else {
                                    vec![(values.as_slice(), kind.hue())]
                                };
                                graph::lines(
                                    ui,
                                    Vec2::new(64.0, 36.0),
                                    &series,
                                    max,
                                    Some(Readout {
                                        samples: vitals::history::WINDOW,
                                        interval_s: s.interval_s,
                                        format: &format,
                                    }),
                                );
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
    }
    egui::CentralPanel::default()
        .frame(bars::dock_frame(ui.ctx()))
        .show(ui, |ui| {
            let width = ui.available_width();
            let compact = ui.ctx().content_rect().width() < 640.0;
            let output = ScrollArea::new([compact, true]).auto_shrink(false).show(ui, |ui| {
                if compact {
                    ui.set_max_width(width);
                }
                let key = engine.ui.section.as_str();
                if key == "cpu" {
                    cpu(ui, engine, s);
                } else if key == "memory" {
                    memory(ui, engine, s);
                } else if key == "tcp" {
                    tcp(ui, engine, s);
                } else if let Some(name) = key.strip_prefix("disk:") {
                    disk(ui, engine, s, name);
                } else if let Some(name) = key.strip_prefix("net:") {
                    net(ui, engine, s, name);
                }
            });
            ui.ctx().data_mut(|data| {
                data.insert_temp(
                    egui::Id::unique("monitor-performance-layout"),
                    PerformanceLayout {
                        content_size: output.content_size,
                        pane_size: output.inner_rect.size(),
                    },
                )
            });
        });
}

/// Geometry of the last rendered Performance pane, for layout regression checks.
#[derive(Clone, Copy, Debug)]
pub struct PerformanceLayout {
    pub content_size: Vec2,
    pub pane_size: Vec2,
}

pub fn performance_layout(ctx: &egui::Context) -> Option<PerformanceLayout> {
    ctx.data_mut(|data| data.get_temp(egui::Id::unique("monitor-performance-layout")))
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
    chart::show(
        ui,
        ("cpu", &engine.ui.node),
        graph_size(ui, 0.3),
        Chart {
            series: &[Series {
                legend_label: None,
                name: &label("utilisation"),
                hue: Kind::Cpu.hue(),
                values: &engine.rings.get("cpu.busy"),
            }],
            samples: vitals::history::WINDOW,
            interval_s: s.interval_s,
            scale: Scale::Percent,
            format: percent,
        },
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
                let format = |_, value, age| {
                    chart::readout(
                        &label_with("graph-core", &[("core", &core.cpu.to_string())]),
                        value,
                        age as f64,
                        percent,
                    )
                };
                graph::line(
                    ui,
                    cell,
                    &values,
                    100.0,
                    Kind::Cpu.hue(),
                    Some(Readout {
                        samples: vitals::history::WINDOW,
                        interval_s: s.interval_s,
                        format: &format,
                    }),
                );
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
    chart::show(
        ui,
        ("memory", &engine.ui.node),
        graph_size(ui, 0.4),
        Chart {
            series: &[Series {
                legend_label: None,
                name: &label("in-use"),
                hue: Kind::Memory.hue(),
                values: &engine.rings.get("mem.used"),
            }],
            samples: vitals::history::WINDOW,
            interval_s: s.interval_s,
            scale: Scale::Capacity(m.total as f64),
            format: bytes,
        },
    );
    ui.add_space(8.0);
    let share = if m.total > 0 {
        m.used as f64 / m.total as f64
    } else {
        0.0
    };
    graph::meter(ui, ui.available_width(), share, Kind::Memory.hue());
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
    chart::show(
        ui,
        ("disk-busy", &engine.ui.node, name),
        graph_size(ui, 0.3),
        Chart {
            series: &[Series {
                legend_label: None,
                name: &label("active-time"),
                hue: Kind::Disk.hue(),
                values: &busy,
            }],
            samples: vitals::history::WINDOW,
            interval_s: s.interval_s,
            scale: Scale::Percent,
            format: percent,
        },
    );
    ui.add_space(8.0);
    let read = engine.rings.get(&format!("disk.{name}.read"));
    let write = engine.rings.get(&format!("disk.{name}.write"));
    let top = scale(&read, 1024.0 * 1024.0).max(scale(&write, 1024.0 * 1024.0));
    let title = format!(
        "{} / {} · {} · 60 s",
        label("read"),
        label("write"),
        rate(top)
    );
    chart::show_titled(
        ui,
        ("disk-rates", &engine.ui.node, name),
        &title,
        graph_size(ui, 0.25),
        Chart {
            series: &[
                Series {
                    legend_label: None,
                    name: &label("read"),
                    hue: Kind::Disk.hue(),
                    values: &read,
                },
                Series {
                    legend_label: None,
                    name: &label("write"),
                    hue: Hue::AccentPlus,
                    values: &write,
                },
            ],
            samples: vitals::history::WINDOW,
            interval_s: s.interval_s,
            scale: Scale::Rate(top),
            format: rate,
        },
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
                    graph::meter(ui, 160.0, share, Kind::Disk.hue());
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
    let title = format!(
        "{} / {} · {} · 60 s",
        label("receive"),
        label("send"),
        rate(top)
    );
    chart::show_titled(
        ui,
        ("net-rates", &engine.ui.node, name),
        &title,
        graph_size(ui, 0.45),
        Chart {
            series: &[
                Series {
                    legend_label: None,
                    name: &label("receive"),
                    hue: Kind::Net.hue(),
                    values: &rx,
                },
                Series {
                    legend_label: None,
                    name: &label("send"),
                    hue: Hue::AccentPlus,
                    values: &tx,
                },
            ],
            samples: vitals::history::WINDOW,
            interval_s: s.interval_s,
            scale: Scale::Rate(top),
            format: rate,
        },
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

fn count(value: f64) -> String {
    format!("{value:.0}")
}

fn tcp_rate(value: f64) -> String {
    label_with("tcp-rate", &[("value", &format!("{value:.1}"))])
}

fn tcp_chart(
    ui: &mut Ui,
    engine: &Engine,
    s: &Snapshot,
    title: &str,
    fields: &[(&str, &str, Hue)],
    gauge: bool,
    annotation: Option<&dyn Fn(usize) -> Option<String>>,
) {
    let values: Vec<_> = fields
        .iter()
        .map(|(key, _, _)| engine.rings.get(key))
        .collect();
    let labels: Vec<_> = fields.iter().map(|(_, key, _)| label(key)).collect();
    let legend_labels: Vec<_> = fields
        .iter()
        .map(|(_, key, _)| {
            label(match *key {
                "tcp-active-opens" => "tcp-active-short",
                "tcp-passive-opens" => "tcp-passive-short",
                "tcp-out-rsts" => "tcp-rsts-short",
                "tcp-estab-resets" => "tcp-resets-short",
                key => key,
            })
        })
        .collect();
    let series: Vec<_> = fields
        .iter()
        .zip(&values)
        .zip(&labels)
        .zip(&legend_labels)
        .map(|((((_, _, hue), values), name), legend_label)| Series {
            legend_label: Some(legend_label),
            name,
            hue: *hue,
            values,
        })
        .collect();
    let peak = values.iter().map(|v| scale(v, 1.0)).fold(1.0, f64::max);
    let chart = Chart {
        series: &series,
        samples: vitals::history::WINDOW,
        interval_s: s.interval_s,
        scale: Scale::Count(peak),
        format: if gauge { count } else { tcp_rate },
    };
    let size = Vec2::new(ui.available_width(), 130.0);
    let heading = label(if title == "tcp-opens-resets" {
        "tcp-opens-resets-short"
    } else {
        title
    });
    match annotation {
        Some(annotation) => chart::show_annotated(
            ui,
            (title, &engine.ui.node),
            &heading,
            size,
            chart,
            annotation,
        ),
        None => chart::show_titled(ui, (title, &engine.ui.node), &heading, size, chart),
    }
}

fn tcp(ui: &mut Ui, engine: &Engine, s: &Snapshot) {
    heading(ui, &label("tcp"), None);
    let retrans = engine.rings.get("tcp.retrans_segs");
    let out = engine.rings.get("tcp.out_segs");
    let ratio = |steps: usize| {
        let r = *retrans.get(retrans.len().checked_sub(steps + 1)?)?;
        let o = *out.get(out.len().checked_sub(steps + 1)?)?;
        let value = model::retransmit_percent(r, o)
            .map(percent)
            .unwrap_or_else(|| "—".into());
        Some(format!("{}: {value}", label("tcp-retransmit-percent")))
    };
    ui.columns(2, |col| {
        tcp_chart(
            &mut col[0],
            engine,
            s,
            "tcp-segments",
            &[
                ("tcp.in_segs", "receive", Hue::Series(1)),
                ("tcp.out_segs", "send", Hue::Series(2)),
            ],
            false,
            None,
        );
        tcp_chart(
            &mut col[1],
            engine,
            s,
            "tcp-retransmits",
            &[("tcp.retrans_segs", "tcp-retransmits", Hue::Warning)],
            false,
            Some(&ratio),
        );
    });
    ui.columns(2, |col| {
        tcp_chart(
            &mut col[0],
            engine,
            s,
            "tcp-established",
            &[("tcp.established", "tcp-established", Hue::Series(3))],
            true,
            None,
        );
        tcp_chart(
            &mut col[1],
            engine,
            s,
            "tcp-opens-resets",
            &[
                ("tcp.active_opens", "tcp-active-opens", Hue::Series(1)),
                ("tcp.passive_opens", "tcp-passive-opens", Hue::Series(2)),
                ("tcp.out_rsts", "tcp-out-rsts", Hue::Series(3)),
                ("tcp.estab_resets", "tcp-estab-resets", Hue::Series(4)),
            ],
            false,
            None,
        );
    });
    let socks = &s.tcp.sockstat;
    ui.horizontal_wrapped(|ui| {
        for (key, value) in [
            ("tcp-inuse-v4", socks.inuse),
            ("tcp-inuse-v6", s.tcp.sockstat6.inuse),
            ("tcp-orphans", socks.orphan),
            ("tcp-timewait", socks.tw),
            ("tcp-allocated", socks.alloc),
            ("tcp-memory-pages", socks.mem),
        ] {
            ui.small(format!(
                "{}: {}",
                label(key),
                value.map(|v| v.to_string()).unwrap_or_else(|| "—".into())
            ));
        }
    });
    ui.add_space(6.0);
    panel::section_label(ui, &label("tcp-connections"));
    if let Some(error) = &engine.connections_error {
        ui.colored_label(ui.visuals().error_fg_color, error);
    } else if let Some(connections) = &engine.connections {
        if connections.rows.is_empty() {
            ui.weak(label("tcp-no-connections"));
            return;
        }
        // Widths share the pane and clip long peers/users, with the full
        // endpoint available on hover. No horizontal growth at narrow widths.
        let row_height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
        let width = (ui.available_width() - ui.spacing().item_spacing.x * 7.0).max(1.0);
        TableBuilder::new(ui)
            .id_salt("tcp-connections")
            .striped(true)
            .cell_layout(Layout::left_to_right(Align::Center))
            .vscroll(false)
            .column(Width::remainder().at_least(80.0).clip(true))
            .column(Width::exact(width * 0.09).clip(true))
            .column(Width::exact(width * 0.13).clip(true))
            .column(Width::exact(width * 0.12).clip(true))
            .column(Width::exact(width * 0.12).clip(true))
            .column(Width::exact(width * 0.09).clip(true))
            .column(Width::exact(width * 0.09).clip(true))
            .column(Width::exact(width * 0.10).clip(true))
            .header(row_height, |mut header| {
                for (key, tooltip) in [
                    ("tcp-remote", "tcp-remote"),
                    ("tcp-port", "tcp-local-port"),
                    ("state", "state"),
                    ("tcp-rx", "tcp-rx-full"),
                    ("tcp-tx", "tcp-tx-full"),
                    ("tcp-rtt", "tcp-rtt-full"),
                    ("tcp-retx", "tcp-retransmits"),
                    ("column-user", "column-user"),
                ] {
                    header.col(|ui| {
                        ui.strong(label(key)).on_hover_text(label(tooltip));
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, connections.rows.len().min(20), |mut row| {
                    let c = &connections.rows[row.index()];
                    row.col(|ui| {
                        ui.label(c.peer()).on_hover_text(format!(
                            "{}:{} → {}",
                            c.local,
                            c.local_port,
                            c.peer()
                        ));
                    });
                    row.col(|ui| {
                        ui.label(c.local_port.to_string());
                    });
                    row.col(|ui| {
                        ui.label(label(&format!("tcp-state-{}", c.state)));
                    });
                    row.col(|ui| right(ui, c.rx_bps.map(rate).unwrap_or_else(|| "—".into())));
                    row.col(|ui| right(ui, c.tx_bps.map(rate).unwrap_or_else(|| "—".into())));
                    row.col(|ui| {
                        right(
                            ui,
                            c.rtt_us
                                .map(|v| {
                                    label_with(
                                        "tcp-rtt-ms",
                                        &[("value", &format!("{:.1}", f64::from(v) / 1000.0))],
                                    )
                                })
                                .unwrap_or_else(|| "—".into()),
                        )
                    });
                    row.col(|ui| {
                        right(
                            ui,
                            c.retransmits
                                .map(|v| v.to_string())
                                .unwrap_or_else(|| "—".into()),
                        )
                    });
                    row.col(|ui| {
                        ui.add_space(ui.spacing().item_spacing.x);
                        ui.label(&c.user).on_hover_text(&c.user);
                    });
                });
            });
    } else {
        ui.weak(label("no-readings"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    #[test]
    fn tcp_table_separates_numbers_from_users_and_scrolls_with_the_page() {
        for theme in [
            toolkit::Theme::embedded(),
            toolkit::Theme::for_context(design::DesignContext::revision_one()),
        ] {
            let mut engine = Engine::new(label);
            engine.connections = Some(model::Connections {
                total: 20,
                count: 20,
                rows: (0..20)
                    .map(|i| model::Connection {
                        remote: "192.0.2.1".into(),
                        remote_port: 443,
                        state: "established".into(),
                        retransmits: Some(1000 + i),
                        user: format!("user-{i}"),
                        ..model::Connection::default()
                    })
                    .collect(),
            });
            let mut h = Harness::builder()
                .with_context_setup(move |ctx| toolkit::install(ctx, &theme))
                .with_size(egui::vec2(780.0, 1000.0))
                .with_max_steps(120)
                .build_ui_state(
                    move |ui, offset| {
                        let page = ScrollArea::vertical()
                            .show(ui, |ui| tcp(ui, &engine, &Snapshot::default()));
                        *offset = page.state.clamped_offset().y;
                    },
                    0.0,
                );
            h.run();
            for i in 0..20 {
                let number = h.get_by_label(&format!("{}", 1000 + i)).rect();
                let user = h.get_by_label(&format!("user-{i}")).rect();
                assert!(
                    user.left() - number.right() >= h.ctx.global_style().spacing.item_spacing.x,
                    "Retx and User need a visible gap"
                );
            }
            // A shorter viewport must scroll the containing page even when
            // the wheel is over a table row, and expose the twentieth row.
            h.set_size(egui::vec2(780.0, 520.0));
            h.run();
            h.hover_at(h.get_by_label("user-0").rect().center());
            h.event(egui::Event::MouseWheel {
                source: egui::MouseWheelSource::Unknown,
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -500.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
            h.run();
            assert!(
                *h.state() > 100.0,
                "the page must own scrolling over the table"
            );
            let last = h.get_by_label("user-19").rect();
            assert!(
                last.bottom() <= 520.0,
                "the twentieth row must be reachable"
            );
        }
    }
}
