use super::{
    PopupConfigure, PositionerState, ShellClient, ShellClientData, SurfaceCachedState, ToplevelConfigure,
    XdgPopupSurfaceRoleAttributes, XdgShellHandler, XdgToplevelSurfaceRoleAttributes,
};

mod wm_base;
pub use wm_base::XdgWmBaseUserData;
pub(crate) use wm_base::validate_xdg_surface_attach;

mod positioner;
pub use positioner::XdgPositionerUserData;

mod surface;
pub(in crate::wayland::shell) use surface::make_popup_handle;
pub use surface::{XdgShellSurfaceUserData, XdgSurfaceUserData};
pub(super) use surface::{get_parent, send_popup_configure, send_toplevel_configure};
