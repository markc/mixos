use crate::state::state::DispatchWire;
use protocols::xdg::dialog::state::Dialog;
use smithay::reexports::wayland_protocols::xdg::dialog::v1::server::xdg_wm_dialog_v1::XdgWmDialogV1;
use smithay::reexports::wayland_server::{Dispatch, DisplayHandle, GlobalDispatch};
use smithay::wayland::GlobalData;
use smithay::wayland::shell::xdg::dialog::{XdgDialogHandler, XdgDialogState};

/// Advertise `xdg_wm_dialog_v1`. Until this runs, a client asking whether its
/// toplevel may be a dialog finds no such global and stays silent about it.
pub fn new<I>(display_handle: &DisplayHandle) -> Dialog
where
    I: DispatchWire,
    I: XdgDialogHandler,
    I: GlobalDispatch<XdgWmDialogV1, GlobalData>,
    I: Dispatch<XdgWmDialogV1, GlobalData>,
    I: 'static,
{
    let xdg_dialog_state = XdgDialogState::new::<I>(display_handle);

    Dialog { xdg_dialog_state }
}
