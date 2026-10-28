<!-- SPDX-License-Identifier: MIT OR Apache-2.0 -->
# Changelog

## Unreleased — 2026-10-27

- Offer Bold (700) alongside the bundled Light, Regular, Medium and SemiBold
  interface faces. Test every settings path produced by the controls against
  settingsd's setter and keep interface-font refusals visible after rereading.
- Follow accepted interface font settings in Preferences itself, including
  font-only updates that leave the rest of the appearance unchanged.

- Desktop > Windows offers registry-driven menu, title, Action Bar and window
  button alignment, plus Action Bar hosting in the title bar. Window buttons
  stay editable in CSD and SSD. All axes apply immediately and are described
  by `prefs.appearance.set`.

- Delegate Email launching and registration waits to citizen's shared async
  helper. Keep passwords outside effect bodies until sending, preserve
  refusals, and return a failed password to its field. Account status reads
  and best-effort password removal do not launch Email.
