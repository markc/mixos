pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents3

import "components"

// BUS: the services noded has registered, and the mesh peers it knows about.
Item {
    id: root

    required property var snap

    readonly property var bus: root.snap.bus ?? ({})
    readonly property var services: Array.isArray(root.bus.services) ? root.bus.services : []
    readonly property var peers: Array.isArray(root.bus.peers) ? root.bus.peers : []

    // A noded entry is an object with a name, or sometimes a bare string.
    function label(entry: var): string {
        if (typeof entry === "string") {
            return entry;
        }
        if (entry.name !== undefined) {
            return entry.name;
        }
        if (entry.id !== undefined) {
            return entry.id;
        }
        return JSON.stringify(entry);
    }

    PlasmaComponents3.ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth

        ColumnLayout {
            width: parent.width
            spacing: Kirigami.Units.smallSpacing

            PlasmaComponents3.Label {
                Layout.fillWidth: true
                visible: root.bus.error !== undefined
                text: "noded did not list services: " + root.bus.error
                color: Kirigami.Theme.negativeTextColor
                wrapMode: Text.Wrap
            }

            SectionHeading {
                Layout.fillWidth: true
                text: "Services on the Bus (" + root.services.length + ")"
            }

            PlasmaComponents3.Label {
                Layout.fillWidth: true
                visible: root.services.length === 0 && root.bus.error === undefined
                text: "No services registered."
                opacity: 0.7
            }

            Repeater {
                model: root.services

                delegate: PlasmaComponents3.Label {
                    id: serviceLabel

                    required property var modelData

                    Layout.fillWidth: true
                    text: root.label(serviceLabel.modelData)
                    elide: Text.ElideMiddle
                }
            }

            SectionHeading {
                Layout.fillWidth: true
                visible: root.bus.peers_error === undefined
                text: "Mesh peers (" + root.peers.length + ")"
            }

            PlasmaComponents3.Label {
                Layout.fillWidth: true
                visible: root.bus.peers_error !== undefined
                text: "Mesh peers are not reported by noded."
                opacity: 0.7
                wrapMode: Text.Wrap
            }

            Repeater {
                model: root.peers

                delegate: PlasmaComponents3.Label {
                    id: peerLabel

                    required property var modelData

                    Layout.fillWidth: true
                    text: root.label(peerLabel.modelData)
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
