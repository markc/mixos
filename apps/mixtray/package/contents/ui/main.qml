pragma ComponentBehavior: Bound

import QtQuick

import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasmoid
import org.kde.plasma.plasma5support as P5Support

// The popup asks mixtrayd for one snapshot each time it opens, and again on
// Refresh. Nothing runs in the background, so a closed popup costs nothing.
PlasmoidItem {
    id: root

    readonly property string snapshotCommand: "/opt/mixos/bin/mixtrayd snapshot"

    // The last snapshot, or {} before the first one arrives.
    property var snap: ({})
    property string snapError: ""
    property bool loading: false

    readonly property bool snapReady: Object.keys(root.snap).length > 0

    function refresh(): void {
        if (exec.connectedSources.length > 0) {
            return;
        }
        root.loading = true;
        exec.connectSource(root.snapshotCommand);
    }

    function unitCount(): string {
        const units = root.snap.cos ? root.snap.cos.units : [];
        if (!units || units.length === 0) {
            return "No MixOS units checked yet";
        }
        const active = units.filter(u => u.state === "active").length;
        return active + " of " + units.length + " active";
    }

    P5Support.DataSource {
        id: exec

        engine: "executable"
        connectedSources: []

        onNewData: (source, data) => {
            exec.disconnectSource(source);
            root.loading = false;
            const code = data["exit code"];
            if (code !== 0) {
                root.snapError = "mixtrayd is not running or failed (exit " + code + ")";
                return;
            }
            try {
                root.snap = JSON.parse(data.stdout);
                root.snapError = "";
            } catch (e) {
                root.snapError = "mixtrayd returned unreadable output";
            }
        }
    }

    Plasmoid.backgroundHints: PlasmaCore.Types.DefaultBackground
    Plasmoid.icon: "dev.mixos.tray"
    Plasmoid.status: PlasmaCore.Types.ActiveStatus
    Plasmoid.title: "MixTray"

    toolTipMainText: "MixTray"
    toolTipSubText: root.snapReady ? root.unitCount() : "Applications, daemons, Bus, Mix and SSH"

    compactRepresentation: CompactRepresentation {
        onActivated: root.expanded = !root.expanded
    }

    fullRepresentation: FullRepresentation {
        snap: root.snap
        snapError: root.snapError
        loading: root.loading
        onRefreshRequested: root.refresh()
    }

    onExpandedChanged: {
        if (root.expanded) {
            root.refresh();
        }
    }
}
