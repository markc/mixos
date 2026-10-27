// SPDX-License-Identifier: MIT OR Apache-2.0
//! Open journald only when an admitted event reaches the sink.
use std::sync::Mutex;
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

pub(crate) struct LazyJournal {
    identity: String,
    fallback: bool,
    // Serialises first-use span replay with record/event callbacks. No worker.
    sink: Mutex<Option<Result<tracing_journald::Layer, std::io::Error>>>,
}

#[derive(Default)]
struct SpanFields {
    fields: Vec<(Field, Scalar)>,
    installed: bool,
}
enum Scalar {
    Text(String),
    I64(i64),
    U64(u64),
    F64(f64),
    Bool(bool),
}
impl Scalar {
    fn value(&self) -> &dyn tracing::field::Value {
        match self {
            Self::Text(v) => v,
            Self::I64(v) => v,
            Self::U64(v) => v,
            Self::F64(v) => v,
            Self::Bool(v) => v,
        }
    }
}
impl SpanFields {
    fn put(&mut self, field: &Field, value: Scalar) {
        if let Some((_, old)) = self.fields.iter_mut().find(|(key, _)| key == field) {
            *old = value;
        } else {
            self.fields.push((field.clone(), value));
        }
    }
}
impl Visit for SpanFields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.put(field, Scalar::Text(format!("{value:?}")));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.put(field, Scalar::Text(value.into()));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.put(field, Scalar::I64(value));
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.put(field, Scalar::U64(value));
    }
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.put(field, Scalar::F64(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.put(field, Scalar::Bool(value));
    }
}

impl LazyJournal {
    pub(crate) fn new(identity: &str, fallback: bool) -> Self {
        Self {
            identity: identity.into(),
            fallback,
            sink: Mutex::new(None),
        }
    }
}
impl<S> Layer<S> for LazyJournal
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = SpanFields::default();
        attrs.record(&mut fields);
        ctx.span(id)
            .expect("registered span")
            .extensions_mut()
            .insert(fields);
    }
    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let sink = self.sink.lock().unwrap_or_else(|e| e.into_inner());
        let span = ctx.span(id).expect("registered span");
        let mut extensions = span.extensions_mut();
        let fields = extensions.get_mut::<SpanFields>().expect("span fields");
        let installed = fields.installed;
        if !installed { values.record(fields); }
        drop(extensions);
        drop(span);
        if installed && let Some(Ok(sink)) = sink.as_ref() {
            sink.on_record(id, values, ctx);
        }
    }
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut slot = self.sink.lock().unwrap_or_else(|e| e.into_inner());
        let sink = slot.get_or_insert_with(|| {
            tracing_journald::layer()
                .map(|layer| layer.with_syslog_identifier(self.identity.clone()))
                .inspect_err(|error| eprintln!("logging: journald sink disabled: {error}"))
        });
        match sink {
            Ok(sink) => {
                // Spans may predate the first event. Replay their retained
                // fields before upstream's layer looks them up; no socket is
                // needed for span creation or recording.
                for span in ctx
                    .lookup_current()
                    .into_iter()
                    .flat_map(|span| span.scope().from_root())
                {
                    let mut extensions = span.extensions_mut();
                    let fields = extensions.get_mut::<SpanFields>().expect("span fields");
                    if fields.installed {
                        continue;
                    }
                    fields.installed = true;
                    let owned = std::mem::take(&mut fields.fields);
                    drop(extensions);
                    let values: Vec<_> = span
                        .metadata()
                        .fields()
                        .iter()
                        .map(|field| {
                            owned
                                .iter()
                                .find(|(key, _)| *key == field)
                                .map(|(_, value)| value.value())
                        })
                        .collect();
                    let set = span.metadata().fields().value_set_all(&values);
                    let attrs = Attributes::new(span.metadata(), &set);
                    sink.on_new_span(&attrs, &span.id(), ctx.clone());
                }
                sink.on_event(event, ctx);
            }
            Err(_) if self.fallback => {
                use std::io::Write;
                struct Stderr<'a>(&'a mut std::io::StderrLock<'static>);
                impl Visit for Stderr<'_> {
                    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                        let _ = write!(self.0, " {}={value:?}", field.name());
                    }
                }
                let mut stderr = std::io::stderr().lock();
                let _ = write!(
                    stderr,
                    "{} {}",
                    event.metadata().level(),
                    event.metadata().target()
                );
                event.record(&mut Stderr(&mut stderr));
                let _ = writeln!(stderr);
            }
            Err(_) => {}
        }
    }
}
