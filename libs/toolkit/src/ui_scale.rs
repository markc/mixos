// SPDX-License-Identifier: MIT OR Apache-2.0
//! Desktop text accessibility and spacing, applied after design resolution.
use design::ResolvedStyle;
pub use settings::model::CommonUi;

/// The named density choices; other stored values remain continuous.
pub const DENSITY_STEPS: [f64; 3] = [0.85, 1.0, 1.2];

pub fn nearest_density(value: f64) -> usize {
    DENSITY_STEPS
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (value - **a).abs().total_cmp(&(value - **b).abs()))
        .map_or(1, |(index, _)| index)
}

pub(crate) fn clamped(mut ui: CommonUi) -> CommonUi {
    ui.text_scale = finite(ui.text_scale, 0.5, 3.0);
    ui.density = finite(ui.density, 0.5, 2.0);
    ui
}

fn finite(value: f64, min: f64, max: f64) -> f64 {
    if value.is_nan() {
        1.0
    } else {
        value.clamp(min, max)
    }
}

pub(crate) fn tokens(mut style: ResolvedStyle, ui: &CommonUi) -> ResolvedStyle {
    // Desktop values only: the current summary has no per-app overrides.
    let text = ui.text_scale;
    let density = ui.density;
    style.body_size *= text;
    style.small_size *= text;
    style.heading_size *= text;
    style.mono_size *= text;
    style.button_link_size *= text;
    style.dialog_title_size *= text;
    style.panel_pill_size *= text;
    style.panel_section_size *= text;
    style.push_label_size *= text;
    style.tabs_pro_size *= text;
    style.tabs_studio_name_size *= text;
    style.tabs_studio_meta_size *= text;
    style.title_size *= text;
    style.value_number_size *= text;
    style.value_unit_size *= text;
    style.push_height *= density;
    style.dock_margin *= density;
    style.options_bar *= density;
    style.status_bar *= density;
    style.tool_bar *= density;
    style.tool_button *= density;
    style.tool_margin *= density;
    style.rail *= density;
    style.rail_button *= density;
    style.title_bar_height *= density;
    style.menu_title_height *= density;
    style.menu_row_height *= density;
    style.menu_row_padding_x *= density;
    style.menu_row_padding_y *= density;
    style.bar_row_gap *= density;
    style.bars_bar_inset *= density;
    style.button_padding_x *= density;
    style.button_padding_y *= density;
    style.button_push_padding *= density;
    style.caption_gap *= density;
    style.caption_width *= density;
    style.dialog_above_buttons *= density;
    style.dialog_above_rule *= density;
    style.dialog_below_rule *= density;
    style.dialog_button_gap *= density;
    style.divider_slot *= density;
    style.edge_gap *= density;
    style.field_height *= density;
    style.field_padding_x *= density;
    style.field_padding_y *= density;
    style.icon_button *= density;
    style.interact *= density;
    style.item_spacing_x *= density;
    style.item_spacing_y *= density;
    style.link_gap *= density;
    style.mark_gap *= density;
    style.md_code_padding_x *= density;
    style.md_code_padding_y *= density;
    style.md_gap *= density;
    style.md_gap_after_heading *= density;
    style.md_gap_before_heading *= density;
    style.md_gutter *= density;
    style.md_item_gap *= density;
    style.md_quote_indent *= density;
    style.menu_margin *= density;
    style.menu_separator_height *= density;
    style.menu_title_padding_x *= density;
    style.menu_title_padding_y *= density;
    style.panel_card_after *= density;
    style.panel_card_margin_bottom *= density;
    style.panel_card_margin_collapsed *= density;
    style.panel_card_margin_left *= density;
    style.panel_card_margin_right *= density;
    style.panel_card_margin_top *= density;
    style.panel_header *= density;
    style.panel_header_after *= density;
    style.panel_ellipsis_gap *= density;
    style.panel_pill_gap *= density;
    style.panel_pill_pad *= density;
    style.panel_strip *= density;
    style.search_lead *= density;
    style.shortcut_gap *= density;
    style.slider_height *= density;
    style.slider_row_gap *= density;
    style.space_xs *= density;
    style.space_sm *= density;
    style.space_md *= density;
    style.space_lg *= density;
    style.space_xl *= density;
    style.submenu_gap *= density;
    style.tabs_pro_pad *= density;
    style.tabs_pro_title_left *= density;
    style.tabs_marker_gap *= density;
    style.tabs_strip *= density;
    style.tabs_studio_gap *= density;
    style.tabs_studio_margin_bottom *= density;
    style.tabs_studio_margin_left *= density;
    style.tabs_studio_margin_right *= density;
    style.tabs_studio_margin_top *= density;
    style.tabs_studio_meta_gap *= density;
    style.tabs_studio_pad *= density;
    style.title_bar_margin *= density;
    style.title_gap *= density;
    style.toggle_label_gap *= density;
    style.value_unit_gap *= density;
    style.window_margin *= density;
    style.bar_padding_y *= density;
    // egui margins are whole signed bytes. Match their existing rounding
    // and saturate before conversion so large stored density cannot wrap.
    if density != 1.0 {
        for margin in [
            &mut style.menu_margin,
            &mut style.window_margin,
            &mut style.panel_card_margin_bottom,
            &mut style.panel_card_margin_collapsed,
            &mut style.panel_card_margin_left,
            &mut style.panel_card_margin_right,
            &mut style.panel_card_margin_top,
            &mut style.tabs_studio_margin_bottom,
            &mut style.tabs_studio_margin_left,
            &mut style.tabs_studio_margin_right,
            &mut style.tabs_studio_margin_top,
        ] {
            *margin = margin.round().clamp(0.0, i8::MAX as f64);
        }
    }
    style
}

/// The installed desktop values, also used by independently sized content.
pub fn of(ctx: &egui::Context) -> CommonUi {
    ctx.data(|data| data.get_temp(egui::Id::new("toolkit.common-ui")))
        .unwrap_or_default()
}

pub(crate) fn install(ctx: &egui::Context, ui: &CommonUi) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::new("toolkit.common-ui"), ui.clone()));
}
