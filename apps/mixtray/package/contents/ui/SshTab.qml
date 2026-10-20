pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents3
import org.kde.plasma.extras as PlasmaExtras

import "components"

// SSH: one row per host fragment in ~/.ssh/hosts/, with Connect, Test, Edit
// and Move to Trash; the trash, with Restore and a two-step permanent delete;
// and the keys in ~/.ssh/keys/. Every change goes through mixtrayd, and the
// tab redraws from the snapshot the popup's wait brings back, so probe results
// and edits made elsewhere show up on their own.
Item {
    id: root

    required property var snap
    required property bool acting

    // `mixtrayd ssh` arguments, e.g. ["trash", "alpha"].
    signal sshRequested(var words)
    signal connectRequested(string id)
    signal editRequested(string path)

    readonly property var ssh: root.snap.ssh ?? ({})
    readonly property var hosts: Array.isArray(root.ssh.hosts) ? root.ssh.hosts : []
    readonly property var trash: Array.isArray(root.ssh.trash) ? root.ssh.trash : []
    readonly property var keys: Array.isArray(root.ssh.keys) ? root.ssh.keys : []
    readonly property var usableKeys: root.keys.filter(k => k.error === "")
    readonly property int probing: root.ssh.probing ?? 0
    readonly property bool included: root.ssh.included === true

    property bool trashExpanded: false
    property bool keysExpanded: false

    function dotStatus(host: var): string {
        if (host.error !== "") {
            return "failed";
        }
        return host.probe ? host.probe.status : "unknown";
    }

    function probeText(probe: var): string {
        if (!probe) {
            return "";
        }
        if (probe.status === "probing") {
            return "probing";
        }
        return probe.status + (probe.ms > 0 ? " · " + probe.ms + " ms" : "");
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: Kirigami.Units.smallSpacing

        RowLayout {
            Layout.fillWidth: true
            spacing: Kirigami.Units.smallSpacing

            SectionHeading {
                Layout.fillWidth: true
                text: "SSH hosts · " + root.hosts.length
            }

            PlasmaComponents3.Label {
                visible: root.probing > 0
                text: root.probing + " probing"
                color: Kirigami.Theme.neutralTextColor
                font: Kirigami.Theme.smallFont
            }

            PlasmaComponents3.Button {
                text: "Test all"
                icon.name: "network-connect"
                enabled: !root.acting && root.hosts.some(h => h.error === "")
                onClicked: root.sshRequested(["probe"])
            }

            PlasmaComponents3.Button {
                text: "New"
                icon.name: "list-add"
                enabled: !root.acting && root.included && root.usableKeys.length > 0
                onClicked: {
                    newName.clear();
                    newHostname.clear();
                    newPort.value = 22;
                    newUser.text = "root";
                    newKey.currentIndex = 0;
                    newDialog.open();
                    newName.forceActiveFocus();
                }

                PlasmaComponents3.ToolTip {
                    text: !root.included ? "~/.ssh/config must Include ~/.ssh/hosts/*"
                        : root.usableKeys.length === 0 ? "Add a key pair to ~/.ssh/keys first"
                        : "Add a host fragment to ~/.ssh/hosts"
                }
            }
        }

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: (root.ssh.error ?? "").length > 0
            type: Kirigami.MessageType.Error
            text: root.ssh.error ?? ""
        }

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: (root.ssh.warning ?? "").length > 0
            type: Kirigami.MessageType.Warning
            text: root.ssh.warning ?? ""
        }

        PlasmaComponents3.ScrollView {
            id: scrollView

            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth

            ColumnLayout {
                width: scrollView.availableWidth
                spacing: Kirigami.Units.smallSpacing

                PlasmaComponents3.Label {
                    Layout.fillWidth: true
                    visible: root.hosts.length === 0
                    horizontalAlignment: Text.AlignHCenter
                    wrapMode: Text.Wrap
                    text: "No hosts in ~/.ssh/hosts."
                    opacity: 0.7
                }

                Repeater {
                    model: root.hosts

                    delegate: PlasmaComponents3.Frame {
                        id: hostDelegate

                        required property var modelData

                        readonly property bool usable: hostDelegate.modelData.error === ""
                        readonly property var probe: hostDelegate.modelData.probe

                        Layout.fillWidth: true

                        contentItem: ColumnLayout {
                            spacing: Math.round(Kirigami.Units.smallSpacing / 2)

                            RowLayout {
                                Layout.fillWidth: true

                                StatusDot {
                                    Layout.alignment: Qt.AlignVCenter
                                    status: root.dotStatus(hostDelegate.modelData)
                                }

                                ColumnLayout {
                                    Layout.fillWidth: true
                                    spacing: 0

                                    PlasmaComponents3.Label {
                                        Layout.fillWidth: true
                                        text: hostDelegate.modelData.id
                                        font.bold: true
                                        elide: Text.ElideRight
                                    }

                                    PlasmaComponents3.Label {
                                        Layout.fillWidth: true
                                        text: hostDelegate.usable
                                            ? (hostDelegate.modelData.user.length > 0
                                                ? hostDelegate.modelData.user + "@" : "")
                                                + hostDelegate.modelData.hostname + ":"
                                                + hostDelegate.modelData.port
                                            : "Invalid host file"
                                        elide: Text.ElideMiddle
                                        opacity: 0.7
                                        font: Kirigami.Theme.smallFont
                                    }
                                }

                                PlasmaComponents3.ToolButton {
                                    text: "Connect"
                                    icon.name: "utilities-terminal"
                                    display: PlasmaComponents3.AbstractButton.IconOnly
                                    enabled: hostDelegate.usable
                                    onClicked: root.connectRequested(hostDelegate.modelData.id)

                                    PlasmaComponents3.ToolTip {
                                        text: "Connect to " + hostDelegate.modelData.id
                                    }
                                }

                                PlasmaComponents3.ToolButton {
                                    text: "Test"
                                    icon.name: "network-connect"
                                    display: PlasmaComponents3.AbstractButton.IconOnly
                                    enabled: hostDelegate.usable && !root.acting
                                        && !(hostDelegate.probe && hostDelegate.probe.status === "probing")
                                    onClicked: root.sshRequested(["probe", hostDelegate.modelData.id])

                                    PlasmaComponents3.ToolTip {
                                        text: "Test " + hostDelegate.modelData.id
                                    }
                                }

                                PlasmaComponents3.ToolButton {
                                    text: "Edit"
                                    icon.name: "document-edit"
                                    display: PlasmaComponents3.AbstractButton.IconOnly
                                    onClicked: root.editRequested(hostDelegate.modelData.path)

                                    PlasmaComponents3.ToolTip {
                                        text: "Open the host file in the default editor"
                                    }
                                }

                                PlasmaComponents3.ToolButton {
                                    id: hostMenuButton

                                    text: "More"
                                    icon.name: "application-menu"
                                    display: PlasmaComponents3.AbstractButton.IconOnly
                                    checked: hostMenu.status === PlasmaExtras.Menu.Open
                                    onPressed: hostMenu.openRelative()

                                    PlasmaComponents3.ToolTip {
                                        text: "Host actions"
                                    }
                                }
                            }

                            PlasmaComponents3.Label {
                                Layout.fillWidth: true
                                visible: hostDelegate.probe !== undefined
                                text: root.probeText(hostDelegate.probe)
                                color: hostDelegate.probe && hostDelegate.probe.status === "failed"
                                    ? Kirigami.Theme.negativeTextColor
                                    : Kirigami.Theme.textColor
                                font: Kirigami.Theme.smallFont
                            }

                            Kirigami.InlineMessage {
                                Layout.fillWidth: true
                                visible: hostDelegate.modelData.error.length > 0
                                type: Kirigami.MessageType.Warning
                                text: hostDelegate.modelData.error
                            }

                            Kirigami.InlineMessage {
                                Layout.fillWidth: true
                                visible: hostDelegate.modelData.warning.length > 0
                                type: Kirigami.MessageType.Warning
                                text: hostDelegate.modelData.warning
                            }

                            Kirigami.InlineMessage {
                                Layout.fillWidth: true
                                visible: hostDelegate.probe !== undefined
                                    && hostDelegate.probe.error.length > 0
                                type: Kirigami.MessageType.Error
                                text: hostDelegate.probe ? hostDelegate.probe.error : ""
                            }
                        }

                        PlasmaExtras.Menu {
                            id: hostMenu

                            visualParent: hostMenuButton
                            placement: PlasmaExtras.Menu.BottomPosedLeftAlignedPopup

                            PlasmaExtras.MenuItem {
                                text: "Move to Trash"
                                icon: "user-trash"
                                enabled: !root.acting
                                onClicked: root.sshRequested(["trash", hostDelegate.modelData.id])
                            }
                        }
                    }
                }

                PlasmaComponents3.ToolButton {
                    Layout.fillWidth: true
                    text: "Trash · " + root.trash.length
                    icon.name: root.trashExpanded ? "arrow-down" : "arrow-right"
                    display: PlasmaComponents3.AbstractButton.TextBesideIcon
                    onClicked: root.trashExpanded = !root.trashExpanded
                }

                PlasmaComponents3.Label {
                    Layout.fillWidth: true
                    visible: root.trashExpanded && root.trash.length === 0
                    horizontalAlignment: Text.AlignHCenter
                    text: "Trash is empty."
                    opacity: 0.7
                }

                Repeater {
                    model: root.trashExpanded ? root.trash : []

                    delegate: PlasmaComponents3.ItemDelegate {
                        id: trashDelegate

                        required property var modelData

                        Layout.fillWidth: true

                        contentItem: ColumnLayout {
                            spacing: 0

                            RowLayout {
                                Layout.fillWidth: true

                                PlasmaComponents3.Label {
                                    Layout.fillWidth: true
                                    text: trashDelegate.modelData.id
                                        + (trashDelegate.modelData.error === ""
                                            ? " · " + trashDelegate.modelData.hostname
                                                + ":" + trashDelegate.modelData.port
                                            : "")
                                    elide: Text.ElideMiddle
                                }

                                PlasmaComponents3.ToolButton {
                                    text: "Restore"
                                    icon.name: "edit-undo"
                                    display: PlasmaComponents3.AbstractButton.IconOnly
                                    enabled: !root.acting
                                    onClicked: root.sshRequested(["restore", trashDelegate.modelData.id])

                                    PlasmaComponents3.ToolTip {
                                        text: "Restore " + trashDelegate.modelData.id
                                    }
                                }

                                PlasmaComponents3.ToolButton {
                                    text: "Delete permanently"
                                    icon.name: "edit-delete"
                                    display: PlasmaComponents3.AbstractButton.IconOnly
                                    enabled: !root.acting
                                    onClicked: {
                                        purgeDialog.hostId = trashDelegate.modelData.id;
                                        purgeDialog.open();
                                    }

                                    PlasmaComponents3.ToolTip {
                                        text: "Delete permanently"
                                    }
                                }
                            }

                            PlasmaComponents3.Label {
                                Layout.fillWidth: true
                                visible: trashDelegate.modelData.error.length > 0
                                text: trashDelegate.modelData.error
                                color: Kirigami.Theme.negativeTextColor
                                wrapMode: Text.Wrap
                                font: Kirigami.Theme.smallFont
                            }
                        }
                    }
                }

                PlasmaComponents3.ToolButton {
                    Layout.fillWidth: true
                    text: "Keys · " + root.keys.length
                    icon.name: root.keysExpanded ? "arrow-down" : "arrow-right"
                    display: PlasmaComponents3.AbstractButton.TextBesideIcon
                    onClicked: root.keysExpanded = !root.keysExpanded
                }

                PlasmaComponents3.Label {
                    Layout.fillWidth: true
                    visible: root.keysExpanded && root.keys.length === 0
                    horizontalAlignment: Text.AlignHCenter
                    text: "No keys in ~/.ssh/keys."
                    opacity: 0.7
                }

                Repeater {
                    model: root.keysExpanded ? root.keys : []

                    delegate: PlasmaComponents3.ItemDelegate {
                        id: keyDelegate

                        required property var modelData

                        Layout.fillWidth: true

                        contentItem: ColumnLayout {
                            spacing: 0

                            PlasmaComponents3.Label {
                                Layout.fillWidth: true
                                text: keyDelegate.modelData.id
                                    + (keyDelegate.modelData.fingerprint.length > 0
                                        ? " · " + keyDelegate.modelData.fingerprint : "")
                                elide: Text.ElideMiddle
                            }

                            PlasmaComponents3.Label {
                                Layout.fillWidth: true
                                visible: keyDelegate.modelData.error.length > 0
                                text: keyDelegate.modelData.error
                                color: Kirigami.Theme.negativeTextColor
                                wrapMode: Text.Wrap
                                font: Kirigami.Theme.smallFont
                            }
                        }

                        PlasmaComponents3.ToolTip {
                            text: keyDelegate.modelData.error.length > 0
                                ? keyDelegate.modelData.error : keyDelegate.modelData.fingerprint
                        }
                    }
                }

                Item {
                    Layout.fillWidth: true
                    Layout.preferredHeight: Kirigami.Units.smallSpacing
                }
            }
        }
    }

    Kirigami.Dialog {
        id: newDialog

        title: "New SSH host"
        preferredWidth: Kirigami.Units.gridUnit * 20
        standardButtons: Kirigami.Dialog.Cancel

        property Kirigami.Action createAction: Kirigami.Action {
            text: "Create"
            icon.name: "list-add"
            enabled: newName.text.trim().length > 0
                && newHostname.text.trim().length > 0
                && newKey.currentIndex >= 0
            onTriggered: {
                root.sshRequested(["create", newName.text.trim(), newHostname.text.trim(),
                                   String(newPort.value), newUser.text.trim(), newKey.currentValue]);
                newDialog.close();
            }
        }

        customFooterActions: [createAction]

        Kirigami.FormLayout {
            PlasmaComponents3.TextField {
                id: newName

                Kirigami.FormData.label: "Name:"
                placeholderText: "server-name"
                selectByMouse: true
            }

            PlasmaComponents3.TextField {
                id: newHostname

                Kirigami.FormData.label: "Hostname:"
                placeholderText: "server.example.com"
                selectByMouse: true
            }

            PlasmaComponents3.SpinBox {
                id: newPort

                Kirigami.FormData.label: "Port:"
                from: 1
                to: 65535
                value: 22
                editable: true
            }

            PlasmaComponents3.TextField {
                id: newUser

                Kirigami.FormData.label: "User:"
                text: "root"
                selectByMouse: true
            }

            PlasmaComponents3.ComboBox {
                id: newKey

                Kirigami.FormData.label: "Key:"
                Layout.fillWidth: true
                model: root.usableKeys
                textRole: "id"
                valueRole: "id"
            }
        }
    }

    Kirigami.PromptDialog {
        id: purgeDialog

        property string hostId

        title: "Move beyond recovery?"
        subtitle: "“" + hostId
            + "” is already in Trash. Continue to the final permanent-delete check?"
        standardButtons: Kirigami.Dialog.Ok | Kirigami.Dialog.Cancel
        onAccepted: {
            close();
            purgeFinalDialog.hostId = hostId;
            purgeFinalDialog.open();
        }
    }

    Kirigami.PromptDialog {
        id: purgeFinalDialog

        property string hostId

        title: "Permanently delete now?"
        subtitle: "Final confirmation: “" + hostId
            + "” will be removed. This cannot be undone."
        standardButtons: Kirigami.Dialog.Ok | Kirigami.Dialog.Cancel
        onAccepted: {
            root.sshRequested(["purge", hostId]);
            close();
        }
    }
}
