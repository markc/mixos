// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Metadata for the writable style and chrome vocabulary.

use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TunableRange {
    Number { min: f64, max: f64, step: f64 },
    Choice { variants: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Tunable {
    pub name: String,
    pub kind: &'static str,
    pub range: Option<TunableRange>,
    pub since: &'static str,
    pub aliases: Vec<String>,
    pub retired: Option<&'static str>,
    pub domains: Vec<&'static str>,
    pub group: String,
    pub order: usize,
    pub label: String,
}

pub(crate) fn style_entry(
    name: &str,
    ty: &str,
    min: f64,
    max: f64,
    variants: Option<Vec<String>>,
) -> Tunable {
    let kind = if variants.is_some() {
        "choice"
    } else {
        match ty {
            "bool" => "flag",
            "u8" => "radius",
            "StyleColour" => "colour",
            _ => "number",
        }
    };
    let range = if let Some(variants) = variants {
        Some(TunableRange::Choice { variants })
    } else if kind == "flag" {
        None
    } else {
        Some(TunableRange::Number {
            min,
            max,
            step: if ty == "u8" { 1.0 } else { 0.01 },
        })
    };
    let domains = match name {
        "widgets" | "faces" | "panel_groups" | "document_tabs" => vec!["paint", "text", "layout", "resources"],
        "body_size" | "button_link_size" | "canvas_dot_size" | "dialog_title_size" | "heading_size" | "md_bullet" | "md_code_padding_x" | "md_code_padding_y" | "md_gap" | "md_gap_after_heading" | "md_gap_before_heading" | "md_gutter" | "md_heading_line" | "md_heading_scale_1" | "md_heading_scale_2" | "md_heading_scale_3" | "md_heading_scale_4" | "md_heading_scale_5" | "md_heading_scale_6" | "md_inline_code_scale" | "md_item_gap" | "md_line" | "md_measure" | "md_quote_indent" | "md_quote_rule" | "mono_size" | "panel_pill_size" | "panel_pro_size" | "panel_section_size" | "push_label_size" | "small_size" | "tabs_pro_size" | "tabs_studio_meta_size" | "tabs_studio_name_size" | "title_size" | "value_number_size" | "value_unit_size" => vec!["text", "layout"],
        "bevels" | "widget_stroke" | "selection" | "canvas_dots" | "primary_hover_alpha" | "primary_press_alpha" | "tool_bar_rule" | "close_pressed_alpha" | "combo_chevron_alpha" | "disabled_alpha" | "field_focus_role" | "pair_active_mix" | "pair_hover_mix" | "panel_pill_hover_alpha" | "panel_pro_hover_alpha" | "popup_shadow_alpha" | "scroll_handle_alpha_active" | "scroll_handle_alpha_hot" | "scroll_handle_alpha_rest" | "scroll_track_alpha_active" | "scroll_track_alpha_hot" | "scroll_track_alpha_rest" | "slider_knob_pro_fill" | "slider_knob_pro_ring" | "slider_knob_studio_fill" | "slider_shadow" | "tabs_chevron_hover_alpha" | "tabs_pro_hover_alpha" | "tabs_studio_hover_alpha" | "toggle_mark" | "weak_alpha" => vec!["paint"],
        "scroll_bars" | "toggle" | "switch" | "knob" | "push_shape" | "secondary_button" | "menu_highlight_radius" | "radius_sm" | "radius" | "radius_lg" | "field_radius" | "panel_pro_radius" | "slider_halo_radius" | "slider_track_radius" | "tabs_studio_close_radius" | "toggle_box_radius" => vec!["layout", "paint"],
        "slider_fill" | "push_height" | "dock_width" | "dock_margin" | "options_bar" | "status_bar" | "tool_bar" | "tool_button" | "tool_margin" | "rail" | "rail_button" | "title_bar_height" | "mark" | "menu_title_height" | "menu_row_height" | "menu_row_padding_x" | "menu_row_padding_y" | "menu_min_width" | "bar_row_gap" | "bars_bar_inset" | "bars_rail_rule" | "button_icon_share" | "button_link_icon" | "button_padding_x" | "button_padding_y" | "button_pro_outline" | "button_push_padding" | "canvas_bar" | "canvas_dot_pitch" | "canvas_thumb_inset" | "canvas_thumb_min" | "caption_gap" | "caption_glyph" | "caption_width" | "checkbox_box" | "combo_chevron_half" | "combo_chevron_height" | "combo_chevron_stroke" | "combo_list_height" | "combo_width" | "dialog_above_buttons" | "dialog_above_rule" | "dialog_below_rule" | "dialog_button_gap" | "dialog_button_min" | "dialog_centre_drop" | "dialog_max_width" | "dialog_min_width" | "dialog_resize_max_share" | "divider_height" | "divider_slot" | "edge_gap" | "field_focus_width" | "field_height" | "field_padding_x" | "field_padding_y" | "focus_offset" | "focus_ring" | "hairline_width" | "icon_button" | "icon_width" | "interact" | "item_spacing_x" | "item_spacing_y" | "link_gap" | "mark_gap" | "menu_margin" | "menu_search_width" | "menu_separator_height" | "menu_title_padding_x" | "menu_title_padding_y" | "panel_card_after" | "panel_card_margin_bottom" | "panel_card_margin_collapsed" | "panel_card_margin_left" | "panel_card_margin_right" | "panel_card_margin_top" | "panel_drag_band" | "panel_drop_inset_horizontal" | "panel_drop_inset_vertical" | "panel_drop_line" | "panel_ellipsis" | "panel_ellipsis_gap" | "panel_header" | "panel_header_after" | "panel_menu_box_h" | "panel_menu_box_w" | "panel_menu_from_right" | "panel_menu_line" | "panel_menu_pitch" | "panel_pill_gap" | "panel_pill_min" | "panel_pill_pad" | "panel_pro_after" | "panel_pro_margin" | "panel_pro_min" | "panel_pro_pad" | "panel_strip" | "popup_shadow_blur" | "popup_shadow_offset_x" | "popup_shadow_offset_y" | "popup_shadow_spread" | "scroll_allocated" | "scroll_handle_min" | "scroll_margin" | "scroll_outer_margin" | "scroll_width" | "scroll_width_hot" | "search_icon" | "search_lead" | "shortcut_gap" | "slider_gradient_track" | "slider_halo_width" | "slider_height" | "slider_inset" | "slider_knob_classic_h" | "slider_knob_classic_w" | "slider_knob_pro" | "slider_knob_studio" | "slider_min_width" | "slider_plain_track" | "slider_row_gap" | "slider_width" | "space_lg" | "space_md" | "space_sm" | "space_xl" | "space_xs" | "submenu_gap" | "switch_track_h" | "switch_track_w" | "tabs_chevron" | "tabs_chevron_icon" | "tabs_chevron_inset_x" | "tabs_chevron_inset_y" | "tabs_chevron_menu_width" | "tabs_drop_outline" | "tabs_marker" | "tabs_marker_gap" | "tabs_pro_close" | "tabs_pro_close_from_right" | "tabs_pro_close_icon" | "tabs_pro_min" | "tabs_pro_pad" | "tabs_pro_title_left" | "tabs_progress" | "tabs_strip" | "tabs_studio_close" | "tabs_studio_close_from_right" | "tabs_studio_close_icon" | "tabs_studio_gap" | "tabs_studio_margin_bottom" | "tabs_studio_margin_left" | "tabs_studio_margin_right" | "tabs_studio_margin_top" | "tabs_studio_meta_gap" | "tabs_studio_name_left" | "tabs_studio_pad" | "title_bar_margin" | "title_combo_max_width" | "title_combo_min_width" | "title_control_spacing" | "title_gap" | "title_link_room" | "title_min_room" | "toggle_knob" | "toggle_knob_inset" | "toggle_label_gap" | "toggle_outline" | "toggle_tick" | "tooltip_width" | "value_unit_gap" | "value_unit_inset" | "value_unit_reserve" | "value_width" | "window_margin" | "window_shadow_blur" | "window_shadow_offset_x" | "window_shadow_offset_y" | "window_shadow_spread" => vec!["layout"],
        "animation_time" | "menu_scroll_speed" | "tooltip_delay" | "tooltip_grace" => vec!["motion"],
        "double_click_time" | "fine_scrub_rate" | "resize_corner" | "resize_edge" | "scrub_rate" | "slider_big_step" | "slider_double_click" | "slider_double_click_distance" | "slider_key_step_share" | "slider_long_press" => vec!["input"],
        "fine_span" => vec!["input", "text", "layout"],
        "hot_distance" => vec!["input", "paint"],
        "icon_mask" => vec!["paint", "resources"],
        "dark_luminance" => vec!["paint"],
        "square_radius" => vec!["layout", "paint"],
        "bar_padding_y" => vec!["layout"],
        _ => panic!("style token has no audited domains: {name}"),
    };
    let group = if name.starts_with("field_") || name.starts_with("value_") {
        "fields"
    } else if name.starts_with("scroll_") || name.starts_with("canvas_") {
        "scroll"
    } else if name.starts_with("md_") {
        "markdown"
    } else {
        "style"
    };
    Tunable {
        name: format!("style.{name}"),
        kind,
        range,
        since: "0.9.0",
        aliases: if name == "field_radius" {
            vec!["style.fieldradius".into()]
        } else {
            Vec::new()
        },
        retired: None,
        domains,
        group: group.into(),
        order: 0,
        label: format!("tune-{}", name.replace('_', "-")),
    }
}

pub fn tunables() -> &'static [Tunable] {
    static ENTRIES: std::sync::OnceLock<Vec<Tunable>> = std::sync::OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut entries = crate::family::style::declarations();
        entries.extend(crate::family::chrome::ROLES.iter().map(|role| Tunable {
            name: format!("chrome.{role}"),
            kind: "colour",
            range: None,
            since: "0.9.0",
            aliases: Vec::new(),
            retired: None,
            domains: vec!["paint"],
            group: "colours".into(),
            order: 0,
            label: format!("tune-{}", role.replace('_', "-")),
        }));
        for (order, entry) in entries.iter_mut().enumerate() {
            entry.order = order;
        }
        entries
    })
}
