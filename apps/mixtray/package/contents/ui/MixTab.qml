pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PlasmaComponents3

import "components"

// MIX: the installed mix binary. Scripts, runs and trash are cosmix hub
// features with no MixOS source yet, so they are not shown here.
Item {
    id: root

    required property var snap

    readonly property var mix: root.snap.mix ?? ({})

    PlasmaComponents3.ScrollView {
        anchors.fill: parent
        contentWidth: availableWidth

        ColumnLayout {
            width: parent.width
            spacing: Kirigami.Units.smallSpacing

            SectionHeading {
                Layout.fillWidth: true
                text: "mix"
            }

            PlasmaComponents3.Label {
                Layout.fillWidth: true
                visible: root.mix.error !== undefined
                text: root.mix.error ?? ""
                color: Kirigami.Theme.negativeTextColor
                wrapMode: Text.Wrap
            }

            PlasmaComponents3.Label {
                Layout.fillWidth: true
                visible: root.mix.version !== undefined
                text: root.mix.version ?? ""
                wrapMode: Text.Wrap
            }

            PlasmaComponents3.Label {
                Layout.fillWidth: true
                visible: root.mix.path !== undefined
                text: root.mix.path ?? ""
                elide: Text.ElideMiddle
                opacity: 0.6
            }

            PlasmaComponents3.Label {
                Layout.fillWidth: true
                text: "Scripts, runs and trash are not available on MixOS yet."
                opacity: 0.7
                wrapMode: Text.Wrap
            }

            Item {
                Layout.fillWidth: true
                Layout.preferredHeight: Kirigami.Units.smallSpacing
            }
        }
    }
}
