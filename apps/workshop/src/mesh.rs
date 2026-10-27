// SPDX-License-Identifier: MIT OR Apache-2.0
//! Native Details canvas. Animation state belongs to egui; traffic and clocks
//! belong to inspector, so the headless Bus answers do not depend on rendering.
use crate::{label, label_with, view::UiEvent};
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2};
use inspector::{
    Engine, Row, RowKind,
    mesh::{Mesh, MeshView, Recent},
};
use std::{collections::HashMap, sync::Arc};
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

#[derive(Clone, Default)]
struct SummaryCache {
    key: Option<(u64, u64, u64)>,
    value: Arc<MeshView>,
}
impl SummaryCache {
    fn get(&mut self, mesh: &Mesh, now: u64) -> Arc<MeshView> {
        let key = (mesh.started_ms, mesh.revision(), now / 1000);
        if self.key != Some(key) {
            self.value = Arc::new(mesh.view_at(now));
            self.key = Some(key);
        }
        self.value.clone()
    }
}

#[derive(Clone, PartialEq)]
struct GeometryKey {
    rect: Rect,
    unit: f32,
    chrome: Chrome,
    nodes: Vec<(String, Option<String>, bool)>,
}
struct Geometry {
    key: GeometryKey,
    labels: Vec<NodeLabel>,
    positions: Vec<Placement>,
    dashes: Vec<Shape>,
    curves: Vec<Curve>,
    index: HashMap<String, [usize; 2]>,
}
impl Geometry {
    fn new(ui: &Ui, key: GeometryKey) -> Self {
        let padding = key.unit * 0.3;
        let labels: Vec<_> = key
            .nodes
            .iter()
            .map(|(name, ip, _)| {
                let text = |value: &str, style: egui::TextStyle| {
                    toolkit::tabs::label(
                        ui,
                        value,
                        style.resolve(ui.style()),
                        key.rect.width() * 0.25,
                        0.0,
                    )
                    .0
                };
                let name = text(name, egui::TextStyle::Body);
                let ip = ip.as_deref().map(|ip| text(ip, egui::TextStyle::Small));
                let size = Vec2::new(
                    name.size().x.max(ip.as_ref().map_or(0.0, |g| g.size().x)),
                    name.size().y + ip.as_ref().map_or(0.0, |g| g.size().y + padding),
                ) + Vec2::splat(padding * 2.0);
                NodeLabel { name, ip, size }
            })
            .collect();
        let positions = map_layout(
            key.rect,
            &labels.iter().map(|l| l.size).collect::<Vec<_>>(),
            key.unit,
        );
        let dashes = unobserved_links(&positions, key.unit, &key.chrome.palette);
        let mut curves = Vec::new();
        let mut index = HashMap::new();
        for (i, (peer, _, _)) in key.nodes.iter().enumerate().skip(1) {
            index.insert(peer.clone(), [curves.len(), curves.len() + 1]);
            for out in [true, false] {
                curves.push(Curve {
                    peer: peer.clone(),
                    out,
                    points: spoke(positions[0], positions[i], out, key.unit),
                });
            }
        }
        Self {
            key,
            labels,
            positions,
            dashes,
            curves,
            index,
        }
    }
    fn curve(&self, peer: &str, out: bool) -> Option<&Curve> {
        self.index
            .get(peer)
            .map(|indices| &self.curves[indices[usize::from(!out)]])
    }
}

fn geometry_for(ui: &Ui, key: GeometryKey) -> Arc<Geometry> {
    let id = ui.id().with("mesh-geometry");
    let cached = ui.ctx().data_mut(|d| d.get_temp::<Arc<Geometry>>(id));
    if let Some(cached) = cached.filter(|g| g.key == key) {
        cached
    } else {
        let geometry = Arc::new(Geometry::new(ui, key));
        ui.ctx().data_mut(|d| d.insert_temp(id, geometry.clone()));
        geometry
    }
}

struct NodeLabel {
    name: std::sync::Arc<egui::Galley>,
    ip: Option<std::sync::Arc<egui::Galley>>,
    size: Vec2,
}

#[derive(Clone, Copy)]
struct Placement {
    at: Pos2,
    radius: f32,
    label: Rect,
}

/// The lines x = +/- slope*y separate the centre plate (with clearance) from
/// the two sides of the map. Coordinates are relative to the centre node.
fn label_slope(width: f32, unit: f32) -> f32 {
    // The plate starts at local radius + label gap. Inflate its upper corners
    // horizontally and upwards so the separator also leaves stroke clearance.
    (width * 0.5 + unit * 0.5) / (unit * (1.3 + 0.4 - 0.25))
}

fn peer_arc_end(axes: Vec2, label_width: f32, unit: f32) -> f32 {
    let slope = label_slope(label_width, unit);
    let normal_length = Vec2::new(1.0, slope).length();
    // Leave circle clearance at either lower boundary. Solve
    // axes.x*cos(t) - slope*axes.y*sin(t) = clearance*normal_length.
    // If neither boundary fits below horizontal, reserve the entire lower half.
    let ellipse = Vec2::new(axes.x, slope * axes.y);
    let clearance = unit * (0.9 + 0.5) * normal_length;
    let phase = ellipse.y.atan2(ellipse.x);
    ((clearance / ellipse.length().max(f32::EPSILON))
        .clamp(0.0, 1.0)
        .acos()
        - phase)
        .max(0.0)
}

fn peer_angle(index: usize, count: usize, arc_end: f32) -> f32 {
    use std::f32::consts::PI;
    if count == 1 {
        return 0.0;
    }
    if count == 2 {
        return if index == 0 { PI } else { 0.0 };
    }
    // Fill the remaining arc through the top, including its two boundaries.
    // The omitted lower wedge is sized by the measured label and the ellipse.
    arc_end - index as f32 * (PI + 2.0 * arc_end) / (count - 1) as f32
}

fn map_layout(rect: Rect, labels: &[Vec2], unit: f32) -> Vec<Placement> {
    let centre = rect.center();
    let radius = unit * 0.9;
    let gap = unit * 0.4;
    let padding = unit * 0.5;
    // Bound the extent of a radially placed plate in either axis. Reserving
    // measured text margins lets the ellipse use the remaining map rectangle.
    let margin = labels.iter().skip(1).fold(Vec2::ZERO, |margin, size| {
        let half = *size * 0.5;
        let common = radius + gap + padding + half.length() * 0.5;
        margin.max(half * 1.5 + Vec2::splat(common))
    });
    let axes = (rect.size() * 0.5 - margin).max(Vec2::ZERO);
    let arc_end = peer_arc_end(axes, labels[0].x, unit);
    labels
        .iter()
        .enumerate()
        .map(|(i, size)| {
            let (at, radius, direction) = if i == 0 {
                (centre, unit * 1.3, Vec2::Y)
            } else {
                let angle = peer_angle(i - 1, labels.len() - 1, arc_end);
                let delta = Vec2::new(angle.cos() * axes.x, angle.sin() * axes.y);
                (
                    centre + delta,
                    radius,
                    delta / delta.length().max(f32::EPSILON),
                )
            };
            let half = *size * 0.5;
            let support = direction.x.abs() * half.x + direction.y.abs() * half.y;
            Placement {
                at,
                radius,
                label: Rect::from_center_size(at + direction * (radius + gap + support), *size),
            }
        })
        .collect()
}

/// Both directions use separate side ports on this node, leaving room for its
/// label below. The peer ports face inward, away from its outward label plate.
fn spoke_controls(local: Placement, peer: Placement, out: bool, unit: f32) -> [Pos2; 4] {
    let delta = peer.at - local.at;
    let radial = delta / delta.length().max(f32::EPSILON);
    let normal = Vec2::new(-radial.y, radial.x);
    let side = if delta.x < 0.0 { -1.0 } else { 1.0 };
    let lane = if out { -1.0 } else { 1.0 };
    let slope = label_slope(local.label.width(), unit);
    let port = 0.35_f32.min(0.5 / Vec2::new(1.0, slope).length());
    let along = (1.0 - port * port).sqrt();
    let a = local.at + Vec2::new(side * along, side * lane * port) * local.radius;
    let b = peer.at - radial * along * peer.radius + normal * lane * port * peer.radius;
    let lead = (local.label.width() * 0.5 + unit * 2.0)
        .min(delta.length() * 0.5)
        .min((delta.length() - local.radius - unit * 0.5).max(0.0));
    let c = a + Vec2::new(side * lead, 0.0);
    // All four controls stay on the peer's side of the centre-label wedge and
    // inward of the peer. A Bezier lies in their convex hull, so neither label
    // can be crossed, including between the sampled painter points.
    let d = c.lerp(b, 0.5);
    [a, c, d, b]
}

fn spoke(local: Placement, peer: Placement, out: bool, unit: f32) -> Vec<Pos2> {
    let [a, c, d, b] = spoke_controls(local, peer, out, unit);
    let mut points: Vec<_> = (0..=40)
        .map(|i| {
            let t = i as f32 / 40.0;
            let s = 1.0 - t;
            Pos2::new(
                s.powi(3) * a.x + 3.0 * s * s * t * c.x + 3.0 * s * t * t * d.x + t.powi(3) * b.x,
                s.powi(3) * a.y + 3.0 * s * s * t * c.y + 3.0 * s * t * t * d.y + t.powi(3) * b.y,
            )
        })
        .collect();
    if !out {
        points.reverse();
    }
    points
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

/// Peer pairs have no observed counters on this broker. Their painter accepts
/// geometry and theme roles only, so it cannot imply measured traffic.
fn unobserved_links(
    positions: &[Placement],
    unit: f32,
    palette: &toolkit::chrome::Palette,
) -> Vec<Shape> {
    let mut shapes = Vec::new();
    for i in 1..positions.len() {
        for j in i + 1..positions.len() {
            let a = positions[i];
            let b = positions[j];
            let delta = b.at - a.at;
            let normal = Vec2::new(-delta.y, delta.x) / delta.length().max(1.0);
            let control = a.at + delta * 0.5 - normal * unit * 2.0;
            let start = a.at + (control - a.at).normalized() * a.radius;
            let end = b.at + (control - b.at).normalized() * b.radius;
            let points = curve(start, end, -unit * 2.0);
            shapes.extend(Shape::dashed_line(
                &points,
                Stroke::new(unit * 0.06, palette.separator),
                unit * 0.2,
                unit * 0.5,
            ));
        }
    }
    shapes
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
    let now = mesh.now();
    let summary_id = ui.id().with("mesh-summary");
    let snapshot = ui.ctx().data_mut(|d| {
        d.get_temp_mut_or_default::<SummaryCache>(summary_id)
            .get(mesh, now)
    });
    let visible = engine.ui.row_key.as_deref() == Some("nodes:mesh")
        && ui.is_visible()
        && ui.input(|i| {
            i.viewport().visible() != Some(false) && i.viewport().minimized != Some(true)
        });
    if visible {
        // Ages and window widths advance while the visible map is quiet.
        // This schedules rendering only: no ping, topology or observe polling.
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
    }
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
    ui.horizontal_wrapped(|ui| {
        ui.colored_label(
            if snapshot.observing {
                p.accent
            } else {
                p.text_dim
            },
            label(if snapshot.observing {
                "mesh-observe-live"
            } else {
                "mesh-observe-stopped"
            }),
        );
        if let Some(age) = snapshot.last_event_age_ms {
            ui.label(label_with(
                "mesh-last-observe",
                &[("age", &(age / 1000).to_string())],
            ));
        } else {
            ui.label(label("mesh-no-observe-events"));
        }
    });
    if snapshot.ring_dropped > 0 {
        ui.colored_label(
            p.warning,
            label_with(
                "mesh-ring-dropped",
                &[("count", &snapshot.ring_dropped.to_string())],
            ),
        );
    }
    if let Some(reason) = &snapshot.observe_error {
        ui.colored_label(
            ui.visuals().error_fg_color,
            label_with("mesh-observe-unavailable", &[("reason", reason)]),
        );
    }
    if !snapshot.observing
        && mesh.observe_ticket.is_none()
        && let Some(command) = commands.get("bus.observe")
        && ui
            .add_enabled(
                (command.enabled)(engine),
                toolkit::button::PushButton::secondary(label("mesh-observe-start")),
            )
            .clicked()
    {
        events.push(UiEvent::Command("bus.observe", Origin::Ui));
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
    let padding = unit * 0.3;
    let geometry_key = GeometryKey {
        rect,
        unit,
        chrome,
        nodes: mesh
            .nodes
            .iter()
            .map(|node| (node.name.clone(), node.wg_ip.clone(), node.local))
            .collect(),
    };
    let geometry = geometry_for(ui, geometry_key);
    let positions = &geometry.positions;
    let curves = &geometry.curves;
    painter.extend(geometry.dashes.iter().cloned());
    for curve in curves {
        let count = snapshot
            .links
            .get(&curve.peer)
            .map(|link| {
                if curve.out {
                    link.counts.out[2]
                } else {
                    link.counts.input[2]
                }
            })
            .unwrap_or(0);
        painter.add(Shape::line(
            curve.points.clone(),
            Stroke::new(
                width(count, unit),
                if curve.out { p.accent } else { p.warning }.gamma_multiply(if count > 0 {
                    0.55
                } else {
                    0.2
                }),
            ),
        ));
    }
    // The midpoint of each curve is also a named keyboard control. Pointer
    // users may click anywhere along its path, without a large bounding box
    // stealing input from the node widgets.
    let mut link_activated = false;
    for curve in curves {
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
        && !positions
            .iter()
            .any(|p| p.at.distance(pointer) < p.radius * 1.25 || p.label.contains(pointer))
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
    let (shapes, moving, next_start) = ui.ctx().data_mut(|d| {
        let motion = d.get_temp_mut_or_default::<Motion>(motion_id);
        motion.feed(mesh, frame);
        motion.particles.retain(|particle| {
            particle.event.dir == "local"
                || geometry
                    .curve(
                        if particle.event.dir == "mesh_out" {
                            &particle.event.to
                        } else {
                            &particle.event.from
                        },
                        particle.event.dir == "mesh_out",
                    )
                    .is_some()
        });
        let mut shapes = Vec::new();
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
                shapes.push(Shape::circle_stroke(
                    centre,
                    unit * (1.3 + k as f32 * 1.3),
                    Stroke::new(unit * 0.1, colour.gamma_multiply((1.0 - k) as f32 * 0.8)),
                ));
            } else {
                let out = particle.event.dir == "mesh_out";
                let peer = if out {
                    &particle.event.to
                } else {
                    &particle.event.from
                };
                if let Some(curve) = geometry.curve(peer, out) {
                    let eased = if k < 0.5 {
                        2.0 * k * k
                    } else {
                        1.0 - (-2.0 * k + 2.0).powi(2) / 2.0
                    };
                    let at = point_at(&curve.points, eased as f32);
                    let radius = unit
                        * (0.16 + ((1.0 + particle.event.size as f32).log2() / 50.0).min(0.27));
                    let alpha = (k * 10.0).min((1.0 - k) * 10.0).min(1.0) as f32;
                    shapes.push(Shape::circle_filled(
                        at,
                        radius,
                        colour.gamma_multiply(alpha),
                    ));
                    shapes.push(Shape::circle_filled(
                        at,
                        radius * 0.45,
                        p.text.gamma_multiply(alpha),
                    ));
                }
            }
        }
        (shapes, moving, next_start)
    });
    painter.extend(shapes);
    // Animation runs at frame rate only while moving; delayed replies wake
    // once at launch. The separate one-second wake updates visible ages.
    if moving && visible {
        ui.ctx().request_repaint();
    } else if visible && let Some(start) = next_start {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs_f64((start - frame).max(0.0)));
    }

    let mut panel_node = 0;
    for (i, node) in mesh.nodes.iter().enumerate() {
        let Placement {
            at,
            radius,
            label: plate,
        } = positions[i];
        let hit = Rect::from_center_size(at, Vec2::splat(radius * 2.5)).union(plate);
        let response = ui.interact(hit, ui.id().with(("mesh-node", &node.name)), Sense::click());
        let accessible_name = label_with(
            if node.local {
                "mesh-this-node"
            } else {
                "mesh-peer"
            },
            &[("name", &node.name)],
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &accessible_name)
        });
        if response.hovered() || response.has_focus() {
            panel_node = i;
        }
        let border = if snapshot
            .pings
            .get(&node.name)
            .is_some_and(|ping| ping.failed)
        {
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
    // Plates paint after all links, particles and circles, keeping labels legible
    // even where an unobserved peer-pair link passes behind them.
    for (placement, text) in positions.iter().zip(&geometry.labels) {
        let plate = placement.label;
        painter.rect_filled(plate, chrome.style.square_radius, p.card);
        let name_at = egui::pos2(
            plate.center().x - text.name.size().x * 0.5,
            plate.top() + padding,
        );
        let ip_y = name_at.y + text.name.size().y + padding;
        painter.galley(name_at, text.name.clone(), p.text);
        if let Some(ip) = &text.ip {
            let at = egui::pos2(plate.center().x - ip.size().x * 0.5, ip_y);
            painter.galley(at, ip.clone(), p.text_dim);
        }
    }
    node_panel(ui, &snapshot, &mesh.nodes[panel_node]);
    for (name, link) in snapshot
        .links
        .iter()
        .filter(|(name, _)| !mesh.nodes.iter().any(|n| n.name == **name))
    {
        ui.label(label_with(
            if name == inspector::mesh::UNKNOWN {
                "mesh-unnamed"
            } else {
                "mesh-unplaced-peer"
            },
            &[
                ("name", name),
                ("out", &link.counts.out[0].to_string()),
                ("in", &link.counts.input[0].to_string()),
            ],
        ));
    }
}

fn node_panel(ui: &mut Ui, snapshot: &MeshView, node: &inspector::mesh::Node) {
    toolkit::bars::hairline(ui);
    ui.strong(label_with(
        if node.local {
            "mesh-this-node"
        } else {
            "mesh-peer"
        },
        &[("name", &node.name)],
    ));
    let link = snapshot.links.get(&node.name);
    let counts = if node.local {
        snapshot.total
    } else {
        link.map(|l| l.counts).unwrap_or_default()
    };
    egui::Grid::new("mesh-node-counts")
        .striped(true)
        .show(ui, |ui| {
            ui.label(label("mesh-window"));
            ui.label(label("mesh-out"));
            ui.label(label("mesh-in"));
            ui.end_row();
            for (i, title) in ["mesh-since-start", "mesh-one-minute", "mesh-five-minutes"]
                .iter()
                .enumerate()
            {
                ui.label(label(title));
                ui.monospace(counts.out[i].to_string());
                ui.monospace(counts.input[i].to_string());
                ui.end_row();
            }
        });
    let stats = if node.local {
        Some(&snapshot.local_stats)
    } else {
        link.map(|l| &l.stats)
    };
    if node.local {
        let local = &snapshot.local_counts;
        ui.label(label_with(
            "mesh-local-counts",
            &[
                ("total", &local[0].to_string()),
                ("one", &local[1].to_string()),
                ("five", &local[2].to_string()),
            ],
        ));
    }
    if let Some(stats) = stats {
        ui.columns(2, |columns| {
            for (column, title, entries) in [
                (0, "mesh-top-services", &stats.services),
                (1, "mesh-top-commands", &stats.commands),
            ] {
                columns[column].small(label(title));
                for (name, count) in entries {
                    columns[column].label(format!("{name} × {count}"));
                }
            }
        });
    }
    if !node.local {
        if let Some(ping) = snapshot.pings.get(&node.name) {
            let age = (ping.age_ms / 1000).to_string();
            ui.label(if ping.ok {
                label_with(
                    "mesh-ping-result",
                    &[("ms", &ping.ms.unwrap_or(0).to_string()), ("age", &age)],
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
    fn typed_summary_cache_reuses_frames_and_expires_on_revision_or_second() {
        let mut mesh = Mesh::new("alpha", "observer", clock);
        mesh.add_event_at(
            &serde_json::json!({"direction":"mesh_out","peer":"beta"}),
            clock(),
        );
        let mut cache = SummaryCache::default();
        let first = cache.get(&mesh, clock());
        assert!(Arc::ptr_eq(&first, &cache.get(&mesh, clock() + 999)));
        let second = cache.get(&mesh, clock() + 1000);
        assert!(!Arc::ptr_eq(&first, &second));
        mesh.set_observing(true, None);
        let changed = cache.get(&mesh, clock() + 1000);
        assert!(!Arc::ptr_eq(&second, &changed));
        assert!(changed.observing);
        let expired = cache.get(&mesh, clock() + 301_000);
        assert_eq!(expired.links["beta"].counts.out, [1, 0, 0]);
        assert_eq!(expired.last_event_age_ms, Some(301_000));
    }

    #[test]
    fn geometry_cache_reuses_curves_and_invalidates_rect_peers_unit_and_theme() {
        let ctx = egui::Context::default();
        toolkit::install(&ctx, &toolkit::Theme::embedded());
        let _ = ctx.run_ui(Default::default(), |ui| {
            let key = GeometryKey {
                rect: Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 480.0)),
                unit: 18.0,
                chrome: Chrome::of(ui.ctx()),
                nodes: vec![
                    ("alpha".into(), None, true),
                    ("beta".into(), None, false),
                    ("gamma".into(), None, false),
                ],
            };
            let first = geometry_for(ui, key.clone());
            assert!(Arc::ptr_eq(&first, &geometry_for(ui, key.clone())));
            assert!(!first.dashes.is_empty());
            assert!(first.curve("beta", true).unwrap().out);
            assert!(!first.curve("beta", false).unwrap().out);
            assert!(first.curve("delta", true).is_none());
            for i in 0..4 {
                let baseline = geometry_for(ui, key.clone());
                let mut changed = key.clone();
                match i {
                    0 => changed.rect.max.x += 20.0,
                    1 => changed.nodes[1].0 = "delta".into(),
                    2 => changed.unit += 1.0,
                    _ => changed.chrome.palette.accent = changed.chrome.palette.warning,
                }
                let next = geometry_for(ui, changed.clone());
                assert!(!Arc::ptr_eq(&baseline, &next));
                assert!(Arc::ptr_eq(&next, &geometry_for(ui, changed)));
            }
        });
    }
    fn layout(peers: usize, size: Vec2) -> Vec<Placement> {
        map_layout(
            Rect::from_min_size(Pos2::ZERO, size),
            &vec![Vec2::new(104.0, 40.0); peers + 1],
            18.0,
        )
    }

    #[test]
    fn one_peer_is_right_and_two_peers_span_the_available_width() {
        let size = Vec2::new(900.0, 440.0);
        let one = layout(1, size);
        assert!(one[1].at.x > one[0].at.x);
        assert_eq!(one[1].at.y, one[0].at.y);
        let two = layout(2, size);
        assert!(two[1].at.x < two[0].at.x);
        assert!(two[2].at.x > two[0].at.x);
        assert!((two[1].at.y - two[0].at.y).abs() < 0.001);
        assert_eq!(two[2].at.y, two[0].at.y);
        assert!(two[2].at.x - two[1].at.x > size.x * 0.68);
        assert!(two[1].label.left() < 18.0);
        assert!(two[2].label.right() > size.x - 18.0);
    }

    #[test]
    fn ellipse_uses_rect_aspect_with_even_angles_and_outward_labels() {
        use std::f32::consts::PI;
        let axes = Vec2::new(220.0, 150.0);
        let end = peer_arc_end(axes, 104.0, 18.0);
        assert!(end > 0.0 && end < PI * 0.5);
        assert!(peer_arc_end(axes, 208.0, 18.0) < end);
        assert!(peer_arc_end(Vec2::new(440.0, 150.0), 104.0, 18.0) > end);
        for count in 3..=64 {
            for i in 0..count {
                let angle = peer_angle(i, count, end);
                assert!((-PI - end - 0.001..=end + 0.001).contains(&angle));
                if i > 0 {
                    let step = (PI + 2.0 * end) / (count - 1) as f32;
                    assert!((angle - peer_angle(i - 1, count, end) + step).abs() < 0.001);
                }
            }
            assert!((peer_angle(0, count, end) - end).abs() < 0.001);
            assert!((peer_angle(count - 1, count, end) + PI + end).abs() < 0.001);
        }
        for size in [Vec2::new(900.0, 440.0), Vec2::new(440.0, 900.0)] {
            let rect = Rect::from_min_size(Pos2::ZERO, size);
            let nodes = layout(8, size);
            let centre = nodes[0].at;
            assert!(nodes[0].label.top() > centre.y + nodes[0].radius);
            for node in nodes.iter().skip(1) {
                assert!(rect.contains_rect(node.label));
                let radial = (node.at - centre).normalized();
                assert!((node.label.center() - node.at).normalized().dot(radial) > 0.999);
                for corner in [
                    node.label.min,
                    node.label.max,
                    node.label.left_bottom(),
                    node.label.right_top(),
                ] {
                    assert!((corner - node.at).dot(radial) > node.radius);
                }
            }
            let extent = nodes
                .iter()
                .skip(1)
                .fold(Vec2::ZERO, |max, node| max.max((node.at - centre).abs()));
            if size.x > size.y {
                assert!(extent.x > extent.y * 2.0);
            } else {
                assert!(extent.y > extent.x * 2.0);
            }
        }
    }

    #[test]
    fn measured_wedge_separates_peer_circles_and_every_spoke_control_from_labels() {
        let unit = 18.0;
        for size in [
            Vec2::new(700.0, 480.0),
            Vec2::new(900.0, 440.0),
            Vec2::new(440.0, 900.0),
        ] {
            for label_width in [52.0, 104.0, 208.0] {
                for count in 1..=64 {
                    let mut labels = vec![Vec2::new(104.0, 40.0); count + 1];
                    labels[0].x = label_width;
                    let nodes = map_layout(Rect::from_min_size(Pos2::ZERO, size), &labels, unit);
                    let local = nodes[0];
                    let slope = label_slope(local.label.width(), unit);
                    for peer in nodes.iter().skip(1) {
                        let delta = peer.at - local.at;
                        let radial = delta.normalized();
                        let side = if delta.x < 0.0 { -1.0 } else { 1.0 };
                        let separator = Vec2::new(side, -slope);
                        assert!(
                            !Rect::from_center_size(peer.at, Vec2::splat(peer.radius * 2.0))
                                .intersects(local.label)
                        );
                        for out in [true, false] {
                            for point in spoke_controls(local, *peer, out, unit) {
                                // Both are linear inequalities, hence hold for
                                // the full Bezier and every painter segment.
                                assert!(
                                    (point - local.at).dot(separator) > 0.0,
                                    "centre separator: {count} peers, {size:?}, label {label_width}"
                                );
                                assert!(
                                    (point - peer.at).dot(radial) < 0.0,
                                    "peer separator: {count} peers, {size:?}, label {label_width}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn spokes_meet_circle_edges_and_clear_the_centre_and_peer_labels() {
        for count in 1..=64 {
            let nodes = layout(count, Vec2::new(700.0, 480.0));
            let local = nodes[0];
            for peer in nodes.iter().skip(1) {
                let out = spoke(local, *peer, true, 18.0);
                let input = spoke(local, *peer, false, 18.0);
                assert!(out[0].distance(*input.last().unwrap()) > 1.0);
                assert!(out.last().unwrap().distance(input[0]) > 1.0);
                for (points, start, end) in [(&out, local, *peer), (&input, *peer, local)] {
                    assert!((points[0].distance(start.at) - start.radius).abs() < 0.001);
                    assert!((points.last().unwrap().distance(end.at) - end.radius).abs() < 0.001);
                    for segment in points.windows(2) {
                        let bounds = Rect::from_two_pos(segment[0], segment[1]);
                        assert!(
                            !bounds.intersects(local.label),
                            "centre label crossed with {count} peers"
                        );
                        assert!(
                            !bounds.intersects(peer.label),
                            "peer label crossed with {count} peers"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn peer_pair_painter_draws_only_short_unobserved_dashes_and_excludes_this_node() {
        let ctx = egui::Context::default();
        toolkit::install(&ctx, &toolkit::Theme::embedded());
        let chrome = Chrome::of(&ctx);
        let centres = [
            egui::pos2(-100.0, -100.0),
            egui::pos2(0.0, 0.0),
            egui::pos2(100.0, 0.0),
        ];
        let unit = 10.0;
        let positions = centres.map(|at| Placement {
            at,
            radius: unit * 0.9,
            label: Rect::NOTHING,
        });
        let shapes = unobserved_links(&positions, unit, &chrome.palette);
        assert!(!shapes.is_empty());
        for shape in shapes {
            let Shape::LineSegment { points, stroke } = shape else {
                panic!("peer pair must be dashed")
            };
            assert_eq!(stroke.color, chrome.palette.separator);
            assert_eq!(stroke.width, unit * 0.06);
            assert!(points[0].distance(points[1]) <= unit * 0.2 + 0.001);
            for point in points {
                assert!((-0.001..=100.001).contains(&point.x));
                assert!(point.y <= 0.001);
                assert!(point.distance(positions[0].at) > 100.0);
                assert!(point.distance(positions[1].at) >= positions[1].radius - 0.001);
                assert!(point.distance(positions[2].at) >= positions[2].radius - 0.001);
            }
        }
        assert!(unobserved_links(&positions[..2], unit, &chrome.palette).is_empty());
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
