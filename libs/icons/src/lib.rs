// SPDX-License-Identifier: MIT OR Apache-2.0
//! Lucide 1.54.0 SVGs (ISC and MIT; see NOTICE). No renderer dependency.

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BarItem {
    pub command: String,
    pub icon: String,
    pub tooltip: String,
    pub enabled: bool,
    pub toggled: bool,
}

pub const TAIL: &[&[&str]] = &[
    &["help.macros"],
    &["app.runner", "view.search"],
    &["help.shortcuts", "help.keys"],
    &["view.mode"],
];

pub fn standard(command: &str) -> bool {
    TAIL.iter().any(|ids| ids.contains(&command))
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bar {
    pub revision: u64,
    pub items: Vec<BarItem>,
}

impl Bar {
    pub fn valid(&self) -> bool {
        self.items.len() <= 128
            && self.items.iter().enumerate().all(|(i, item)| {
                !item.command.is_empty()
                    && item.command.len() <= 128
                    && item.command.bytes().all(|b| {
                        b.is_ascii_lowercase()
                            || b.is_ascii_digit()
                            || b == b'.'
                            || b == b'-'
                            || b == b'_'
                    })
                    && Icon::from_name(&item.icon).is_some()
                    && item.tooltip.len() <= 2048
                    && self.items[..i]
                        .iter()
                        .all(|other| other.command != item.command)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_round_trip_and_bars_reject_unknown_or_duplicate_items() {
        for icon in Icon::ALL {
            assert_eq!(Icon::from_name(icon.name()), Some(*icon));
        }
        let item = BarItem {
            command: "file.save".into(),
            icon: "save".into(),
            tooltip: "Save".into(),
            enabled: true,
            toggled: false,
        };
        let mut bar = Bar {
            revision: 1,
            items: vec![item.clone()],
        };
        assert!(bar.valid());
        bar.items.push(item);
        assert!(!bar.valid());
        bar.items.pop();
        bar.items[0].icon = "unknown-icon".into();
        assert!(!bar.valid());
    }
}

macro_rules! icons {
    ($($variant:ident => $file:literal),+ $(,)?) => {
        /// The icons the toolkit ships. Add an icon by copying its SVG from
        /// the pinned Lucide release into `assets/` and listing it here.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Icon { $($variant),+ }

        impl Icon {
            /// Every shipped icon.
            pub const ALL: &[Icon] = &[$(Icon::$variant),+];

            /// The Lucide name.
            pub fn name(self) -> &'static str {
                match self { $(Icon::$variant => $file),+ }
            }

            pub fn from_name(name: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|icon| icon.name() == name)
            }

            pub fn svg(self) -> &'static str {
                match self {
                    $(Icon::$variant => include_str!(concat!("../assets/", $file, ".svg"))),+
                }
            }
        }
    };
}

icons! {
    Accessibility => "accessibility",
    AppWindow => "app-window",
    ArrowDown => "arrow-down",
    ArrowLeft => "arrow-left",
    ArrowRight => "arrow-right",
    ArrowUp => "arrow-up",
    AudioLines => "audio-lines",
    BatteryMedium => "battery-medium",
    Blocks => "blocks",
    Bluetooth => "bluetooth",
    BookUser => "book-user",
    Calendar => "calendar",
    CalendarClock => "calendar-clock",
    Camera => "camera",
    Check => "check",
    ChevronDown => "chevron-down",
    ChevronLeft => "chevron-left",
    ChevronRight => "chevron-right",
    ChevronsLeft => "chevrons-left",
    ChevronsRight => "chevrons-right",
    Circle => "circle",
    CircleAlert => "circle-alert",
    CircleCheck => "circle-check",
    Clock => "clock",
    Command => "command",
    Contact => "contact",
    Copy => "copy",
    Cpu => "cpu",
    Crop => "crop",
    Disc => "disc",
    Download => "download",
    Ellipsis => "ellipsis",
    EllipsisVertical => "ellipsis-vertical",
    EthernetPort => "ethernet-port",
    ExternalLink => "external-link",
    Eye => "eye",
    EyeOff => "eye-off",
    File => "file",
    FileArchive => "file-archive",
    FileCode => "file-code",
    FileCog => "file-cog",
    FileImage => "file-image",
    FileMusic => "file-music",
    FileSpreadsheet => "file-spreadsheet",
    FileTerminal => "file-terminal",
    FileText => "file-text",
    FileType => "file-type",
    FileVideoCamera => "file-video-camera",
    Film => "film",
    Flag => "flag",
    Folder => "folder",
    FolderInput => "folder-input",
    FolderOpen => "folder-open",
    FolderPlus => "folder-plus",
    Forward => "forward",
    Globe => "globe",
    GripHorizontal => "grip-horizontal",
    Hash => "hash",
    HardDrive => "hard-drive",
    Highlighter => "highlighter",
    House => "house",
    Inbox => "inbox",
    Image => "image",
    Info => "info",
    Keyboard => "keyboard",
    Languages => "languages",
    Layers => "layers",
    LayoutDashboard => "layout-dashboard",
    LayoutGrid => "layout-grid",
    LayoutTemplate => "layout-template",
    LoaderCircle => "loader-circle",
    Mail => "mail",
    MailOpen => "mail-open",
    Menu => "menu",
    MessageSquare => "message-square",
    Mic => "mic",
    Minus => "minus",
    Monitor => "monitor",
    Moon => "moon",
    Mouse => "mouse",
    MousePointer2 => "mouse-pointer-2",
    MoveUpRight => "move-up-right",
    Music => "music",
    Package => "package",
    Palette => "palette",
    PanelLeft => "panel-left",
    PanelRight => "panel-right",
    PanelBottom => "panel-bottom",
    PanelTop => "panel-top",
    Paperclip => "paperclip",
    Pencil => "pencil",
    PenTool => "pen-tool",
    Play => "play",
    Plug => "plug",
    Plus => "plus",
    Presentation => "presentation",
    Printer => "printer",
    Redo2 => "redo-2",
    RefreshCw => "refresh-cw",
    Reply => "reply",
    ReplyAll => "reply-all",
    Rocket => "rocket",
    Save => "save",
    Scan => "scan",
    ScrollText => "scroll-text",
    Search => "search",
    Send => "send",
    Server => "server",
    Shield => "shield",
    ShieldAlert => "shield-alert",
    Settings => "settings",
    Speaker => "speaker",
    Square => "square",
    SquareDashed => "square-dashed",
    SquarePen => "square-pen",
    SquareTerminal => "square-terminal",
    Star => "star",
    Sun => "sun",
    Touchpad => "touchpad",
    Trash => "trash",
    Type => "type",
    Undo2 => "undo-2",
    Unplug => "unplug",
    User => "user",
    Users => "users",
    Volume2 => "volume-2",
    VolumeX => "volume-x",
    Wifi => "wifi",
    WifiOff => "wifi-off",
    X => "x",
    ZoomIn => "zoom-in",
    ZoomOut => "zoom-out",
}
