// SPDX-License-Identifier: MIT OR Apache-2.0
//! The document edit table, shared by the window and headless dispatch.
use crate::{Crop, Document, Shape};
use action::{Class, Action, Error, ParamSpec};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

pub trait HasDocument {
    fn document(&self) -> Option<&Document>;
    fn document_mut(&mut self) -> Option<&mut Document>;
    fn document_changed(&mut self) {}
}

pub type Annotate = Shape;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Move {
    pub id: u64,
    pub dx: f32,
    pub dy: f32,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Delete {
    pub id: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetCrop {
    pub crop: Option<Crop>,
}

fn parse<T: DeserializeOwned>(id: &'static str, body: &Value) -> Result<T, Error> {
    serde_json::from_value(body.clone()).map_err(|e| Error::BadParams {
        id,
        msg: e.to_string(),
    })
}
fn failure(id: &'static str, msg: String) -> Error {
    match msg.as_str() {
        "annotation limit reached"
        | "document point budget exceeded"
        | "annotation id exhausted" => Error::Failed(msg),
        _ => Error::BadParams { id, msg },
    }
}
fn document<S: HasDocument>(s: &mut S) -> Result<&mut Document, Error> {
    s.document_mut()
        .ok_or_else(|| Error::Failed("no image".into()))
}
fn result<S: HasDocument>(s: &mut S) -> Result<Value, Error> {
    s.document_changed();
    Ok(document(s)?.info())
}
fn annotate<S: HasDocument>(s: &mut S, body: &Value) -> Result<Value, Error> {
    let shape: Annotate = parse("edit.annotate", body)?;
    let id = document(s)?
        .add(shape)
        .map_err(|e| failure("edit.annotate", e))?;
    let mut info = result(s)?;
    info["id"] = json!(id);
    Ok(info)
}
fn move_object<S: HasDocument>(s: &mut S, body: &Value) -> Result<Value, Error> {
    let a: Move = parse("edit.move", body)?;
    document(s)?
        .move_object(a.id, a.dx, a.dy)
        .map_err(|e| failure("edit.move", e))?;
    result(s)
}
fn delete<S: HasDocument>(s: &mut S, body: &Value) -> Result<Value, Error> {
    let a: Delete = parse("edit.delete", body)?;
    document(s)?
        .delete(a.id)
        .map_err(|e| failure("edit.delete", e))?;
    result(s)
}
fn crop<S: HasDocument>(s: &mut S, body: &Value) -> Result<Value, Error> {
    let a: SetCrop = parse("edit.crop", body)?;
    document(s)?
        .set_crop(a.crop)
        .map_err(|e| failure("edit.crop", e))?;
    result(s)
}
fn undo<S: HasDocument>(s: &mut S, _: &Value) -> Result<Value, Error> {
    document(s)?.undo();
    result(s)
}
fn redo<S: HasDocument>(s: &mut S, _: &Value) -> Result<Value, Error> {
    document(s)?.redo();
    result(s)
}
pub fn with_document<S: HasDocument>(s: &S) -> Result<(), &'static str> {
    s.document().map(|_| ()).ok_or("no image")
}
pub fn can_undo<S: HasDocument>(s: &S) -> Result<(), &'static str> {
    with_document(s)?;
    s.document()
        .is_some_and(Document::can_undo)
        .then_some(())
        .ok_or("nothing to undo")
}
pub fn can_redo<S: HasDocument>(s: &S) -> Result<(), &'static str> {
    with_document(s)?;
    s.document()
        .is_some_and(Document::can_redo)
        .then_some(())
        .ok_or("nothing to redo")
}

pub fn commands<S: HasDocument>() -> Vec<Action<S>> {
    let typed = |id, label, run, params| Action {
        id,
        label,
        menu: None,
        submenu: None,
        group: 0,
        shortcut: None,
        icon: None,
        enabled: with_document::<S>,
        run,
        params: Some(params),
        class: Class::Intent,
    };
    let mut undo_command = action::nullary!("edit.undo", "undo", |_: &mut S| {});
    undo_command.run = undo::<S>;
    undo_command.enabled = can_undo::<S>;
    undo_command.menu = Some("edit");
    undo_command.shortcut = Some("Ctrl+Z");
    undo_command.icon = Some("undo-2");
    let mut redo_command = action::nullary!("edit.redo", "redo", |_: &mut S| {});
    redo_command.run = redo::<S>;
    redo_command.enabled = can_redo::<S>;
    redo_command.menu = Some("edit");
    redo_command.shortcut = Some("Ctrl+Shift+Z");
    redo_command.icon = Some("redo-2");
    vec![
        undo_command,
        redo_command,
        typed(
            "edit.annotate",
            "annotate",
            annotate::<S>,
            ParamSpec {
                type_name: "annotation::Annotate",
                schema: || schema_for!(Annotate),
            },
        ),
        typed(
            "edit.move",
            "move",
            move_object::<S>,
            ParamSpec {
                type_name: "annotation::Move",
                schema: || schema_for!(Move),
            },
        ),
        typed(
            "edit.delete",
            "delete",
            delete::<S>,
            ParamSpec {
                type_name: "annotation::Delete",
                schema: || schema_for!(Delete),
            },
        ),
        typed(
            "edit.crop",
            "crop",
            crop::<S>,
            ParamSpec {
                type_name: "annotation::SetCrop",
                schema: || schema_for!(SetCrop),
            },
        ),
    ]
}
