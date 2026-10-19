pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents3

import "components"

// COS: the MixOS units and their systemd state. Start, stop and restart go
// through hostd's grant, so only the units mixtrayd reports as controllable
// get them; the others show their state and the log button only.
Item {
    id: root

    required property var snap
    required property bool acting

    signal controlRequested(string unit, string action)
    signal logsRequested(string unit)

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

                    contentItem: ColumnLayout {
                        spacing: 0

                        RowLayout {
                            Layout.fillWidth: true
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

                        RowLayout {
                            Layout.fillWidth: true
                            spacing: Kirigami.Units.smallSpacing

                            Item {
                                Layout.fillWidth: true
                            }

                            PlasmaComponents3.ToolButton {
                                text: "Start"
                                icon.name: "media-playback-start"
                                display: PlasmaComponents3.AbstractButton.IconOnly
                                enabled: row.modelData.controllable && !root.acting
                                onClicked: root.controlRequested(row.modelData.name, "start")

                                PlasmaComponents3.ToolTip {
                                    text: row.modelData.controllable ? "Start" : "Start is not offered for this unit"
                                }
                            }

                            PlasmaComponents3.ToolButton {
                                text: "Stop"
                                icon.name: "media-playback-stop"
                                display: PlasmaComponents3.AbstractButton.IconOnly
                                enabled: row.modelData.controllable && !root.acting
                                onClicked: root.controlRequested(row.modelData.name, "stop")

                                PlasmaComponents3.ToolTip {
                                    text: row.modelData.controllable ? "Stop" : "Stop is not offered for this unit"
                                }
                            }

                            PlasmaComponents3.ToolButton {
                                text: "Restart"
                                icon.name: "view-refresh"
                                display: PlasmaComponents3.AbstractButton.IconOnly
                                enabled: row.modelData.controllable && !root.acting
                                onClicked: root.controlRequested(row.modelData.name, "restart")

                                PlasmaComponents3.ToolTip {
                                    text: row.modelData.controllable ? "Restart" : "Restart is not offered for this unit"
                                }
                            }

                            PlasmaComponents3.ToolButton {
                                text: "Logs"
                                icon.name: "utilities-log-viewer"
                                display: PlasmaComponents3.AbstractButton.IconOnly
                                onClicked: root.logsRequested(row.modelData.name)

                                PlasmaComponents3.ToolTip {
                                    text: "Open the unit's journal"
                                }
                            }
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
