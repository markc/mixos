// SPDX-License-Identifier: MIT OR Apache-2.0
use action::{Error, Origin, Registry};
use annotation::{Document, HasDocument};
use serde_json::{Value, json};

struct State {
    document: Option<Document>,
    changes: usize,
}
impl HasDocument for State {
    fn document(&self) -> Option<&Document> {
        self.document.as_ref()
    }
    fn document_mut(&mut self) -> Option<&mut Document> {
        self.document.as_mut()
    }
    fn document_changed(&mut self) {
        self.changes += 1;
    }
}
#[test]
fn engine_table_owns_validation_history_enablement_and_results() {
    let mut r = Registry::new();
    for c in annotation::commands() {
        r.add(c);
    }
    let mut s = State {
        document: None,
        changes: 0,
    };
    assert!(matches!(
        r.execute("edit.annotate", &mut s, json!({}), Origin::Bus),
        Err(Error::Disabled { .. })
    ));
    s.document = Some(Document::new(image::RgbaImage::new(16, 16)).unwrap());
    let params = json!({"kind":"arrow", "points":[{"x":1,"y":1},{"x":8,"y":8}], "colour":[255,0,0,255], "width":2});
    let result = r
        .execute("edit.annotate", &mut s, params.clone(), Origin::Ui)
        .unwrap();
    let id = result["id"].as_u64().unwrap();
    let event = r.take_executed().0.remove(0);
    assert_eq!(event.params, params);
    assert_eq!(event.result, result);
    println!("UI_EXECUTED {}", event.body());
    for (id, params) in [
        ("edit.delete", json!({"id":999})),
        ("edit.move", json!({"id":id,"dx":0,"dy":0,"extra":1})),
        (
            "edit.crop",
            json!({"crop":{"x":0,"y":0,"width":99,"height":1}}),
        ),
        ("edit.annotate", json!({"kind":"bad"})),
    ] {
        assert!(matches!(
            r.execute(id, &mut s, params, Origin::Bus),
            Err(Error::BadParams { .. })
        ));
    }
    assert_eq!(s.changes, 1);
    assert!(r.take_executed().0.is_empty());
    r.execute(
        "edit.move",
        &mut s,
        json!({"id":id,"dx":2,"dy":0}),
        Origin::Bus,
    )
    .unwrap();
    r.execute("edit.undo", &mut s, Value::Null, Origin::Bus)
        .unwrap();
    assert_eq!(s.document().unwrap().objects()[0].shape.points[0].x, 1.0);
    r.execute("edit.redo", &mut s, Value::Null, Origin::Bus)
        .unwrap();
    r.execute("edit.delete", &mut s, json!({"id":id}), Origin::Bus)
        .unwrap();
    assert!(s.document().unwrap().objects().is_empty());
}
