// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native Details canvas. Animation state belongs to egui; traffic and clocks
//! belong to inspector, so the headless Bus answers do not depend on rendering.
use crate::{label, label_with, view::UiEvent};
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use inspector::{Engine, Row, RowKind, mesh::Recent};
use serde_json::Value;
use toolkit::{Registry, Strings, action::Origin, chrome::Chrome};

const TRAVEL_S: f64 = 1.1;
const RIPPLE_S: f64 = 0.9;

#[derive(Clone)]
struct Particle {
    event: Recent,
    start: f64,
}
#[derive(Clone, Default)]
struct Motion {
    last_seq: u64,
    started_ms: u64,
    particles: Vec<Particle>,
    last_frame: f64,
}
impl Motion {
    fn feed(&mut self, mesh: &inspector::mesh::Mesh, frame: f64) {
        let now = mesh.now();
        let fresh_start = self.started_ms != mesh.started_ms;
        if fresh_start {
            *self = Self {
                started_ms: mesh.started_ms,
                ..Default::default()
            };
        }
        let first_load = fresh_start || self.last_frame + 1.0 < frame;
        let mut fresh: Vec<_> = mesh
            .recent
            .iter()
            .filter(|e| {
                e.seq > self.last_seq
                    && now.saturating_sub(e.t)
                        <= if first_load {
                            3000
                        } else {
                            inspector::mesh::RECENT_MS
                        }
            })
            .cloned()
            .collect();
        // Keep the source order for equal timestamps, as in mesh.js animate().
        fresh.sort_by_key(|e| e.t);
        if let Some(last) = mesh.recent.back() {
            self.last_seq = last.seq;
        }
        if let (Some(first), Some(last)) = (fresh.first(), fresh.last()) {
            let t0 = first.t;
            let span = last.t.saturating_sub(t0).max(1) as f64;
            for event in fresh {
                let delay = event.t.saturating_sub(t0) as f64 / span * span.min(1000.0) / 1000.0;
                let reply = event.message_type == "response" && event.dir != "local";
                self.particles.push(Particle {
                    event,
                    start: frame + delay + if reply { TRAVEL_S } else { 0.0 },
                });
            }
        }
        self.particles.retain(|p| {
            frame
                < p.start
                    + if p.event.dir == "local" {
                        RIPPLE_S
                    } else {
                        TRAVEL_S
                    }
        });
        if self.particles.len() > inspector::mesh::RECENT_MAX {
            self.particles
                .drain(..self.particles.len() - inspector::mesh::RECENT_MAX);
        }
        self.last_frame = frame;
    }
}

#[derive(Clone)]
struct Curve {
    peer: String,
    out: bool,
    points: Vec<Pos2>,
}

fn curve(a: Pos2, b: Pos2, bow: f32) -> Vec<Pos2> {
    let delta = b - a;
    let normal = Vec2::new(-delta.y, delta.x) / delta.length().max(1.0);
    let control = a + delta * 0.5 + normal * bow;
    (0..=40)
        .map(|i| {
            let t = i as f32 / 40.0;
            Pos2::new(
                (1.0 - t).powi(2) * a.x + 2.0 * (1.0 - t) * t * control.x + t * t * b.x,
                (1.0 - t).powi(2) * a.y + 2.0 * (1.0 - t) * t * control.y + t * t * b.y,
            )
        })
        .collect()
}

/// Sample by arc length, matching SVG getPointAtLength rather than letting
/// a quadratic's varying derivative change the dot's speed along a spoke.
fn point_at(points: &[Pos2], fraction: f32) -> Pos2 {
    let length: f32 = points.windows(2).map(|s| s[0].distance(s[1])).sum();
    let mut remaining = length * fraction.clamp(0.0, 1.0);
    for segment in points.windows(2) {
        let distance = segment[0].distance(segment[1]);
        if remaining <= distance {
            return segment[0].lerp(segment[1], remaining / distance.max(f32::EPSILON));
        }
        remaining -= distance;
    }
    *points.last().unwrap()
}
fn number(value: &Value, key: &str) -> u64 {
    value[key].as_u64().unwrap_or(0)
}
fn width(count: u64, unit: f32) -> f32 {
    unit * (0.08 + ((1.0 + count as f32).log2() * 0.075).min(0.38))
}
fn colour(event: &Recent, palette: &toolkit::chrome::Palette) -> Color32 {
    if matches!(event.outcome.as_str(), "rejected" | "dropped") {
        palette.danger
    } else if event.dir == "mesh_out" {
        palette.accent
    } else if event.dir == "mesh_in" {
        palette.warning
    } else {
        palette.text_dim
    }
}

pub fn view(
    ui: &mut Ui,
    engine: &Engine,
    commands: &Registry<Engine>,
    strings: &Strings,
    events: &mut Vec<UiEvent>,
) {
    let mesh = &engine.workshop.mesh;
    let chrome = Chrome::of(ui.ctx());
    let p = &chrome.palette;
    ui.horizontal(|ui| {
        toolkit::panel::section_label(ui, &label("mesh-map"));
        if let Some(command) = commands.get("mesh.ping")
            && ui
                .add_enabled(
                    (command.enabled)(engine),
                    toolkit::button::PushButton::secondary(strings.get(command.label)),
                )
                .clicked()
        {
            events.push(UiEvent::Command("mesh.ping", Origin::Ui));
        }
    });
    if !mesh.observing {
        let reason = mesh
            .observe_error
            .clone()
            .unwrap_or_else(|| label("mesh-waiting"));
        ui.colored_label(
            ui.visuals().error_fg_color,
            label_with("mesh-observe-unavailable", &[("reason", &reason)]),
        );
    }
    if let Some(reason) = &mesh.nodes_error {
        ui.label(label_with("mesh-nodes-unavailable", &[("reason", reason)]));
    }
    ui.small(label("mesh-scope"));
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(p.accent, label("mesh-legend-out"));
        ui.colored_label(p.warning, label("mesh-legend-in"));
        ui.weak(label("mesh-legend-local"));
        ui.colored_label(p.danger, label("mesh-legend-danger"));
    });
    if mesh.nodes.is_empty() {
        ui.label(label("mesh-loading"));
        return;
    }

    let unit = ui.text_style_height(&egui::TextStyle::Body);
    let size = Vec2::new(
        ui.available_width(),
        (ui.available_height() * 0.62).max(unit * 16.0),
    );
    let (_, rect) = ui.allocate_space(size);
    toolkit::canvas::surround(ui, rect);
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    let centre = rect.center();
    let peers = mesh.nodes.len().saturating_sub(1);
    let positions: Vec<_> = mesh
        .nodes
        .iter()
        .enumerate()
        .map(|(i, _)| {
            if i == 0 {
                centre
            } else {
                let angle = -std::f32::consts::FRAC_PI_2
                    + (i - 1) as f32 * std::f32::consts::TAU / peers.max(1) as f32;
                centre
                    + Vec2::new(
                        angle.cos() * rect.width() * 0.36,
                        angle.sin() * rect.height() * 0.32,
                    )
            }
        })
        .collect();
    // These links exist, but this broker cannot observe peer-to-peer traffic.
    for i in 1..positions.len() {
        for j in i + 1..positions.len() {
            let points = curve(positions[i], positions[j], -unit * 2.0);
            painter.extend(Shape::dashed_line(
                &points,
                Stroke::new(unit * 0.06, p.separator),
                unit * 0.2,
                unit * 0.5,
            ));
        }
    }
    let snapshot = mesh.snapshot();
    let links = snapshot["links"].as_array().unwrap();
    let mut curves = Vec::new();
    for (i, node) in mesh.nodes.iter().enumerate().skip(1) {
        let link = links.iter().find(|link| link["b"] == node.name);
        for out in [true, false] {
            let points = if out {
                curve(centre, positions[i], unit * 1.8)
            } else {
                curve(positions[i], centre, unit * 1.8)
            };
            let count = link
                .map(|link| number(link, if out { "out_5m" } else { "in_5m" }))
                .unwrap_or(0);
            painter.add(Shape::line(
                points.clone(),
                Stroke::new(
                    width(count, unit),
                    if out { p.accent } else { p.warning }.gamma_multiply(if count > 0 {
                        0.55
                    } else {
                        0.2
                    }),
                ),
            ));
            curves.push(Curve {
                peer: node.name.clone(),
                out,
                points,
            });
        }
    }
    // The midpoint of each curve is also a named keyboard control. Pointer
    // users may click anywhere along its path, without a large bounding box
    // stealing input from the node widgets.
    let mut link_activated = false;
    for curve in &curves {
        let midpoint = point_at(&curve.points, 0.5);
        let response = ui.interact(
            Rect::from_center_size(midpoint, Vec2::splat(unit)),
            ui.id().with(("mesh-link", &curve.peer, curve.out)),
            Sense::click(),
        );
        let direction = label(if curve.out { "mesh-out" } else { "mesh-in" });
        let name = label_with(
            "mesh-link-filter",
            &[("name", &curve.peer), ("direction", &direction)],
        );
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &name));
        if response.has_focus() {
            painter.circle_stroke(midpoint, unit * 0.6, Stroke::new(unit * 0.08, p.accent));
        }
        if response.clicked() {
            link_activated = true;
            events.push(UiEvent::TrafficFilter(curve.peer.clone()));
            events.push(UiEvent::Dock(true));
        }
    }
    if !link_activated
        && ui.rect_contains_pointer(rect)
        && ui.input(|i| i.pointer.primary_clicked())
        && let Some(pointer) = ui.input(|i| i.pointer.interact_pos())
        && !positions.iter().any(|p| p.distance(pointer) < unit * 1.7)
    {
        let closest = curves
            .iter()
            .map(|c| {
                (
                    c,
                    c.points
                        .iter()
                        .map(|p| p.distance(pointer))
                        .fold(f32::INFINITY, f32::min),
                )
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((curve, distance)) = closest
            && distance < unit * 0.55
        {
            events.push(UiEvent::TrafficFilter(curve.peer.clone()));
            events.push(UiEvent::Dock(true));
        }
    }
    let motion_id = ui.id().with("mesh-motion");
    let frame = ui.input(|i| i.time);
    let mut motion = ui
        .ctx()
        .data_mut(|d| d.get_temp::<Motion>(motion_id).unwrap_or_default());
    motion.feed(mesh, frame);
    motion.particles.retain(|particle| {
        particle.event.dir == "local"
            || curves.iter().any(|c| {
                c.out == (particle.event.dir == "mesh_out")
                    && c.peer
                        == if c.out {
                            &particle.event.to
                        } else {
                            &particle.event.from
                        }
                        .as_str()
            })
    });
    let mut moving = false;
    let mut next_start: Option<f64> = None;
    for particle in &motion.particles {
        let duration = if particle.event.dir == "local" {
            RIPPLE_S
        } else {
            TRAVEL_S
        };
        let k = (frame - particle.start) / duration;
        if k < 0.0 {
            next_start = Some(next_start.unwrap_or(f64::INFINITY).min(particle.start));
            continue;
        }
        if k >= 1.0 {
            continue;
        }
        moving = true;
        let colour = colour(&particle.event, p);
        if particle.event.dir == "local" {
            painter.circle_stroke(
                centre,
                unit * (1.3 + k as f32 * 1.3),
                Stroke::new(unit * 0.1, colour.gamma_multiply((1.0 - k) as f32 * 0.8)),
            );
        } else {
            let out = particle.event.dir == "mesh_out";
            let peer = if out {
                &particle.event.to
            } else {
                &particle.event.from
            };
            if let Some(curve) = curves.iter().find(|c| c.out == out && &c.peer == peer) {
                let eased = if k < 0.5 {
                    2.0 * k * k
                } else {
                    1.0 - (-2.0 * k + 2.0).powi(2) / 2.0
                };
                let at = point_at(&curve.points, eased as f32);
                let radius =
                    unit * (0.16 + ((1.0 + particle.event.size as f32).log2() / 50.0).min(0.27));
                let alpha = (k * 10.0).min((1.0 - k) * 10.0).min(1.0) as f32;
                painter.circle_filled(at, radius, colour.gamma_multiply(alpha));
                painter.circle_filled(at, radius * 0.45, p.text.gamma_multiply(alpha));
            }
        }
    }
    // No idle repaint loop. Delayed replies wake once at launch; only dots
    // currently in flight request another animation frame.
    if moving {
        ui.ctx().request_repaint();
    } else if let Some(start) = next_start {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64((start - frame).max(0.0)));
    }
    ui.ctx().data_mut(|d| d.insert_temp(motion_id, motion));

    let mut panel_node = 0;
    for (i, node) in mesh.nodes.iter().enumerate() {
        let at = positions[i];
        let radius = unit * if node.local { 1.3 } else { 0.9 };
        let hit = Rect::from_center_size(at, Vec2::splat(radius * 2.5));
        let response = ui.interact(hit, ui.id().with(("mesh-node", &node.name)), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &node.name)
        });
        if response.hovered() || response.has_focus() {
            panel_node = i;
        }
        let border = if mesh.ping_failed(&node.name) {
            p.danger
        } else if response.has_focus() || response.hovered() {
            p.accent
        } else {
            p.field_border
        };
        painter.circle_filled(at, radius, p.card);
        painter.circle_stroke(
            at,
            radius,
            Stroke::new(unit * if response.has_focus() { 0.16 } else { 0.1 }, border),
        );
        painter.circle_filled(
            at,
            radius * 0.25,
            if node.local { p.accent } else { p.text_dim },
        );
        painter.text(
            at + Vec2::new(0.0, radius + unit * 0.7),
            egui::Align2::CENTER_CENTER,
            &node.name,
            egui::TextStyle::Body.resolve(ui.style()),
            p.text,
        );
        if let Some(ip) = &node.wg_ip {
            painter.text(
                at + Vec2::new(0.0, radius + unit * 1.65),
                egui::Align2::CENTER_CENTER,
                ip,
                egui::TextStyle::Small.resolve(ui.style()),
                p.text_dim,
            );
        }
        if response.clicked() {
            let key = format!("node:{}", node.name);
            if let Some(row) = inspector::engine::find(&engine.tree(), &key) {
                events.push(UiEvent::Select(row.clone()));
            } else {
                // Topology can arrive before service discovery; still select
                // the native node key, which discovery will fill in shortly.
                events.push(UiEvent::Select(Row {
                    key,
                    kind: RowKind::Node(node.name.clone()),
                    expanded: false,
                    children: Vec::new(),
                }));
            }
        }
    }
    node_panel(ui, &snapshot, &mesh.nodes[panel_node]);
    if let Some(unknown) = links.iter().find(|l| l["b"] == inspector::mesh::UNKNOWN) {
        ui.label(label_with(
            "mesh-unnamed",
            &[
                ("out", &number(unknown, "out_msgs").to_string()),
                ("in", &number(unknown, "in_msgs").to_string()),
            ],
        ));
    }
}

fn node_panel(ui: &mut Ui, snapshot: &Value, node: &inspector::mesh::Node) {
    toolkit::bars::hairline(ui);
    ui.strong(label_with(
        if node.local {
            "mesh-this-node"
        } else {
            "mesh-peer"
        },
        &[("name", &node.name)],
    ));
    let links = snapshot["links"].as_array().unwrap();
    let link = links.iter().find(|l| l["b"] == node.name);
    let total = |key| {
        if node.local {
            links
                .iter()
                .fold(0_u64, |sum, l| sum.saturating_add(number(l, key)))
        } else {
            link.map(|l| number(l, key)).unwrap_or(0)
        }
    };
    egui::Grid::new("mesh-node-counts")
        .striped(true)
        .show(ui, |ui| {
            ui.label(label("mesh-window"));
            ui.label(label("mesh-out"));
            ui.label(label("mesh-in"));
            ui.end_row();
            for (title, out, input) in [
                ("mesh-since-start", "out_msgs", "in_msgs"),
                ("mesh-one-minute", "out_1m", "in_1m"),
                ("mesh-five-minutes", "out_5m", "in_5m"),
            ] {
                ui.label(label(title));
                ui.monospace(total(out).to_string());
                ui.monospace(total(input).to_string());
                ui.end_row();
            }
        });
    let stats = if node.local {
        Some(&snapshot["local"])
    } else {
        link
    };
    if node.local {
        let local = &snapshot["local"];
        ui.label(label_with(
            "mesh-local-counts",
            &[
                ("total", &number(local, "msgs").to_string()),
                ("one", &number(local, "msgs_1m").to_string()),
                ("five", &number(local, "msgs_5m").to_string()),
            ],
        ));
    }
    if let Some(stats) = stats {
        ui.columns(2, |columns| {
            for (column, title, key) in [
                (0, "mesh-top-services", "top_services"),
                (1, "mesh-top-commands", "top_commands"),
            ] {
                columns[column].small(label(title));
                for entry in stats[key].as_array().into_iter().flatten() {
                    columns[column].label(format!(
                        "{} × {}",
                        entry["name"].as_str().unwrap_or_default(),
                        number(entry, "count")
                    ));
                }
            }
        });
    }
    let ping = &snapshot["probe"]["results"][&node.name];
    if !node.local {
        if ping.is_object() {
            let age = (number(ping, "age_ms") / 1000).to_string();
            ui.label(if ping["ok"] == true {
                label_with(
                    "mesh-ping-result",
                    &[("ms", &number(ping, "ms").to_string()), ("age", &age)],
                )
            } else {
                label_with("mesh-ping-failed", &[("age", &age)])
            });
        } else {
            ui.small(label("mesh-not-pinged"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn clock() -> u64 {
        100_000
    }
    #[test]
    fn animation_preserves_spacing_delays_replies_and_finishes() {
        let mut mesh = inspector::mesh::Mesh::new("alpha", "observer", clock);
        for (t, kind) in [
            (99_100, "request"),
            (99_350, "request"),
            (99_350, "response"),
        ] {
            mesh.add_event_at(&serde_json::json!({"direction":"mesh_out","peer":"beta","ts":inspector::mesh::iso_ms(t),"message_type":kind}),100_000);
        }
        let mut motion = Motion::default();
        motion.feed(&mesh, 10.0);
        assert_eq!(motion.particles.len(), 3);
        assert_eq!(motion.particles[0].start, 10.0);
        assert_eq!(motion.particles[1].start, 10.25);
        assert!((motion.particles[2].start - 11.35).abs() < 0.0001);
        motion.feed(&mesh, 10.5);
        assert_eq!(motion.particles.len(), 3);
        motion.feed(&mesh, 13.0);
        assert!(motion.particles.is_empty());
    }

    #[test]
    fn animation_is_bounded_and_does_not_replay_an_idle_tally() {
        let mut mesh = inspector::mesh::Mesh::new("alpha", "observer", clock);
        let mut motion = Motion::default();
        for frame in [0.0, 0.1] {
            for _ in 0..inspector::mesh::RECENT_MAX {
                mesh.add_event_at(
                    &serde_json::json!({"direction":"mesh_in","peer":"beta"}),
                    clock(),
                );
            }
            motion.feed(&mesh, frame);
            assert_eq!(motion.particles.len(), inspector::mesh::RECENT_MAX);
        }
        motion.feed(&mesh, 5.0);
        assert!(motion.particles.is_empty());
        motion.feed(&mesh, 10.0);
        assert!(motion.particles.is_empty());
    }
}
