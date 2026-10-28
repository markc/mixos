// SPDX-License-Identifier: MIT OR Apache-2.0
//! rfd only on Windows/macOS. Its Option result conflates cancel and failure.
use super::*;

pub(super) async fn attempt(request: &Request) -> Option<Outcome> {
    if request.mode == Mode::SaveFiles {
        return None;
    }
    // The synchronous macOS backend dispatches runModal to the UI thread.
    // Use the asynchronous sheet API from this chooser's worker instead.
    let mut dialog = rfd::AsyncFileDialog::new().set_title(&request.title);
    if let Some(folder) = &request.current_folder {
        dialog = dialog.set_directory(path_from_bytes(folder));
    }
    if let Some(name) = &request.current_name {
        dialog = dialog.set_file_name(String::from_utf8_lossy(name));
    }
    for filter in &request.filters {
        let extensions: Vec<_> = filter
            .patterns()
            .iter()
            .filter_map(|p| p.strip_prefix("*."))
            .filter(|ext| !ext.contains(['*', '?', '[', ']', '/']))
            .collect();
        if !extensions.is_empty() {
            dialog = dialog.add_filter(&filter.name, &extensions);
        }
    }
    let paths = match (request.mode, request.directory, request.multiple) {
        (Mode::Save, _, _) => dialog.save_file().await.map(|p| vec![p]),
        (_, true, true) => dialog.pick_folders().await,
        (_, true, false) => dialog.pick_folder().await.map(|p| vec![p]),
        (_, false, true) => dialog.pick_files().await,
        _ => dialog.pick_file().await.map(|p| vec![p]),
    };
    Some(
        paths
            .map(|paths| Outcome::Chosen {
                paths: paths.into_iter().map(|p| p.path().to_owned()).collect(),
                choices: request.choice_defaults(),
                current_filter: None,
            })
            .unwrap_or(Outcome::Cancelled),
    )
}