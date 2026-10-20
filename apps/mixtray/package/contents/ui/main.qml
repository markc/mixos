pragma ComponentBehavior: Bound

import QtQuick

import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.plasmoid
import org.kde.plasma.plasma5support as P5Support

// The popup asks mixtrayd for one snapshot each time it opens, and again after
// every control action and on Refresh. While it is open, it also keeps one
// `mixtrayd wait` running: that call returns as soon as systemd reports a
// change to a COS unit, a file under ~/.ssh changes or an SSH probe moves on,
// and the popup starts the next one. A closed popup
// starts no new wait, so it costs nothing.
PlasmoidItem {
    id: root

    readonly property string mixtrayd: "/opt/mixos/bin/mixtrayd"

    // The last snapshot, or {} before the first one arrives.
    property var snap: ({})
    property string snapError: ""
    property bool loading: false

    // The last control action: its outcome, shown above the tabs.
    property bool acting: false
    property string actionMessage: ""
    property string actionError: ""

    readonly property bool snapReady: Object.keys(root.snap).length > 0

    function refresh(): void {
        if (exec.connectedSources.length > 0) {
            return;
        }
        root.loading = true;
        exec.connectSource(root.mixtrayd + " snapshot");
    }

    // Keep one wait running while the popup is open, from the revision the
    // current snapshot was read at.
    function follow(): void {
        if (!root.expanded || root.snap.rev === undefined) {
            return;
        }
        if (watcher.connectedSources.length > 0) {
            return;
        }
        watcher.connectSource(root.mixtrayd + " wait " + root.snap.rev);
    }

    function control(unit: string, action: string): void {
        if (root.acting) {
            return;
        }
        root.acting = true;
        root.actionMessage = "";
        root.actionError = "";
        ctl.connectSource(root.mixtrayd + " control " + unit + " " + action);
    }

    // The executable engine runs each source through sh, so every word that
    // came from a snapshot or a form is single-quoted.
    function shq(word: string): string {
        return "'" + String(word).replace(/'/g, "'\\''") + "'";
    }

    // One `mixtrayd ssh` action. A probe reports nothing here: its progress
    // and result arrive through the wait like any other change.
    function sshAction(words: var): void {
        if (root.acting) {
            return;
        }
        root.acting = words[0] !== "probe";
        root.actionMessage = "";
        root.actionError = "";
        sshCtl.connectSource(root.mixtrayd + " ssh " + words.map(w => root.shq(w)).join(" "));
    }

    function openLogs(unit: string): void {
        launch.connectSource("konsole -e journalctl -u " + root.shq(unit) + " -n 200 -f");
    }

    function connectHost(id: string): void {
        launch.connectSource("konsole -e ssh " + root.shq(id));
    }

    function editHost(path: string): void {
        launch.connectSource("xdg-open " + root.shq(path));
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
                return;
            }
            root.follow();
        }
    }

    // `mixtrayd wait` answers with a fresh snapshot once a COS unit changes,
    // or when the daemon's wait limit passes; either way the popup starts the
    // next wait. A failed wait stops following until the popup opens again,
    // so a missing daemon never turns into a busy loop.
    P5Support.DataSource {
        id: watcher

        engine: "executable"
        connectedSources: []

        onNewData: (source, data) => {
            watcher.disconnectSource(source);
            if (data["exit code"] !== 0) {
                return;
            }
            try {
                root.snap = JSON.parse(data.stdout);
            } catch (e) {
                return;
            }
            root.follow();
        }
    }

    P5Support.DataSource {
        id: ctl

        engine: "executable"
        connectedSources: []

        onNewData: (source, data) => {
            ctl.disconnectSource(source);
            root.acting = false;
            if (data["exit code"] === 0) {
                root.actionMessage = "Done";
            } else {
                // mixtrayd prints its refusal or hostd's reason on stderr.
                const text = (data.stderr || data.stdout || "").trim();
                root.actionError = text.length > 0 ? text : "The action failed";
            }
            root.refresh();
        }
    }

    P5Support.DataSource {
        id: sshCtl

        engine: "executable"
        connectedSources: []

        onNewData: (source, data) => {
            sshCtl.disconnectSource(source);
            root.acting = false;
            if (data["exit code"] !== 0) {
                const text = (data.stderr || data.stdout || "").trim();
                root.actionError = text.length > 0 ? text : "The SSH action failed";
            } else if (!source.includes(" ssh 'probe'")) {
                root.actionMessage = "Done";
            }
            root.refresh();
        }
    }

    // Konsole windows (logs, ssh sessions) and the editor: the source ends
    // when the launched program does, and nothing is read from it.
    P5Support.DataSource {
        id: launch

        engine: "executable"
        connectedSources: []

        onNewData: (source, data) => {
            launch.disconnectSource(source);
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
        acting: root.acting
        actionMessage: root.actionMessage
        actionError: root.actionError
        onRefreshRequested: root.refresh()
        onControlRequested: (unit, action) => root.control(unit, action)
        onLogsRequested: unit => root.openLogs(unit)
        onSshRequested: words => root.sshAction(words)
        onConnectRequested: id => root.connectHost(id)
        onEditRequested: path => root.editHost(path)
    }

    onExpandedChanged: {
        if (root.expanded) {
            root.refresh();
        }
    }
}
