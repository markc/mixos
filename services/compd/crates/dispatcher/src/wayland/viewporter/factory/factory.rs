use crate::state::state::DispatchWire;
use protocols::viewporter::state::Viewporter;
use smithay::reexports::wayland_protocols::wp::presentation_time::server::{
    wp_presentation, wp_presentation_feedback,
};
use smithay::reexports::wayland_protocols::wp::viewporter::server::{wp_viewport, wp_viewporter};
use smithay::reexports::wayland_server::{Dispatch, DisplayHandle, GlobalDispatch};
use smithay::wayland::GlobalData;
use smithay::wayland::presentation::{PresentationData, PresentationState};
use smithay::wayland::viewporter::{ViewportState, ViewporterState};

pub fn new<I>(display_handle: &DisplayHandle) -> Viewporter
where
    I: DispatchWire,
    I: GlobalDispatch<wp_viewporter::WpViewporter, GlobalData>
        + Dispatch<wp_viewporter::WpViewporter, GlobalData>
        + Dispatch<wp_viewport::WpViewport, ViewportState>
        + 'static,
{
    // Part of loop construction. Shouldn't be here at all.
    let mut viewporter_state = ViewporterState::new::<I>(display_handle);
    Viewporter { viewporter_state }
}
