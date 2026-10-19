pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents3

import "components"

// SSH: the concrete host aliases in ~/.ssh/config. Keys and trash need a
// MixOS source, so they are not listed.
Item {
    id: root

    required property var snap

    readonly property var ssh: root.snap.ssh ?? ({})
    readonly property var hosts: Array.isArray(root.ssh.hosts) ? root.ssh.hosts : []

    PlasmaComponents3.ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth

        ColumnLayout {
            width: parent.width
            spacing: Kirigami.Units.smallSpacing

            PlasmaComponents3.Label {
                Layout.fillWidth: true
                visible: root.ssh.error !== undefined
                text: root.ssh.error ?? ""
                color: Kirigami.Theme.negativeTextColor
                wrapMode: Text.Wrap
            }

            SectionHeading {
                Layout.fillWidth: true
                text: "Hosts (" + root.hosts.length + ")"
            }

            PlasmaComponents3.Label {
                Layout.fillWidth: true
                visible: root.hosts.length === 0 && root.ssh.error === undefined
                text: "No hosts in ~/.ssh/config."
                opacity: 0.7
            }

            Repeater {
                model: root.hosts

                delegate: PlasmaComponents3.Label {
                    id: hostLabel

                    required property var modelData

                    Layout.fillWidth: true
                    text: hostLabel.modelData
                    elide: Text.ElideMiddle
                }
            }

            Item {
                Layout.fillWidth: true
                Layout.preferredHeight: Kirigami.Units.smallSpacing
            }
        }
    }
}
