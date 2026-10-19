import QtQuick
import QtQuick.Layouts

import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents3

// Four tabs, in the order COS, BUS, MIX, SSH. Each tab reads the one snapshot
// the popup holds, so switching tabs never asks mixtrayd again.
Item {
    id: root

    required property var snap
    required property string snapError
    required property bool loading
    required property bool acting
    required property string actionMessage
    required property string actionError

    signal refreshRequested()
    signal controlRequested(string unit, string action)
    signal logsRequested(string unit)

    Layout.minimumWidth: Kirigami.Units.gridUnit * 20
    Layout.minimumHeight: Kirigami.Units.gridUnit * 20
    Layout.preferredWidth: Kirigami.Units.gridUnit * 23
    Layout.preferredHeight: Kirigami.Units.gridUnit * 30
    implicitWidth: Layout.preferredWidth
    implicitHeight: Layout.preferredHeight

    ColumnLayout {
        anchors.fill: parent
        spacing: Kirigami.Units.smallSpacing

        RowLayout {
            Layout.fillWidth: true

            PlasmaComponents3.TabBar {
                id: tabs

                Layout.fillWidth: true
                currentIndex: 0

                PlasmaComponents3.TabButton {
                    text: "COS"
                }
                PlasmaComponents3.TabButton {
                    text: "BUS"
                }
                PlasmaComponents3.TabButton {
                    text: "MIX"
                }
                PlasmaComponents3.TabButton {
                    text: "SSH"
                }
            }

            PlasmaComponents3.BusyIndicator {
                running: root.loading || root.acting
                visible: running
                Layout.preferredWidth: Kirigami.Units.iconSizes.smallMedium
                Layout.preferredHeight: Kirigami.Units.iconSizes.smallMedium
            }

            PlasmaComponents3.ToolButton {
                icon.name: "view-refresh"
                text: "Refresh"
                display: PlasmaComponents3.AbstractButton.IconOnly
                onClicked: root.refreshRequested()

                PlasmaComponents3.ToolTip {
                    text: "Ask mixtrayd again"
                }
            }
        }

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: root.snapError.length > 0
            type: Kirigami.MessageType.Error
            text: root.snapError
        }

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: root.actionError.length > 0
            type: Kirigami.MessageType.Error
            text: root.actionError
        }

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            visible: root.actionMessage.length > 0
            type: Kirigami.MessageType.Positive
            text: root.actionMessage
        }

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: tabs.currentIndex

            CosTab {
                snap: root.snap
                acting: root.acting
                onControlRequested: (unit, action) => root.controlRequested(unit, action)
                onLogsRequested: unit => root.logsRequested(unit)
            }

            BusTab {
                snap: root.snap
            }

            MixTab {
                snap: root.snap
            }

            SshTab {
                snap: root.snap
            }
        }
    }
}
