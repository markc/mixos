use crate::state::state::{Dispatch, DispatchWire};
use smithay::wayland::xdg_activation::XdgActivationState;

pub use protocols::xdg::activation::request::{
    ActivationDetails, ActivationLog, activations, request_activation,
};

pub fn activation_state(dispatch: &mut Dispatch) -> &mut XdgActivationState {
    &mut dispatch.xdg_activation.xdg_activation
}
