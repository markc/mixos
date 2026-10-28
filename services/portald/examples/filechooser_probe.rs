// SPDX-License-Identifier: MIT OR Apache-2.0
//! Persistent zbus caller for chooser_bus_test.mix. Drives only requests whose
//! correlation belongs to this connection on the test's private native Bus.
use anyhow::{Context, ensure};
use bus::native_client::SupervisedClient;
use futures_util::StreamExt;
use portald::filechooser::{INTERFACE, mapping::Options};
use portald::portal::{BUS_NAME, OBJECT_PATH};
use serde_json::{Value as Json, json};
use std::collections::HashMap;
use std::time::Duration;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

const WAIT: Duration = Duration::from_secs(20);

fn put(options: &mut Options, key: &str, value: Value<'_>) -> anyhow::Result<()> {
    options.insert(key.into(), OwnedValue::try_from(value)?); Ok(())
}

async fn drive(bus: &SupervisedClient, id: &str, command: &str, mut body: Json) -> anyhow::Result<Json> {
    body["viewport"] = json!(id);
    Ok(bus.call("files", command, body).await?)
}

async fn one(client: &Connection, bus: &SupervisedClient, folder: &str, method: &str, cancel: bool) -> anyhow::Result<()> {
    let token = format!("probe{method}{}", u8::from(cancel));
    let path = portald::screencast::handle::request_path(client.unique_name().context("unique name")?.as_str(), &token)?;
    let rule = MatchRule::builder().msg_type(zbus::message::Type::Signal)
        .interface("org.freedesktop.portal.Request")?.member("Response")?.path(path.clone())?.build();
    let mut responses = MessageStream::for_match_rule(rule, client, Some(8)).await?;
    let mut options = Options::new();
    put(&mut options,"handle_token",Value::from(token))?;
    let mut bytes = folder.as_bytes().to_vec(); bytes.push(0);
    put(&mut options,"current_folder",Value::from(bytes))?;
    if method == "SaveFile" { put(&mut options,"current_name",Value::from("notes.txt"))?; }
    if method == "SaveFiles" { put(&mut options,"files",Value::from(vec![b"one.txt\0".to_vec(),b"two.txt\0".to_vec()]))?; }
    let reply = client.call_method(Some(BUS_NAME),OBJECT_PATH,Some(INTERFACE),method,&("","Test chooser",options)).await?;
    let actual: OwnedObjectPath = reply.body().deserialize()?;
    ensure!(actual == path,"Request path differs");
    let id = tokio::time::timeout(WAIT, async {
        loop {
            let list = bus.call("files","files.choose.list",json!({})).await?;
            if let Some(id) = list["requests"].as_array().into_iter().flatten()
                .find(|entry| entry["correlation"].as_str() == Some(path.as_str()))
                .and_then(|entry|entry["request"].as_str()) { break Ok::<_,anyhow::Error>(id.to_owned()); }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }).await??;
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(tree) = drive(bus,&id,"files.ui.tree",json!({})).await
                && tree.to_string().contains("Places") { break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }).await?;
    if cancel {
        let _ = drive(bus,&id,"files.ui.click",json!({"label":"Cancel"})).await;
    } else if method == "OpenFile" {
        drive(bus,&id,"files.ui.click",json!({"label":"notes.txt"})).await?;
        let _ = drive(bus,&id,"files.ui.click",json!({"label":"Open"})).await;
    } else {
        // SaveFile exercises replacement consent; SaveFiles uses the folder.
        let _ = drive(bus,&id,"files.ui.click",json!({"label":"Save"})).await;
        if method == "SaveFile" {
            tokio::time::timeout(WAIT, async {
                loop {
                    let tree = drive(bus,&id,"files.ui.tree",json!({})).await?;
                    if tree.to_string().contains("Replace existing file?") { break Ok::<_,anyhow::Error>(()); }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }).await??;
            let _ = drive(bus,&id,"files.ui.click",json!({"label":"Replace"})).await;
        }
    }
    let message = tokio::time::timeout(WAIT,responses.next()).await?.context("Response stream ended")??;
    let (code, results): (u32,HashMap<String,OwnedValue>) = message.body().deserialize()?;
    ensure!(code == if cancel {1} else {0},"{method}: Response {code}");
    if cancel { ensure!(results.is_empty(),"cancel carries results"); }
    else {
        let uris = Vec::<String>::try_from(results["uris"].try_clone()?)?;
        let expected = if method == "SaveFiles" {vec![format!("file://{folder}/one.txt"),format!("file://{folder}/two.txt")]}
            else {vec![format!("file://{folder}/notes.txt")]};
        ensure!(uris == expected,"{method}: {uris:?} differs from {expected:?}");
    }
    ensure!(tokio::time::timeout(Duration::from_millis(100),responses.next()).await.is_err(),"duplicate Response");
    println!("PASS {method} {} through D-Bus, portald, native files and viewport {id}",if cancel {"cancel"} else {"accept"});
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let folder = std::env::args().nth(1).context("folder argument")?;
    let client = Connection::session().await?;
    let bus = SupervisedClient::connect_options("portal-chooser-probe",&bus::client_helpers::resolve_noded_url()).connect().await?;
    tokio::time::timeout(WAIT,async {
        loop {
            if client.call_method(Some(BUS_NAME),OBJECT_PATH,Some("org.freedesktop.DBus.Properties"),"Get",&(INTERFACE,"version")).await.is_ok() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }).await?;
    for method in ["OpenFile","SaveFile","SaveFiles"] {
        one(&client,&bus,&folder,method,false).await?;
        one(&client,&bus,&folder,method,true).await?;
    }
    for save in [false, true] {
        for cancel in [false, true] { rfd_dialog(&bus, &folder, save, cancel).await?; }
    }
    ensure!(std::fs::read_to_string(format!("{folder}/notes.txt"))? == "original","chooser wrote existing file");
    for name in ["one.txt","two.txt"] { ensure!(!std::path::Path::new(&format!("{folder}/{name}")).exists(),"chooser wrote file"); }
    bus.close().await;
    Ok(())
}

async fn rfd_dialog(bus: &SupervisedClient, folder: &str, save: bool, cancel: bool) -> anyhow::Result<()> {
    let title = format!("rfd probe {save} {cancel}");
    let directory = folder.to_owned();
    let dialog_title = title.clone();
    let dialog = tokio::task::spawn_blocking(move || {
        let dialog = rfd::FileDialog::new().set_title(dialog_title).set_directory(directory).add_filter("Text", &["txt"]);
        if save { dialog.set_file_name("rfd-output.txt").save_file() } else { dialog.pick_file() }
    });
    let id = tokio::time::timeout(WAIT, async {
        loop {
            let list = bus.call("files", "files.choose.list", json!({})).await?;
            if let Some(id) = list["requests"].as_array().into_iter().flatten()
                .find(|entry| entry["title"] == title).and_then(|entry| entry["request"].as_str()) {
                break Ok::<_, anyhow::Error>(id.to_owned());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }).await??;
    tokio::time::timeout(WAIT, async {
        loop {
            if let Ok(tree) = drive(bus, &id, "files.ui.tree", json!({})).await
                && tree.to_string().contains("Places") { break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }).await?;
    if cancel {
        let _ = drive(bus, &id, "files.ui.click", json!({"label":"Cancel"})).await;
    } else if save {
        let _ = drive(bus, &id, "files.ui.click", json!({"label":"Save"})).await;
    } else {
        drive(bus, &id, "files.ui.click", json!({"label":"notes.txt"})).await?;
        let _ = drive(bus, &id, "files.ui.click", json!({"label":"Open"})).await;
    }
    let result = tokio::time::timeout(WAIT, dialog).await??;
    if cancel { ensure!(result.is_none(), "rfd cancel returned a file"); }
    else {
        let expected = std::path::Path::new(folder).join(if save { "rfd-output.txt" } else { "notes.txt" });
        ensure!(result.as_ref() == Some(&expected), "rfd returned {result:?}, expected {expected:?}");
    }
    ensure!(!std::path::Path::new(folder).join("rfd-output.txt").exists(), "chooser wrote rfd output");
    println!("PASS rfd 0.17.2 save={save} cancel={cancel} through portal viewport {id}");
    Ok(())
}
