pub mod interface;
#[cfg(feature = "desktop-dbus")]
pub mod media;
#[cfg(not(feature = "desktop-dbus"))]
#[path = "interface/media_unavailable.rs"]
pub mod media;
