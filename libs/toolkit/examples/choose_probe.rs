// SPDX-License-Identifier: MIT OR Apache-2.0
//! CBC-only real DOpus chooser probe, run by tests/choose_bus_test.mix.
use anyhow::{Context as _, Result, ensure};
use bus::native_client::SupervisedClient;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use toolkit::choose::{Mode, Outcome, Tier};

const WAIT: Duration = Duration::from_secs(30);

async fn drive(
    bus: Arc<SupervisedClient>,
    correlation: String,
    mode: Mode,
    cancel: bool,
) -> Result<()> {
    let id = tokio::time::timeout(WAIT, async {
        loop {
            let value = bus.call("files", "files.choose.list", json!({})).await?;
            if let Some(id) = value["requests"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| r["correlation"].as_str() == Some(&correlation))
                .and_then(|r| r["request"].as_str())
            {
                break Ok::<_, anyhow::Error>(id.to_owned());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(tree) = bus
                .call("files", "files.ui.tree", json!({"viewport":id}))
                .await
                && tree.to_string().contains("Places")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    if !cancel && mode == Mode::Open {
        bus.call(
            "files",
            "files.ui.click",
            json!({"viewport":id,"label":"notes.md"}),
        )
        .await?;
    }
    let label = if cancel {
        "Cancel"
    } else if mode == Mode::Open {
        "Open"
    } else {
        "Save"
    };
    // The closing viewport may settle the drive input with CANCELLED. The
    // toolkit handle's outcome is the authoritative acceptance below.
    let _ = bus
        .call(
            "files",
            "files.ui.click",
            json!({"viewport":id,"label":label}),
        )
        .await;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let folder = PathBuf::from(std::env::args().nth(1).context("folder argument")?);
    let bus = Arc::new(
        SupervisedClient::connect_options(
            "toolkit-chooser-probe",
            &bus::client_helpers::resolve_noded_url(),
        )
        .connect()
        .await?,
    );
    for mode in [Mode::Open, Mode::Save, Mode::SaveFiles] {
        for cancel in [false, true] {
            let correlation = format!("compose-{mode:?}-{cancel}");
            let mut body = json!({"mode":mode,"current_folder":folder,"correlation":correlation,
                "choices":[{"id":"check","label":"Check","options":[]}]});
            if mode != Mode::SaveFiles {
                body["filters"] = json!([{"name":"Markdown","patterns":["*.md"],"mime_types":[]}]);
            }
            if mode == Mode::Save {
                body["current_name"] = json!("draft");
            }
            if mode == Mode::SaveFiles {
                body["files"] = json!(["one.md", "two.md"]);
            }
            let mut pending = toolkit::choose(
                toolkit::choose::request(&body.to_string())
                    .map_err(|e| anyhow::anyhow!(e.message))?,
            );
            let ctx = egui::Context::default();
            let driver = tokio::spawn(drive(bus.clone(), correlation, mode, cancel));
            let outcome = tokio::time::timeout(WAIT, async {
                loop {
                    let mut outcome = None;
                    ctx.run_ui(Default::default(), |ui| outcome = pending.poll(ui.ctx()))
                        .drop_without_applying_deltas();
                    if let Some(outcome) = outcome {
                        break outcome;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await?;
            driver.await??;
            ensure!(pending.tier() == Tier::Bus, "real provider was bypassed");
            if cancel {
                ensure!(outcome == Outcome::Cancelled, "Cancel returned {outcome:?}");
            } else {
                let Outcome::Chosen {
                    paths,
                    choices,
                    current_filter,
                } = outcome
                else {
                    anyhow::bail!("not chosen")
                };
                let expected = match mode {
                    Mode::Open => vec![folder.join("notes.md")],
                    Mode::Save => vec![folder.join("draft.md")],
                    Mode::SaveFiles => vec![folder.join("one.md"), folder.join("two.md")],
                };
                ensure!(
                    paths == expected,
                    "paths {paths:?} differ from {expected:?}"
                );
                ensure!(choices["check"] == "false", "choice default lost");
                ensure!(
                    current_filter.is_some() == (mode != Mode::SaveFiles),
                    "chosen filter lost"
                );
            }
            println!("PASS toolkit::choose {mode:?} cancel={cancel} through real DOpus");
        }
    }
    ensure!(
        std::fs::read_to_string(folder.join("notes.md"))? == "original",
        "chooser changed source"
    );
    for name in ["draft.md", "one.md", "two.md"] {
        ensure!(!folder.join(name).exists(), "chooser wrote destination");
    }
    bus.close().await;
    Ok(())
}
