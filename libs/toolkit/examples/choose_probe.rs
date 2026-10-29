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
    tier: Tier,
) -> Result<()> {
    let id = tokio::time::timeout(WAIT, async {
        loop {
            let value = bus.call("files", "files.choose.list", json!({})).await?;
            if let Some(id) = value["requests"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|r| {
                    if tier == Tier::System {
                        r["title"].as_str() == Some(&correlation)
                    } else {
                        r["correlation"].as_str() == Some(&correlation)
                    }
                })
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
            json!({"viewport":id,"label":"File type"}),
        )
        .await?;
        bus.call(
            "files",
            "files.ui.click",
            json!({"viewport":id,"label":"All files"}),
        )
        .await?;
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
    let mode = std::env::args().nth(2).unwrap_or_else(|| "bus".into());
    if matches!(mode.as_str(), "system" | "embedded") {
        let resolved = bus::client_helpers::resolve_noded_url();
        let empty = SupervisedClient::connect_options("chooser-empty-audit", &resolved)
            .connect()
            .await?;
        ensure!(
            !empty
                .list_services()
                .await?
                .iter()
                .any(|name| name == "files"),
            "fallback caller's broker unexpectedly has files"
        );
        let info = empty.call("noded", "noded.info", json!({})).await?;
        ensure!(
            info["node"] == "chooser-empty",
            "wrong caller broker: {info}"
        );
        println!("PASS caller resolver reaches provider-free broker {resolved}: {info}");
        empty.close().await;
    }
    if mode == "embedded" {
        return embedded(&folder);
    }
    let expected_tier = match mode.as_str() {
        "bus" => Tier::Bus,
        "system" => Tier::System,
        _ => anyhow::bail!("expected bus, system or embedded mode"),
    };
    // Only the test driver's URL is overridden. The production Handle still
    // resolves MIXOS_NODED_URL normally, independently of this connection.
    let driver_url = std::env::var("CHOOSER_DRIVER_URL")
        .unwrap_or_else(|_| bus::client_helpers::resolve_noded_url());
    let bus = Arc::new(
        SupervisedClient::connect_options("toolkit-chooser-probe", &driver_url)
            .connect()
            .await?,
    );
    #[cfg(all(target_os = "linux", feature = "portal"))]
    if expected_tier == Tier::System {
        let connection = zbus::Connection::session().await?;
        tokio::time::timeout(WAIT, async {
            loop {
                if connection
                    .call_method(
                        Some("org.freedesktop.portal.Desktop"),
                        "/org/freedesktop/portal/desktop",
                        Some("org.freedesktop.DBus.Properties"),
                        "Get",
                        &("org.freedesktop.portal.FileChooser", "version"),
                    )
                    .await
                    .is_ok()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await?;
    }
    for mode in [Mode::Open, Mode::Save, Mode::SaveFiles] {
        for cancel in [false, true] {
            let correlation = format!("compose-{mode:?}-{cancel}");
            let mut body = json!({"mode":mode,"current_folder":folder,"correlation":correlation,"title":correlation,
                "choices":[{"id":"check","label":"Check","options":[]}]});
            if mode != Mode::SaveFiles {
                body["filters"] = json!([{"name":"Markdown","patterns":["*.md"],"mime_types":[]},
                    {"name":"All files","patterns":["*"],"mime_types":[]}]);
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
            let driver = tokio::spawn(drive(bus.clone(), correlation, mode, cancel, expected_tier));
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
            .await;
            // Surface a failed drive command before a chooser deadline masks
            // it. A drive error can otherwise leave the live window waiting.
            tokio::time::timeout(WAIT, driver).await???;
            let outcome = outcome?;
            ensure!(
                pending.tier() == expected_tier,
                "wrong tier: {:?}",
                pending.tier()
            );
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
                if mode == Mode::Open {
                    ensure!(
                        current_filter.unwrap().name == "All files",
                        "selected filter did not round-trip"
                    );
                }
            }
            // Keep polling after either terminal result: cancellation and
            // acceptance must remain final and cannot spawn an embedded UI.
            for _ in 0..10 {
                ctx.run_ui(Default::default(), |ui| {
                    assert!(pending.poll(ui.ctx()).is_none());
                    assert_eq!(pending.tier(), expected_tier);
                })
                .drop_without_applying_deltas();
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            ensure!(
                bus.call("files", "files.choose.list", json!({})).await?["requests"]
                    .as_array()
                    .context("requests")?
                    .is_empty(),
                "chooser left open after outcome"
            );
            println!(
                "PASS toolkit::choose {mode:?} cancel={cancel} tier={expected_tier:?} through real DOpus"
            );
        }
    }
    if expected_tier == Tier::Bus {
        simultaneous_and_busy(bus.clone()).await?;
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

/// Real production cascade and real FileChooser, driven through egui's public
/// accessibility/input harness. This is frame/UI proof, not a mapped window.
fn embedded(folder: &std::path::Path) -> Result<()> {
    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};
    use std::time::Instant;
    for mode in [Mode::Open, Mode::Save, Mode::SaveFiles] {
        for cancel in [false, true] {
            let mut body = json!({"mode":mode,"current_folder":folder,
                "title":"Embedded acceptance", "choices":[{"id":"check","label":"Check","options":[]}]});
            if mode == Mode::Save {
                body["current_name"] = json!("embedded.md");
            }
            if mode == Mode::SaveFiles {
                body["files"] = json!(["one.md", "two.md"]);
            }
            let handle = toolkit::choose(
                toolkit::choose::request(&body.to_string())
                    .map_err(|e| anyhow::anyhow!(e.message))?,
            );
            let mut h = Harness::builder()
                .with_size(egui::vec2(900.0, 700.0))
                .build_ui_state(
                    |ui, state: &mut (toolkit::choose::Handle, Vec<Outcome>)| {
                        if let Some(answer) = state.0.poll(ui.ctx()) {
                            state.1.push(answer);
                        }
                    },
                    (handle, Vec::new()),
                );
            toolkit::install(&h.ctx, &toolkit::Theme::embedded());
            let deadline = Instant::now() + WAIT;
            while h.state().0.tier() != Tier::InProcess
                || if mode == Mode::SaveFiles {
                    h.query_by_label("Save")
                        .is_none_or(|node| node.accesskit_node().is_disabled())
                } else {
                    h.query_by_label("notes.md").is_none()
                }
            {
                ensure!(
                    Instant::now() < deadline,
                    "embedded UI did not become ready"
                );
                ensure!(
                    h.state().1.is_empty(),
                    "unexpected early outcome: {:?}",
                    h.state().1
                );
                h.run_steps(1);
                std::thread::sleep(Duration::from_millis(10));
            }
            if cancel {
                h.get_by_label("Cancel").click();
            } else {
                if mode == Mode::Open {
                    h.get_by_label("notes.md").click();
                    h.run_steps(2);
                }
                h.get_by_label(if mode == Mode::Open { "Open" } else { "Save" })
                    .click();
            }
            while h.state().1.is_empty() {
                ensure!(Instant::now() < deadline, "embedded input did not settle");
                h.run_steps(1);
                std::thread::sleep(Duration::from_millis(10));
            }
            for _ in 0..10 {
                h.run_steps(1);
            }
            ensure!(
                h.state().0.tier() == Tier::InProcess && h.state().1.len() == 1,
                "embedded outcome/tier not final"
            );
            if cancel {
                ensure!(
                    h.state().1[0] == Outcome::Cancelled,
                    "embedded cancellation failed"
                );
            } else {
                let Outcome::Chosen { paths, choices, .. } = &h.state().1[0] else {
                    anyhow::bail!("embedded did not choose: {:?}", h.state().1);
                };
                let expected = match mode {
                    Mode::Open => vec![folder.join("notes.md")],
                    Mode::Save => vec![folder.join("embedded.md")],
                    Mode::SaveFiles => vec![folder.join("one.md"), folder.join("two.md")],
                };
                ensure!(
                    *paths == expected && choices["check"] == "false",
                    "embedded result differs"
                );
            }
            println!(
                "PASS real cascade InProcess {mode:?} cancel={cancel}; deterministic egui frame/input, no native window"
            );
        }
    }
    ensure!(
        std::fs::read_to_string(folder.join("notes.md"))? == "original",
        "source changed"
    );
    for name in ["embedded.md", "one.md", "two.md"] {
        ensure!(!folder.join(name).exists(), "chooser wrote destination");
    }
    Ok(())
}

async fn simultaneous_and_busy(bus: Arc<SupervisedClient>) -> Result<()> {
    // Each Handle owns its own client. The real provider must open precisely
    // one window per request, including two from the same toolkit app.
    let mut handles = ["parallel-one", "parallel-two"].map(|correlation| {
        toolkit::choose(
            toolkit::choose::request(
                &json!({"correlation":correlation,"on_behalf_of":"dev.mixos.example"}).to_string(),
            )
            .unwrap(),
        )
    });
    let ctx = egui::Context::default();
    let requests = tokio::time::timeout(WAIT, async {
        loop {
            ctx.run_ui(Default::default(), |ui| {
                for handle in &mut handles {
                    assert!(handle.poll(ui.ctx()).is_none());
                }
            })
            .drop_without_applying_deltas();
            let listed = bus.call("files", "files.choose.list", json!({})).await?;
            let requests: Vec<_> = listed["requests"]
                .as_array()
                .context("list")?
                .iter()
                .filter(|r| {
                    matches!(
                        r["correlation"].as_str(),
                        Some("parallel-one" | "parallel-two")
                    )
                })
                .cloned()
                .collect();
            if requests.len() == 2 {
                break Ok::<_, anyhow::Error>(requests);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await??;
    for correlation in ["parallel-one", "parallel-two"] {
        ensure!(
            requests
                .iter()
                .filter(|r| r["correlation"] == correlation)
                .count()
                == 1,
            "duplicate chooser"
        );
    }
    for row in &requests {
        bus.call(
            "files",
            "files.choose.cancel",
            json!({"request":row["request"]}),
        )
        .await?;
    }
    let mut answers = [0usize; 2];
    tokio::time::timeout(WAIT, async {
        loop {
            ctx.run_ui(Default::default(), |ui| {
                for (i, handle) in handles.iter_mut().enumerate() {
                    if let Some(answer) = handle.poll(ui.ctx()) {
                        assert_eq!(answer, Outcome::Cancelled);
                        answers[i] += 1;
                    }
                    assert_eq!(
                        handle.tier(),
                        Tier::Bus,
                        "external cancellation opened another chooser"
                    );
                }
            })
            .drop_without_applying_deltas();
            if answers == [1, 1] {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    for _ in 0..3 {
        ctx.run_ui(Default::default(), |ui| {
            for handle in &mut handles {
                assert!(handle.poll(ui.ctx()).is_none());
            }
        })
        .drop_without_applying_deltas();
    }
    // Exercise BUSY on one actual caller connection: its first two requests
    // are accepted and its third is refused without opening a window.
    let mut ids = Vec::new();
    for _ in 0..2 {
        let reply = bus.call("files", "files.choose", json!({})).await?;
        ids.push(reply["request"].as_str().context("request id")?.to_owned());
    }
    let (rc, busy, _) = bus
        .call_with_headers_raw("files", "files.choose", &Default::default(), "{}")
        .await?;
    ensure!(
        rc >= 10 && serde_json::from_str::<serde_json::Value>(&busy)?["error_code"] == "BUSY",
        "real provider did not refuse BUSY: {busy:?}"
    );
    for id in ids {
        bus.call("files", "files.choose.cancel", json!({"request":id}))
            .await?;
    }
    ensure!(
        bus.call("files", "files.choose.list", json!({})).await?["requests"]
            .as_array()
            .context("list")?
            .is_empty(),
        "parked window left behind"
    );
    println!("PASS two Handles, external cancel is final, real per-caller BUSY");
    Ok(())
}
