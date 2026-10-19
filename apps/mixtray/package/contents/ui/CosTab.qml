pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents3

import "components"

// COS: the MixOS units and their systemd state. Start, stop and restart are
// not offered yet; mixtrayd is read-only for now.
Item {
    id: root

    required property var snap

    readonly property var units: root.snap.cos && root.snap.cos.units ? root.snap.cos.units : []

    PlasmaComponents3.ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth

        ColumnLayout {
            width: parent.width
            spacing: Kirigami.Units.smallSpacing

            SectionHeading {
                Layout.fillWidth: true
                text: "MixOS units"
            }

            PlasmaComponents3.Label {
                Layout.fillWidth: true
                visible: root.snap.cos !== undefined && root.units.length === 0
                text: "No MixOS units listed."
                opacity: 0.7
            }

            Repeater {
                model: root.units

                delegate: PlasmaComponents3.Frame {
                    id: row

                    required property var modelData

                    Layout.fillWidth: true

                    contentItem: RowLayout {
                        spacing: Kirigami.Units.smallSpacing

                        StatusDot {
                            Layout.alignment: Qt.AlignVCenter
                            status: row.modelData.state
                        }

                        PlasmaComponents3.Label {
                            Layout.fillWidth: true
                            Layout.alignment: Qt.AlignVCenter
                            text: row.modelData.name
                            elide: Text.ElideMiddle
                        }

                        PlasmaComponents3.Label {
                            Layout.alignment: Qt.AlignVCenter
                            text: row.modelData.manager
                            opacity: 0.6
                        }

                        PlasmaComponents3.Label {
                            Layout.alignment: Qt.AlignVCenter
                            text: row.modelData.state
                            opacity: 0.8
                        }
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
