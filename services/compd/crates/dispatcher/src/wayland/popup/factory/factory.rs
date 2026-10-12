use crate::state::state::DispatchWire;
use protocols::popup::state::PopupState;
use smithay::desktop::PopupManager;

pub fn new<I: DispatchWire>() -> PopupState {
    let popup = PopupManager::default();

    PopupState { state: popup }
}
